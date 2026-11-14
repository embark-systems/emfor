//! Command line entry point: `emfor [--config <path>] <server|check-config|status>`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use emfor_format::{HIST_KEY_PREFIX, HealthState, ServiceHistory};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{mpsc, oneshot, watch};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, reload};

use crate::config::{self, Config, ConfigError, DEFAULT_CONFIG_PATH, DnsConfig, in_zone};
use crate::dns::{self, DnsSettings};
use crate::live::LiveState;
use crate::metrics;
use crate::peers::{self, Targets};
use crate::plugin::{PluginSet, Prepared};
use crate::store::Store;
use crate::supervisor::Supervisor;
use crate::writer::{self, WriterMsg};

/// Exit status after clean shutdown or a valid `emfor check-config`.
pub const EXIT_OK: u8 = 0;
/// Exit status when the store cannot be opened or is locked by another daemon.
pub const EXIT_STARTUP: u8 = 1;
/// Exit status when the state writer or a DNS listener stops while the
/// daemon runs: `EX_SOFTWARE`, which service managers restart, unlike 1
/// and 2.
pub const EXIT_INTERNAL: u8 = 70;
/// Exit status for an invalid config, and for a command line without a
/// subcommand.
pub const EXIT_CONFIG: u8 = 2;

#[derive(Debug, Parser)]
#[command(
    name = "emfor",
    version,
    about = "Per-host backend health-check daemon",
    override_usage = "emfor [OPTIONS] <COMMAND>"
)]
pub struct Cli {
    /// Config file.
    #[arg(long, value_name = "PATH", default_value = DEFAULT_CONFIG_PATH, global = true)]
    pub config: PathBuf,
    /// Replaced by `emfor check-config`; only prints that.
    #[arg(long, hide = true, global = true)]
    pub check_config: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Run the daemon.
    Server,
    /// Validate the config file, then exit: 0 when it is valid, 2 when not.
    CheckConfig,
    /// Show the health state the running daemon publishes, then exit.
    ///
    /// Exits 1 when the daemon is not updating the store or a shown service
    /// has no healthy backend, 2 when the store cannot be read.
    Status(crate::status::StatusArgs),
}

pub fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.check_config {
        eprintln!("--check-config was replaced by \"emfor check-config\"");
        return ExitCode::from(EXIT_CONFIG);
    }
    let check_only = match &cli.command {
        None => {
            // Never start a daemon by accident: only `server` does.
            use clap::CommandFactory;
            let _ = Cli::command().write_help(&mut std::io::stderr());
            return ExitCode::from(EXIT_CONFIG);
        }
        Some(Command::Status(args)) => return crate::status::main(&cli.config, args),
        Some(Command::CheckConfig) => true,
        Some(Command::Server) => false,
    };
    // Loading a config starts plugins, so the runtime comes first.
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot start async runtime: {e}");
            return ExitCode::from(EXIT_STARTUP);
        }
    };
    let code = runtime.block_on(start(cli.config, check_only));
    // A reload abandoned at shutdown can leave a config validation running
    // on a blocking thread, waiting up to 5s per plugin; do not wait for it.
    runtime.shutdown_timeout(Duration::from_secs(1));
    code
}

async fn start(config_path: PathBuf, check_only: bool) -> ExitCode {
    if check_only {
        let mut plugins = PluginSet::default();
        let (prepared, loaded) = load(&config_path, None, &plugins).await;
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
    // Before loading, which starts plugins and can take seconds: until a
    // handler exists, SIGHUP's default action kills the daemon. A signal
    // that arrives meanwhile waits for the main loop, where SIGHUP reloads
    // and SIGTERM / SIGINT stop.
    let Some(mut signals) = Signals::install() else {
        tracing::error!("cannot install signal handlers");
        return ExitCode::from(EXIT_STARTUP);
    };
    let mut plugins = PluginSet::default();
    // Loading can wait seconds on plugins (start, handshake, validate); a
    // stop request ends it there, as during a reload. The plugins started so
    // far are killed when the runtime shuts down.
    let outcome = {
        let startup = load(&config_path, None, &plugins);
        tokio::pin!(startup);
        tokio::select! {
            r = &mut startup => Some(r),
            _ = signals.term.recv() => None,
            _ = signals.int.recv() => None,
        }
    };
    let Some((prepared, loaded)) = outcome else {
        tracing::info!("stop requested while starting; exiting");
        return ExitCode::from(EXIT_OK);
    };
    let config = match loaded {
        Ok(c) => c,
        Err(errors) => {
            prepared.rollback().await;
            log_config_errors(&config_path, &errors);
            return ExitCode::from(EXIT_CONFIG);
        }
    };
    plugins.commit(prepared).await;
    if let Some(h) = &log
        && let Err(e) = h.reload(log_filter(&config.log.level))
    {
        tracing::error!("cannot set log level: {e}");
    }
    let code = run(config_path, config, &mut plugins, log, signals).await;
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
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(log_colors()),
        )
        .try_init();
    (!env_set).then_some(handle)
}

