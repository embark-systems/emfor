//! Command line entry point: `emfor [--config <path>] [--check-config]`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{mpsc, oneshot, watch};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, reload};

use crate::config::{self, Config, ConfigError, DEFAULT_CONFIG_PATH, DnsConfig, in_zone};
use crate::dns::{self, DnsSettings};
use crate::live::LiveState;
use crate::plugin::{PluginSet, Prepared};
use crate::store::Store;
use crate::supervisor::Supervisor;
use crate::writer::{self, WriterMsg};

/// Exit status after clean shutdown or a valid `--check-config`.
pub const EXIT_OK: u8 = 0;
/// Exit status when the store cannot be opened or is locked by another daemon.
pub const EXIT_STARTUP: u8 = 1;
/// Exit status for an invalid config.
pub const EXIT_CONFIG: u8 = 2;

#[derive(Debug, Parser)]
#[command(
    name = "emfor",
    version,
    about = "Per-host backend health-check daemon"
)]
pub struct Cli {
    /// Config file.
    #[arg(long, value_name = "PATH", default_value = DEFAULT_CONFIG_PATH)]
    pub config: PathBuf,
    /// Validate the config file and exit.
    #[arg(long)]
    pub check_config: bool,
}

pub fn main() -> ExitCode {
    let cli = Cli::parse();
    // Loading a config starts plugins, so the runtime comes first.
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot start async runtime: {e}");
            return ExitCode::from(EXIT_STARTUP);
        }
    };
    runtime.block_on(start(cli))
}

async fn start(cli: Cli) -> ExitCode {
    if cli.check_config {
        let mut plugins = PluginSet::default();
        let (prepared, loaded) = load(&cli.config, None, &plugins).await;
        // Nothing keeps running after a config check.
        plugins.commit(prepared).await;
        plugins.stop_all().await;
        return match loaded {
            Ok(c) => {
                println!(
                    "config ok: {} services, {} backends",
                    c.services.len(),
                    c.backend_count()
                );
                ExitCode::from(EXIT_OK)
            }
            Err(errors) => {
                for e in &errors {
                    eprintln!("{e}");
                }
                ExitCode::from(EXIT_CONFIG)
            }
        };
    }

    // Log at `info` while loading (plugin start lines), then switch to the
    // configured level.
    let log = init_logging("info");
    let mut plugins = PluginSet::default();
    let (prepared, loaded) = load(&cli.config, None, &plugins).await;
    let config = match loaded {
        Ok(c) => c,
        Err(errors) => {
            prepared.rollback().await;
            log_config_errors(&cli.config, &errors);
            return ExitCode::from(EXIT_CONFIG);
        }
    };
    plugins.commit(prepared).await;
    if let Some(h) = &log
        && let Err(e) = h.reload(log_filter(&config.log.level))
    {
        tracing::error!("cannot set log level: {e}");
    }
    let code = run(cli.config, config, &mut plugins, log).await;
    plugins.stop_all().await;
    code
}

type LogHandle = reload::Handle<EnvFilter, tracing_subscriber::Registry>;

/// Install logging. Returns a handle to change the level on reload, or `None`
/// when `RUST_LOG` is set (it then overrides `[log] level` for the whole run).
fn init_logging(level: &str) -> Option<LogHandle> {
    let from_env = EnvFilter::try_from_default_env().ok();
    let env_set = from_env.is_some();
    let (filter, handle) = reload::Layer::new(from_env.unwrap_or_else(|| log_filter(level)));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .try_init();
    (!env_set).then_some(handle)
}

/// Filter for `[log] level`. The hickory DNS crates log every query at
/// `info`, so they are capped at `warn` (or the configured level when it is
/// stricter). `RUST_LOG` bypasses this.
fn log_filter(level: &str) -> EnvFilter {
    let hickory = if level == "error" { "error" } else { "warn" };
    EnvFilter::new(format!(
        "{level},hickory_server={hickory},hickory_proto={hickory},hickory_net={hickory}"
    ))
}

fn log_config_errors(path: &Path, errors: &[ConfigError]) {
    tracing::error!(
        "invalid config {}: {} error(s)",
        path.display(),
        errors.len()
    );
    for e in errors {
        tracing::error!("{e}");
    }
}

