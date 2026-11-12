//! Command line entry point: `emfor [--config <path>] [--check-config]`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{mpsc, oneshot, watch};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, reload};

use crate::checker::Registry;
use crate::config::{self, Config, ConfigError, DEFAULT_CONFIG_PATH, DnsConfig, in_zone};
use crate::dns::{self, DnsSettings};
use crate::live::LiveState;
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
    let registry = Registry::builtin();
    let loaded = config::load(&cli.config, &registry);

    if cli.check_config {
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

    let config = match loaded {
        Ok(c) => c,
        Err(errors) => {
            init_logging("info");
            log_config_errors(&cli.config, &errors);
            return ExitCode::from(EXIT_CONFIG);
        }
    };
    let log = init_logging(&config.log.level);

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("cannot start async runtime: {e}");
            return ExitCode::from(EXIT_STARTUP);
        }
    };
    runtime.block_on(run(cli.config, config, registry, log))
}

type LogHandle = reload::Handle<EnvFilter, tracing_subscriber::Registry>;

/// Install logging. Returns a handle to change the level on reload, or `None`
/// when `RUST_LOG` is set (it then overrides `[log] level` for the whole run).
fn init_logging(level: &str) -> Option<LogHandle> {
    let from_env = EnvFilter::try_from_default_env().ok();
    let env_set = from_env.is_some();
    let (filter, handle) = reload::Layer::new(from_env.unwrap_or_else(|| EnvFilter::new(level)));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .try_init();
    (!env_set).then_some(handle)
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
    registry: Registry,
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
    warn_outside_zone(&config);

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
                match reload(&config_path, &current, &registry) {
                    Ok(new) => {
                        if new.log != current.log {
                            match &log {
                                Some(h) => match h.reload(EnvFilter::new(&new.log.level)) {
                                    Ok(()) => tracing::info!("log level set to {}", new.log.level),
                                    Err(e) => tracing::error!("cannot change log level: {e}"),
                                },
                                None => tracing::warn!("[log] level changed but RUST_LOG is set; keeping RUST_LOG"),
                            }
                        }
                        supervisor.reload(&new);
                        if let (Some(tx), Some(d)) = (&dns_tx, &new.dns) {
                            tx.send_replace(dns_settings(d));
                        }
                        warn_outside_zone(&new);
                        tracing::info!("config reloaded: {} services, {} backends", new.services.len(), new.backend_count());
                        current = new;
                    }
                    Err(errors) => {
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

/// Load the config again. A changed `[store]` table or `[dns] listen` is an
/// error, reported together with every other validation error.
fn reload(path: &Path, current: &Config, registry: &Registry) -> Result<Config, Vec<ConfigError>> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        vec![ConfigError {
            path: String::new(),
            message: format!("cannot read {}: {e}", path.display()),
        }]
    })?;
    let parsed = config::parse(&text, registry);
    let store_changed = config::parse_store(&text).is_some_and(|s| s != current.store);
    let current_listen = current.dns.as_ref().map(|d| d.listen.clone());
    let listen_changed = config::parse_dns_listen(&text).is_some_and(|l| l != current_listen);
    let mut restart_errors = Vec::new();
    if store_changed {
        restart_errors.push(ConfigError {
            path: "store".into(),
            message: "store settings require restart".into(),
        });
    }
    if listen_changed {
        restart_errors.push(ConfigError {
            path: "dns.listen".into(),
            message: "dns listen settings require restart".into(),
        });
    }
    match parsed {
        Ok(new) if restart_errors.is_empty() => Ok(new),
        Ok(_) => Err(restart_errors),
        Err(mut errors) => {
            errors.extend(restart_errors);
            Err(errors)
        }
    }
}