/// Color log lines only on a terminal without `NO_COLOR`, as `emfor
/// status` does: under systemd, a service manager or a pipe, escape
/// sequences end up in the journal or log file.
fn log_colors() -> bool {
    use std::io::IsTerminal;
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none()
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

/// Backend state history left by earlier runs, by service name. A value that
/// does not parse (or is not UTF-8) is logged and loaded as empty, so the
/// writer deletes it; the other services' history still loads.
/// When the keys cannot be read at all, none is loaded and the writer
/// leaves them as they are.
fn load_history(store: &Store) -> BTreeMap<String, ServiceHistory> {
    let stored = match store.prefix(HIST_KEY_PREFIX) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("cannot read state history, starting without it: {e}");
            return BTreeMap::new();
        }
    };
    stored
        .into_iter()
        .map(|(key, value)| {
            let h = match value.as_deref().map(serde_json::from_str::<ServiceHistory>) {
                Some(Ok(mut h)) => {
                    // Left by a newer daemon: transitions to or from a state
                    // this one does not know are dropped, so that it never
                    // writes such a state back.
                    let known = |s: HealthState| s != HealthState::Unrecognized;
                    for entries in h.backends.values_mut() {
                        entries.retain(|e| known(e.from) && known(e.to));
                    }
                    h.retain(|_| true);
                    h
                }
                Some(Err(e)) => {
                    tracing::warn!("discarding {key}: {e}");
                    ServiceHistory::default()
                }
                None => {
                    tracing::warn!("discarding {key}: not UTF-8");
                    ServiceHistory::default()
                }
            };
            (key[HIST_KEY_PREFIX.len()..].to_owned(), h)
        })
        .collect()
}

struct Signals {
    hup: tokio::signal::unix::Signal,
    term: tokio::signal::unix::Signal,
    int: tokio::signal::unix::Signal,
}

impl Signals {
    fn install() -> Option<Signals> {
        Some(Signals {
            hup: signal(SignalKind::hangup()).ok()?,
            term: signal(SignalKind::terminate()).ok()?,
            int: signal(SignalKind::interrupt()).ok()?,
        })
    }
}