async fn run(
    config_path: PathBuf,
    config: Config,
    plugins: &mut PluginSet,
    log: Option<LogHandle>,
) -> ExitCode {
    let store = match Store::open(&config.store) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("cannot open state store: {e}");
            return ExitCode::from(EXIT_STARTUP);
        }
    };
    let (Ok(mut hup), Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::hangup()),
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        tracing::error!("cannot install signal handlers");
        return ExitCode::from(EXIT_STARTUP);
    };

    let live = LiveState::default();
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dns_settings = |d: &DnsConfig| DnsSettings {
        zone: d.zone.clone(),
        ttl: d.ttl,
        serial: started as u32,
    };
    let mut dns_server = None;
    let mut dns_tx = None;
    if let Some(d) = &config.dns {
        let (stx, srx) = watch::channel(dns_settings(d));
        match dns::serve(&d.listen, dns::Handler::new(live.clone(), srx)).await {
            Ok(server) => dns_server = Some(server),
            Err(e) => {
                tracing::error!("cannot start DNS server: {e}");
                return ExitCode::from(EXIT_STARTUP);
            }
        }
        dns_tx = Some(stx);
        let addrs: Vec<String> = d.listen.iter().map(|a| a.to_string()).collect();
        tracing::info!("DNS server for zone {} on {}", d.zone, addrs.join(", "));
    }
    warn_dns(&config);

    let (tx, rx) = mpsc::unbounded_channel();
    let writer = writer::spawn_live(store, rx, live);
    let mut supervisor = Supervisor::start(&config, tx.clone());
    tracing::info!(
        "emfor started: {} services, {} backends, store {}",
        config.services.len(),
        config.backend_count(),
        config.store.path.display()
    );
    let mut current = config;

    loop {
        tokio::select! {
            _ = hup.recv() => {
                let (prepared, loaded) = load(&config_path, Some(&current), plugins).await;
                match loaded {
                    Ok(new) => {
                        if new.log != current.log {
                            match &log {
                                Some(h) => match h.reload(log_filter(&new.log.level)) {
                                    Ok(()) => tracing::info!("log level set to {}", new.log.level),
                                    Err(e) => tracing::error!("cannot change log level: {e}"),
                                },
                                None => tracing::warn!("[log] level changed but RUST_LOG is set; keeping RUST_LOG"),
                            }
                        }
                        supervisor.reload(&new);
                        // After the supervisor switched checkers, so replaced
                        // plugins are no longer used when they stop.
                        plugins.commit(prepared).await;
                        if let (Some(tx), Some(d)) = (&dns_tx, &new.dns) {
                            tx.send_replace(dns_settings(d));
                        }
                        warn_dns(&new);
                        tracing::info!("config reloaded: {} services, {} backends", new.services.len(), new.backend_count());
                        current = new;
                    }
                    Err(errors) => {
                        prepared.rollback().await;
                        log_config_errors(&config_path, &errors);
                        tracing::error!("reload rejected; keeping the previous config");
                    }
                }
            }
            _ = term.recv() => break,
            _ = int.recv() => break,
        }
    }

    tracing::info!("shutting down");
    if let Some(mut server) = dns_server
        && let Err(e) = server.shutdown_gracefully().await
    {
        tracing::debug!("DNS server shutdown: {e}");
    }
    supervisor.stop();
    let (done_tx, done_rx) = oneshot::channel();
    let _ = tx.send(WriterMsg::Shutdown(done_tx));
    let _ = done_rx.await;
    let _ = writer.await;
    ExitCode::from(EXIT_OK)
}

/// DNS warnings for a loaded config: services outside the zone, and
/// services in the zone whose backends use different ports.
fn warn_dns(config: &Config) {
    warn_outside_zone(config);
    for m in port_warnings(config) {
        tracing::warn!("{m}");
    }
}

/// One message per service under the DNS zone whose backends (both address
/// families together) do not share one port: A/AAAA answers carry no port.
fn port_warnings(config: &Config) -> Vec<String> {
    let Some(d) = &config.dns else {
        return Vec::new();
    };
    config
        .services
        .iter()
        .filter(|s| in_zone(&s.name, &d.zone))
        .filter_map(|s| {
            let ports: BTreeSet<u16> = s.backends.iter().map(|b| b.addr.port()).collect();
            (ports.len() > 1).then(|| {
                let list: Vec<String> = ports.iter().map(u16::to_string).collect();
                format!(
                    "service {} has backends on different ports ({}); DNS A/AAAA answers carry no port, so use SRV records or an emfor client to get the port",
                    s.name,
                    list.join(", ")
                )
            })
        })
        .collect()
}

/// One `warn` line per service that DNS cannot answer for.
fn warn_outside_zone(config: &Config) {
    let Some(d) = &config.dns else { return };
    for s in &config.services {
        if !in_zone(&s.name, &d.zone) {
            tracing::warn!(
                "service {} is outside DNS zone {} and cannot be resolved via DNS",
                s.name,
                d.zone
            );
        }
    }
}

/// Load a config: start its plugins (reusing running ones whose command is
/// unchanged), then parse and validate with them on a blocking thread. On a
/// reload (`current` set), a changed `[store]` table or `[dns] listen` is an
/// error too. Every error is returned together. The caller commits or rolls
/// back the returned plugins.
async fn load(
    path: &Path,
    current: Option<&Config>,
    plugins: &PluginSet,
) -> (Prepared, Result<Config, Vec<ConfigError>>) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            let error = ConfigError {
                path: String::new(),
                message: format!("cannot read {}: {e}", path.display()),
            };
            return (Prepared::default(), Err(vec![error]));
        }
    };
    let (specs, _) = config::parse_plugins(&text);
    let mut prepared = plugins.prepare(&specs).await;
    let registry = prepared.registry(tokio::runtime::Handle::current(), current);
    let parsed = {
        let text = text.clone();
        tokio::task::spawn_blocking(move || config::parse(&text, &registry))
            .await
            .unwrap_or_else(|e| {
                Err(vec![ConfigError {
                    path: String::new(),
                    message: format!("config validation failed: {e}"),
                }])
            })
    };
    let mut errors = std::mem::take(&mut prepared.errors);
    if let Some(current) = current {
        if config::parse_store(&text).is_some_and(|s| s != current.store) {
            errors.push(ConfigError {
                path: "store".into(),
                message: "store settings require restart".into(),
            });
        }
        let current_listen = current.dns.as_ref().map(|d| d.listen.clone());
        if config::parse_dns_listen(&text).is_some_and(|l| l != current_listen) {
            errors.push(ConfigError {
                path: "dns.listen".into(),
                message: "dns listen settings require restart".into(),
            });
        }
    }
    let result = match parsed {
        Ok(new) if errors.is_empty() => Ok(new),
        Ok(_) => Err(errors),
        Err(mut parse_errors) => {
            parse_errors.extend(errors);
            Err(parse_errors)
        }
    };
    (prepared, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::Registry;

    fn warnings(dns: bool, name: &str, backends: &[&str]) -> Vec<String> {
        let list: Vec<String> = backends.iter().map(|b| format!("\"{b}\"")).collect();
        let text = format!(
            "{}[services.\"{name}\"]\ncheck = \"tcp\"\nbackends = [{}]\n",
            if dns {
                "[dns]\nzone = \"svc.emfor\"\n\n"
            } else {
                ""
            },
            list.join(", ")
        );
        let config =
            config::parse(&text, &Registry::builtin()).unwrap_or_else(|e| panic!("errors: {e:?}"));
        port_warnings(&config)
    }

    /// Example "which services are reported".
    #[test]
    fn which_services_are_reported() {
        type Row<'a> = (bool, &'a str, &'a [&'a str], Option<&'a str>);
        let rows: &[Row] = &[
            (
                true,
                "api.svc.emfor",
                &["10.0.0.11:8080", "10.0.0.12:8081"],
                Some("8080, 8081"),
            ),
            (
                true,
                "api.svc.emfor",
                &["10.0.0.12:9000", "10.0.0.11:8080", "10.0.0.13:8080"],
                Some("8080, 9000"),
            ),
            (
                true,
                "api.svc.emfor",
                &["10.0.0.11:8080", "[2001:db8::11]:8080"],
                None,
            ),
            (
                true,
                "api.svc.emfor",
                &["10.0.0.11:8080", "[2001:db8::11]:8443"],
                Some("8080, 8443"),
            ),
            (
                true,
                "db.internal",
                &["10.0.0.21:5432", "10.0.0.22:5433"],
                None,
            ),
            (
                false,
                "api.svc.emfor",
                &["10.0.0.11:8080", "10.0.0.12:8081"],
                None,
            ),
        ];
        for (dns, name, backends, want) in rows {
            let got = warnings(*dns, name, backends);
            match want {
                None => assert!(got.is_empty(), "{name} {backends:?}: {got:?}"),
                Some(ports) => {
                    assert_eq!(got.len(), 1, "{name} {backends:?}: {got:?}");
                    let m = &got[0];
                    assert!(m.contains(name), "{m}");
                    assert!(m.contains(&format!("({ports})")), "{m}");
                    assert!(m.contains("SRV") && m.contains("port"), "{m}");
                }
            }
        }
    }
}