async fn run(
    config_path: PathBuf,
    config: Config,
    plugins: &mut PluginSet,
    log: Option<LogHandle>,
    signals: Signals,
) -> ExitCode {
    let store = match Store::open(&config.store) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("cannot open state store: {e}");
            return ExitCode::from(EXIT_STARTUP);
        }
    };
    let Signals {
        mut hup,
        mut term,
        mut int,
    } = signals;

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
    // Kept until the daemon exits; dropping it stops the server.
    let _metrics_server = match &config.metrics {
        Some(m) => match metrics::serve(&m.listen, live.clone()).await {
            Ok(server) => {
                let addrs: Vec<String> = m.listen.iter().map(|a| a.to_string()).collect();
                tracing::info!("metrics on {}", addrs.join(", "));
                Some(server)
            }
            Err(e) => {
                tracing::error!("cannot start metrics server: {e}");
                return ExitCode::from(EXIT_STARTUP);
            }
        },
        None => None,
    };

    // Validation makes `site` required with `[peers]`.
    let peer_site = |c: &Config| c.site.clone().unwrap_or_default();
    let targets = Targets::default();
    let (tx, rx) = mpsc::unbounded_channel();
    let peers = match &config.peers {
        Some(p) => {
            let off = config.services_off();
            match peers::start(
                p,
                &peer_site(&config),
                targets.clone(),
                Some(tx.clone()),
                off,
            )
            .await
            {
                Ok(handle) => {
                    tracing::info!(
                        "peer group on {} with {} other members",
                        p.listen,
                        p.others().count()
                    );
                    Some(handle)
                }
                Err(e) => {
                    tracing::error!("cannot start peer group: {e}");
                    return ExitCode::from(EXIT_STARTUP);
                }
            }
        }
        None => None,
    };
    live.set_peers(peers.as_ref().map(|h| h.stats.clone()));

    let history = load_history(&store);
    let keys = writer::ListenKeys {
        peers: peers.as_ref().map(|h| h.initial_state.clone()),
        ..listen_keys(&config)
    };
    let mut writer = writer::spawn_live(store, rx, live.clone(), history, keys);
    live.set_plugins(plugins.statuses());
    let mut supervisor = Supervisor::start_with_peers(
        &config,
        tx.clone(),
        targets,
        peers.as_ref().map(peers::PeersHandle::hooks),
    );
    tracing::info!(
        "emfor started: {} services, {} backends, store {}",
        config.services.len(),
        config.backend_count(),
        config.store.path.display()
    );
    let mut current = config;
    let mut exit = EXIT_OK;
    // Check tasks that stopped (only a panic does that) are started again.
    let mut watchdog = tokio::time::interval(Duration::from_secs(1));
    watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            ended = &mut writer => {
                // Only a panic ends it before the Shutdown message. Without it
                // the heartbeat stops and every client sees a stale store, so
                // exit and let the service manager restart the daemon.
                let why = match ended {
                    Err(e) if e.is_panic() => "panicked",
                    _ => "ended",
                };
                tracing::error!("state writer {why}; exiting");
                supervisor.stop();
                return ExitCode::from(EXIT_INTERNAL);
            }
            why = async {
                match dns_server.as_mut() {
                    Some(s) => s.ended().await,
                    None => std::future::pending().await,
                }
            } => {
                // DNS would stay silent while the rest runs: exit, after the
                // usual shutdown, and let the service manager restart it.
                tracing::error!("{why}; exiting");
                exit = EXIT_INTERNAL;
                break;
            }
            _ = watchdog.tick() => {
                supervisor.restart_finished();
            }
            _ = hup.recv() => {
                // A reload can wait seconds on plugins (start, handshake,
                // validate); a stop request ends it there.
                let outcome = {
                    let reload = load(&config_path, Some(&current), plugins);
                    tokio::pin!(reload);
                    tokio::select! {
                        r = &mut reload => Some(r),
                        _ = term.recv() => None,
                        _ = int.recv() => None,
                    }
                };
                let Some((prepared, loaded)) = outcome else {
                    tracing::info!("stop requested while reloading; the reload is abandoned");
                    break;
                };
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
                        live.set_plugins(plugins.statuses());
                        if let (Some(tx), Some(d)) = (&dns_tx, &new.dns) {
                            tx.send_replace(dns_settings(d));
                        }
                        warn_dns(&new);
                        if let (Some(h), Some(p)) = (&peers, &new.peers) {
                            h.reload(p, &peer_site(&new));
                        }
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
    if let Some(server) = dns_server {
        server.shutdown().await;
    }
    supervisor.stop();
    let (done_tx, done_rx) = oneshot::channel();
    let _ = tx.send(WriterMsg::Shutdown(done_tx));
    let _ = done_rx.await;
    let _ = writer.await;
    ExitCode::from(exit)
}

/// `[dns] listen` and `[metrics] listen` as written to `meta/dns_listen` and
/// `meta/metrics_listen`: IPv4 as `<ip>:<port>`, IPv6 as `[<ip>]:<port>`.
fn listen_keys(config: &Config) -> writer::ListenKeys {
    let strings = |l: &[std::net::SocketAddr]| l.iter().map(ToString::to_string).collect();
    writer::ListenKeys {
        dns: config.dns.as_ref().map(|d| strings(&d.listen)),
        metrics: config.metrics.as_ref().map(|m| strings(&m.listen)),
        peers: None,
    }
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
/// reload (`current` set), a changed `[store]` table, `[dns] listen`,
/// `[metrics] listen` or `[peers] listen` is an error too. Every error is returned together. The
/// caller commits or rolls back the returned plugins.
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
        let current_listen = current.metrics.as_ref().map(|m| m.listen.clone());
        if config::parse_metrics_listen(&text).is_some_and(|l| l != current_listen) {
            errors.push(ConfigError {
                path: "metrics.listen".into(),
                message: "metrics listen settings require restart".into(),
            });
        }
        let current_listen = current.peers.as_ref().map(|p| p.listen);
        if config::parse_peers_listen(&text).is_some_and(|l| l != current_listen) {
            errors.push(ConfigError {
                path: "peers.listen".into(),
                message: "peers listen settings require restart".into(),
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

    /// State-store "Backend state history": one value or key that is not
    /// UTF-8 is discarded on its own; the other services keep their
    /// history, and the bad value is loaded empty so the writer deletes it.
    #[test]
    fn history_loads_around_bytes_that_are_not_utf8() {
        use crate::store::tests::{cfg, put_raw};
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&cfg(dir.path())).unwrap();
        let good = r#"{"backends":{"10.0.0.1:80":[{"at":1,"from":"healthy","to":"unhealthy","reason":null}]}}"#;
        put_raw(&store, b"hist/a.svc.emfor", good.as_bytes());
        put_raw(&store, b"hist/b.svc.emfor", &[0xff, 0xfe]);
        put_raw(&store, b"hist/\xff", good.as_bytes());
        let loaded = load_history(&store);
        assert_eq!(
            loaded.keys().map(String::as_str).collect::<Vec<_>>(),
            ["a.svc.emfor", "b.svc.emfor"]
        );
        assert_eq!(loaded["a.svc.emfor"].backends["10.0.0.1:80"].len(), 1);
        assert!(loaded["b.svc.emfor"].is_empty());
    }

    /// History left by a newer daemon: transitions with a state this one
    /// does not know are dropped, the others kept.
    #[test]
    fn history_drops_states_it_does_not_know() {
        use crate::store::tests::{cfg, put_raw};
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&cfg(dir.path())).unwrap();
        let value = r#"{"backends":{"10.0.0.1:80":[{"at":1,"from":"healthy","to":"unhealthy","reason":null},{"at":2,"from":"unhealthy","to":"some-future-state","reason":null}],"10.0.0.2:80":[{"at":3,"from":"some-future-state","to":"healthy","reason":null}]}}"#;
        put_raw(&store, b"hist/a.svc.emfor", value.as_bytes());
        let loaded = load_history(&store);
        let h = &loaded["a.svc.emfor"];
        assert_eq!(
            h.backends.keys().map(String::as_str).collect::<Vec<_>>(),
            ["10.0.0.1:80"]
        );
        assert_eq!(h.backends["10.0.0.1:80"].len(), 1);
        assert_eq!(h.backends["10.0.0.1:80"][0].to, HealthState::Unhealthy);
    }

    /// State-store "DNS listen addresses in the store" and "Metrics listen
    /// addresses in the store": the address forms written to the store.
    #[test]
    fn listen_key_address_forms() {
        let parse = |text: &str| {
            config::parse(text, &Registry::builtin()).unwrap_or_else(|e| panic!("errors: {e:?}"))
        };
        let c = parse(
            "[dns]\nlisten = [\"127.0.0.1:5300\", \"[::1]:5300\"]\n\n[metrics]\nlisten = [\"127.0.0.1:5301\", \"[::1]:5301\"]\n",
        );
        assert_eq!(
            listen_keys(&c),
            writer::ListenKeys {
                dns: Some(vec!["127.0.0.1:5300".into(), "[::1]:5300".into()]),
                metrics: Some(vec!["127.0.0.1:5301".into(), "[::1]:5301".into()]),
                peers: None,
            }
        );
        assert_eq!(listen_keys(&parse("")), writer::ListenKeys::default());
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
