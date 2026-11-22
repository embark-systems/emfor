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

use crate::config::{
    self, Config, ConfigError, DEFAULT_CONFIG_PATH, DnsConfig, StoreConfig, in_zone,
};
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
    CheckConfig {
        /// Print the merged config (the main file and its `include_dir`
        /// files) as TOML on standard output, each value followed by the file
        /// that set it. The usual `config ok` lines go to standard error.
        /// Built-in defaults are not added and nothing is redacted.
        #[arg(long)]
        show: bool,
    },
    /// Show the health state the running daemon publishes, then exit.
    ///
    /// Exits 1 when the daemon is not updating the store or a shown service
    /// has no healthy backend, 2 when the store cannot be read.
    Status(crate::status::StatusArgs),
    /// Print the backend an application would get for a service, and with
    /// `--explain` why.
    ///
    /// Exits 0 with an answer, 1 when no backend can be chosen, 2 when the
    /// service does not exist or the command line is invalid, 3 when the
    /// store cannot be used.
    Lookup(crate::lookup::LookupArgs),
}

pub fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.check_config {
        eprintln!("--check-config was replaced by \"emfor check-config\"");
        return ExitCode::from(EXIT_CONFIG);
    }
    let (check_only, show) = match &cli.command {
        None => {
            // Never start a daemon by accident: only `server` does.
            use clap::CommandFactory;
            let _ = Cli::command().write_help(&mut std::io::stderr());
            return ExitCode::from(EXIT_CONFIG);
        }
        Some(Command::Status(args)) => return crate::status::main(&cli.config, args),
        Some(Command::Lookup(args)) => return crate::lookup::main(&cli.config, args),
        Some(Command::CheckConfig { show }) => (true, *show),
        Some(Command::Server) => (false, false),
    };
    // Loading a config starts plugins, so the runtime comes first.
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot start async runtime: {e}");
            return ExitCode::from(EXIT_STARTUP);
        }
    };
    let code = runtime.block_on(start(cli.config, check_only, show));
    // A reload abandoned at shutdown can leave a config validation running
    // on a blocking thread, waiting up to 5s per plugin; do not wait for it.
    runtime.shutdown_timeout(Duration::from_secs(1));
    code
}

async fn start(config_path: PathBuf, check_only: bool, show: bool) -> ExitCode {
    if check_only {
        let mut plugins = PluginSet::default();
        let (prepared, loaded, read) = load(&config_path, None, &plugins).await;
        // Nothing keeps running after a config check.
        plugins.commit(prepared).await;
        plugins.stop_all().await;
        // No log subscriber here: the warnings go to stderr.
        for w in &read.warnings {
            eprintln!("{w}");
        }
        return match loaded {
            Ok(c) => {
                for w in credential_warnings(&c) {
                    eprintln!("{w}");
                }
                for w in store_mode_warnings(&c.store) {
                    eprintln!("{w}");
                }
                let ok = format!(
                    "config ok: {} services, {} backends",
                    c.services.len(),
                    c.backend_count()
                );
                let files: Vec<String> = read
                    .files
                    .iter()
                    .map(|f| format!("read {}", f.display()))
                    .collect();
                match read.layers.as_deref().filter(|_| show) {
                    // The merged TOML alone on standard output, so that it
                    // can be saved and checked again.
                    Some(layers) => {
                        eprintln!("{ok}");
                        for f in &files {
                            eprintln!("{f}");
                        }
                        if let Err(code) = write_stdout(&config::show_layers(layers), EXIT_STARTUP)
                        {
                            return code;
                        }
                    }
                    None => {
                        let mut text = format!("{ok}\n");
                        for f in &files {
                            text.push_str(f);
                            text.push('\n');
                        }
                        if let Err(code) = write_stdout(&text, EXIT_STARTUP) {
                            return code;
                        }
                    }
                }
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
    let Some((prepared, loaded, read)) = outcome else {
        tracing::info!("stop requested while starting; exiting");
        return ExitCode::from(EXIT_OK);
    };
    log_read_warnings(&read);
    let config = match loaded {
        Ok(c) => c,
        Err(errors) => {
            prepared.rollback().await;
            log_config_errors(&config_path, &errors);
            return ExitCode::from(EXIT_CONFIG);
        }
    };
    plugins.commit(prepared).await;
    log_files_read(&read);
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

/// Write `text` to standard output. The reader closing the pipe early
/// (`| head`) is not a failure, as for `emfor status`; any other write error
/// is reported and gives `fail`. `println!` would panic and exit with 101.
pub(crate) fn write_stdout(text: &str, fail: u8) -> Result<(), ExitCode> {
    use std::io::Write;
    match std::io::stdout().lock().write_all(text.as_bytes()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => {
            eprintln!("cannot write the output: {e}");
            Err(ExitCode::from(fail))
        }
    }
}

/// What `load` read besides the main file: the files, only when
/// `include_dir` is set (empty otherwise), and the warnings about names it
/// skipped.
#[derive(Default)]
struct ReadFiles {
    files: Vec<PathBuf>,
    warnings: Vec<String>,
    /// The merged config, for `check-config --show`.
    layers: Option<std::sync::Arc<config::Layers>>,
}

/// Logged whether the load succeeded or not: a skipped file can be the reason
/// it failed.
fn log_read_warnings(read: &ReadFiles) {
    for w in &read.warnings {
        tracing::warn!("{w}");
    }
}

fn log_files_read(read: &ReadFiles) {
    for f in &read.files {
        tracing::info!("config file read: {}", f.display());
    }
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
        rate_limit: d.rate_limit,
    };
    let mut dns_server = None;
    let mut dns_tx = None;
    if let Some(d) = &config.dns {
        let (stx, srx) = watch::channel(dns_settings(d));
        let handler = dns::Handler::new(live.clone(), srx);
        live.set_dns(Some(handler.limiter()));
        for m in listen_warnings(d) {
            tracing::warn!("{m}");
        }
        match dns::serve(&d.listen, handler).await {
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
    warn_metrics(&config);
    warn_credentials(&config);
    // Only here: `[store]` is applied at start, a reload does not change it.
    for m in store_mode_warnings(&config.store) {
        tracing::warn!("{m}");
    }
    // Kept until the daemon exits; dropping it stops the server.
    let _metrics_server = match &config.metrics {
        Some(m) => match metrics::serve(&m.listen, m.allow.as_deref(), live.clone()).await {
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
                &config.store.path,
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
                let Some((prepared, loaded, read)) = outcome else {
                    tracing::info!("stop requested while reloading; the reload is abandoned");
                    break;
                };
                log_read_warnings(&read);
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
                        warn_metrics(&new);
                        warn_credentials(&new);
                        if let (Some(h), Some(p)) = (&peers, &new.peers) {
                            h.reload(p, &peer_site(&new));
                        }
                        tracing::info!("config reloaded: {} services, {} backends", new.services.len(), new.backend_count());
                        log_files_read(&read);
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

/// Warnings for the `[metrics]` table of a loaded config.
fn warn_metrics(config: &Config) {
    if let Some(m) = &config.metrics {
        for w in metrics_warnings(m) {
            tracing::warn!("{w}");
        }
    }
}

/// One message per `[metrics] listen` address that is not a loopback
/// address, unless `allow` limits the sources.
fn metrics_warnings(m: &config::MetricsConfig) -> Vec<String> {
    if m.allow.is_some() {
        return Vec::new();
    }
    m.listen
        .iter()
        .filter(|a| !a.ip().to_canonical().is_loopback())
        .map(|a| {
            format!(
                "metrics listens on {a}, which is not a loopback address: every host that can reach it can read every service name, backend address and check failure reason; limit who can reach it with allow or a firewall"
            )
        })
        .collect()
}

/// One message per `[dns] listen` address that is not a loopback address.
fn listen_warnings(d: &DnsConfig) -> Vec<String> {
    d.listen
        .iter()
        .filter(|a| !a.ip().to_canonical().is_loopback())
        .map(|a| {
            format!(
                "DNS listens on {a}, which is not a loopback address: every host that can reach it can look up every service, and it answers UDP queries from other hosts at up to [dns] rate_limit responses a second per client network; limit who can reach it with a firewall"
            )
        })
        .collect()
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

/// One `warn` line for each warning of [`credential_warnings`].
fn warn_credentials(config: &Config) {
    for m in credential_warnings(config) {
        tracing::warn!("{m}");
    }
}

/// One warning for each of `[store] lock_file_mode` and `file_mode` that
/// lets other users write (`0002`). Never an error. Each line says what the
/// mode allows, what to set instead, where to read more, and that the value
/// reaches the existing files at the next start, because the reader of
/// `emfor check-config` is about to change the config.
fn store_mode_warnings(store: &StoreConfig) -> Vec<String> {
    const WRITE_BY_OTHERS: u32 = 0o002;
    let mut out = Vec::new();
    for (option, mode, can, instead) in [
        (
            "file_mode",
            store.file_mode,
            "write the data file that the emfor clients read: any local user can change what every lookup returns",
            "0640",
        ),
        (
            "lock_file_mode",
            store.lock_file_mode,
            "write the lock file, which holds LMDB's reader table and the lock the daemon takes for every commit: any local user can stop the daemon's commits, and every client then sees a stale store",
            "0660",
        ),
    ] {
        if mode & WRITE_BY_OTHERS == 0 {
            continue;
        }
        out.push(format!(
            "[store] {option} = \"{mode:04o}\" lets every user on this host {can}. Use \"{instead}\" (the default) and put the users of your applications in the group of the emfor daemon. The daemon sets this mode on the existing files at every start, so a change takes effect at the next start. See \"Who can use the store\" in the operations chapter of the manual",
        ));
    }
    out
}

/// The protocol check of `emfor-check` that the plugin called `name` runs:
/// the first argument of its command, when the command's program is
/// `emfor-check`. Not the plugin's name: a plugin can be called anything.
fn emfor_check_kind<'a>(config: &'a Config, name: &str) -> Option<&'a str> {
    let spec = config.plugins.iter().find(|p| p.name == name)?;
    let program = Path::new(spec.command.first()?).file_name()?;
    if program != "emfor-check" {
        return None;
    }
    spec.command.get(1).map(String::as_str)
}

/// The `check_options` that name a file holding a secret.
const SECRET_FILE_KEYS: [&str; 3] = ["password_file", "secret_file", "tls_key_file"];

/// Warnings about how the checks' credentials are kept. Never an error: a
/// config that works keeps working.
///
/// - A secret file (`password_file`, `secret_file`, `tls_key_file` in a
///   service's `check_options`, for built-in checks and plugins alike) that
///   other users can read, or that group or others can write. One message
///   for each file, naming the services that use it.
/// - A check of kind `redis`, `ldap`, `mysql`, `pgsql` or `amqp` with a
///   `password_file` and no `tls` (for `ldap`, neither `tls` nor
///   `starttls`): the password is not protected on the network. The kind is
///   the first argument of the plugin's command when the plugin runs
///   `emfor-check`, whatever the plugin is called; a plugin that does not run
///   `emfor-check` is taken for `redis` or `ldap` by its name alone, as it
///   always was. `mongodb` is not listed: its SCRAM does not send the
///   password.
///
/// A file that cannot be stat'ed is left to the check, which reports it.
fn credential_warnings(config: &Config) -> Vec<String> {
    use std::os::unix::fs::MetadataExt;

    let mut out = Vec::new();
    // (key, path) -> services, in the order of the config.
    let mut files: BTreeMap<(&str, PathBuf), Vec<&str>> = BTreeMap::new();
    for svc in &config.services {
        for key in SECRET_FILE_KEYS {
            if let Some(toml::Value::String(p)) = svc.check_options.get(key) {
                files
                    .entry((key, PathBuf::from(p)))
                    .or_default()
                    .push(&svc.name);
            }
        }
        let is_on = |k: &str| svc.check_options.get(k) == Some(&toml::Value::Boolean(true));
        let from_emfor_check = emfor_check_kind(config, &svc.check);
        let kind = from_emfor_check.unwrap_or(svc.check.as_str());
        if !svc.check_options.contains_key("password_file") || is_on("tls") {
            continue;
        }
        match kind {
            // These two were always taken from the name alone.
            "redis" | "ldap" => {
                if kind == "ldap" && is_on("starttls") {
                    continue;
                }
                out.push(format!(
                    "service {}: check_options has a password_file but {}: the password is sent in clear text, and anyone who can see the traffic can read it",
                    svc.name,
                    if kind == "ldap" {
                        "neither tls nor starttls is true"
                    } else {
                        "tls is not true"
                    },
                ));
            }
            "mysql" | "pgsql" | "amqp" if from_emfor_check.is_some() => {
                out.push(format!(
                    "service {}: check_options has a password_file but tls is not true: the password is not protected on the network, and anyone who can see the traffic, or answer in the backend's place, can get it",
                    svc.name,
                ));
            }
            _ => {}
        }
    }
    for ((key, path), services) in files {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let mode = meta.mode() & 0o777;
        let problem = if mode & 0o007 != 0 {
            "can be read by every user on this host"
        } else if mode & 0o022 != 0 {
            "can be changed by users other than its owner"
        } else {
            continue;
        };
        let shown: Vec<&str> = services.iter().take(3).copied().collect();
        let more = services.len() - shown.len();
        out.push(format!(
            "{key} {} (mode {mode:04o}) {problem}; used by {}{}. Limit it to its owner and the daemon's group, for example chmod 0640",
            path.display(),
            shown.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            },
        ));
    }
    out
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

/// Load a config: read the main file and its `include_dir` fragments, start
/// the plugins (reusing running ones whose command is unchanged), then
/// validate the merged config with them on a blocking thread. On a reload
/// (`current` set), a changed `[store]` table, `[dns] listen`,
/// `[metrics] listen` or `[peers] listen` is an error too, wherever in the
/// files it is set. Every error is returned together. The caller commits or
/// rolls back the returned plugins. The third value lists the files read,
/// only when `include_dir` is set.
async fn load(
    path: &Path,
    current: Option<&Config>,
    plugins: &PluginSet,
) -> (Prepared, Result<Config, Vec<ConfigError>>, ReadFiles) {
    // The read is blocking file I/O (a hung file system never returns), so it
    // runs on a blocking thread: a stop request can then end the reload.
    let (read_result, warnings) = {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let mut warnings = Vec::new();
            let result = config::read_layers_with(&path, &mut warnings);
            (result, warnings)
        })
        .await
        .unwrap_or_else(|e| {
            (
                Err(vec![ConfigError::new(
                    "",
                    format!("config read failed: {e}"),
                )]),
                Vec::new(),
            )
        })
    };
    let layers = match read_result {
        Ok(l) => l,
        Err(errors) => {
            let read = ReadFiles {
                files: Vec::new(),
                warnings,
                layers: None,
            };
            return (Prepared::default(), Err(errors), read);
        }
    };
    let read = ReadFiles {
        files: if layers.included {
            layers.files.clone()
        } else {
            Vec::new()
        },
        warnings,
        layers: None,
    };
    let (specs, _) = config::plugins_in(&layers.table);
    let mut prepared = plugins.prepare(&specs).await;
    let registry = prepared.registry(tokio::runtime::Handle::current(), current);
    let layers = std::sync::Arc::new(layers);
    let parsed = {
        let layers = layers.clone();
        tokio::task::spawn_blocking(move || config::parse_layers(&layers, &registry))
            .await
            .unwrap_or_else(|e| {
                Err(vec![ConfigError::new(
                    "",
                    format!("config validation failed: {e}"),
                )])
            })
    };
    let mut errors = std::mem::take(&mut prepared.errors);
    if let Some(current) = current {
        let table = &layers.table;
        if config::store_in(table).is_some_and(|s| s != current.store) {
            errors.push(ConfigError::new("store", "store settings require restart"));
        }
        let current_listen = current.dns.as_ref().map(|d| d.listen.clone());
        if config::dns_listen_in(table).is_some_and(|l| l != current_listen) {
            errors.push(ConfigError::new(
                "dns.listen",
                "dns listen settings require restart",
            ));
        }
        let current_listen = current.metrics.as_ref().map(|m| m.listen.clone());
        if config::metrics_listen_in(table).is_some_and(|l| l != current_listen) {
            errors.push(ConfigError::new(
                "metrics.listen",
                "metrics listen settings require restart",
            ));
        }
        if let (Ok(new), Some(old)) = (&parsed, &current.metrics)
            && new.metrics.as_ref().is_some_and(|m| m.allow != old.allow)
        {
            errors.push(ConfigError::new(
                "metrics.allow",
                "metrics allow settings require restart",
            ));
        }
        let current_listen = current.peers.as_ref().map(|p| p.listen);
        if config::peers_listen_in(table).is_some_and(|l| l != current_listen) {
            errors.push(ConfigError::new(
                "peers.listen",
                "peers listen settings require restart",
            ));
        }
    }
    for e in &mut errors {
        e.file = config::origin_of(&layers.origins, &e.path);
    }
    let result = match parsed {
        Ok(new) if errors.is_empty() => Ok(new),
        Ok(_) => Err(errors),
        Err(mut parse_errors) => {
            parse_errors.extend(errors);
            Err(parse_errors)
        }
    };
    (
        prepared,
        result,
        ReadFiles {
            layers: Some(layers),
            ..read
        },
    )
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

    /// A stand-in for the redis and ldap plugins: any options are valid.
    struct FakePlugin(&'static str);

    impl crate::checker::CheckerFactory for FakePlugin {
        fn kind(&self) -> &str {
            self.0
        }
        fn validate(
            &self,
            _options: &toml::Table,
        ) -> Result<std::sync::Arc<dyn crate::checker::Checker>, Vec<crate::checker::OptionError>>
        {
            crate::checker::tcp::TcpFactory.validate(&toml::Table::new())
        }
    }

    fn plugin_registry() -> Registry {
        let mut r = Registry::builtin();
        r.register(std::sync::Arc::new(FakePlugin("redis")));
        r.register(std::sync::Arc::new(FakePlugin("ldap")));
        for name in ["mysql", "pgsql", "amqp", "mongodb", "redis-check"] {
            r.register(std::sync::Arc::new(FakePlugin(name)));
        }
        r
    }

    /// A config with one redis service per `(name, extra check_options)`,
    /// all using the password file `pass`.
    fn credential_config(pass: &Path, services: &[(&str, &str)]) -> Config {
        let mut text = String::new();
        for (name, extra) in services {
            text.push_str(&format!(
                "[services.\"{name}\"]\ncheck = \"redis\"\nbackends = [\"10.0.0.1:6379\"]\n\
                 [services.\"{name}\".check_options]\npassword_file = \"{}\"\n{extra}\n",
                pass.display()
            ));
        }
        config::parse(&text, &plugin_registry()).unwrap_or_else(|e| panic!("errors: {e:?}"))
    }

    fn pass_file(dir: &tempfile::TempDir, mode: u32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.path().join("pass");
        std::fs::write(&path, "secret\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    /// A secret file other users can read is reported once, with the
    /// services that use it; owner and group access alone is not.
    #[test]
    fn secret_file_readable_by_others_is_reported_once() {
        let dir = tempfile::tempdir().unwrap();
        let pass = pass_file(&dir, 0o644);
        let tls = "tls = true";
        let w = credential_warnings(&credential_config(&pass, &[("a.svc", tls), ("b.svc", tls)]));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("password_file"), "{w:?}");
        assert!(w[0].contains("0644"), "{w:?}");
        assert!(w[0].contains("read by every user"), "{w:?}");
        assert!(w[0].contains("a.svc, b.svc"), "{w:?}");
        // Not the file's contents.
        assert!(
            !w[0].contains("secret\n") && !w[0].contains(": secret"),
            "{w:?}"
        );
        for ok in [0o600, 0o640, 0o440] {
            let pass = pass_file(&dir, ok);
            let w = credential_warnings(&credential_config(&pass, &[("a.svc", tls)]));
            assert!(w.is_empty(), "mode {ok:o}: {w:?}");
        }
    }

    /// Group or other write access is its own message.
    #[test]
    fn secret_file_writable_by_others_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let pass = pass_file(&dir, 0o660);
        let w = credential_warnings(&credential_config(&pass, &[("a.svc", "tls = true")]));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("0660") && w[0].contains("changed by users other than"),
            "{w:?}"
        );
    }

    /// A file that is not there is the check's to report, not a warning.
    #[test]
    fn missing_secret_file_is_not_a_warning_here() {
        let dir = tempfile::tempdir().unwrap();
        let pass = pass_file(&dir, 0o600);
        let config = credential_config(&pass, &[("a.svc", "tls = true")]);
        std::fs::remove_file(&pass).unwrap();
        assert!(credential_warnings(&config).is_empty());
    }

    /// A redis password without TLS is reported for each service; with `tls`
    /// it is not.
    #[test]
    fn redis_password_without_tls_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let pass = pass_file(&dir, 0o600);
        let w = credential_warnings(&credential_config(
            &pass,
            &[
                ("a.svc", ""),
                ("b.svc", "tls = false"),
                ("c.svc", "tls = true"),
            ],
        ));
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(
            w[0].starts_with("service a.svc:") && w[0].contains("clear text"),
            "{w:?}"
        );
        assert!(w[1].starts_with("service b.svc:"), "{w:?}");
    }

    /// A config with one service that runs the check `check`, which is the
    /// plugin `command` (a plugin declaration with that command) when given,
    /// with a password file and the options `extra`.
    fn plugin_credential_config(
        pass: &Path,
        check: &str,
        command: Option<&str>,
        extra: &str,
    ) -> Config {
        let plugin = command.map_or(String::new(), |c| {
            format!("[plugins.\"{check}\"]\ncommand = {c}\n")
        });
        let text = format!(
            "{plugin}[services.\"a.svc\"]\ncheck = \"{check}\"\nbackends = [\"10.0.0.1:5432\"]\n\
             [services.\"a.svc\".check_options]\npassword_file = \"{}\"\n{extra}\n",
            pass.display()
        );
        config::parse(&text, &plugin_registry()).unwrap_or_else(|e| panic!("errors: {e:?}"))
    }

    /// An executable file called `name` in `dir` (the config checks that a
    /// plugin's command can be run); its path.
    fn fake_program(dir: &tempfile::TempDir, name: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.path().join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    /// Spec-less change emfor-plaintext-credential-warning: a password file
    /// without TLS is reported for the protocols of `emfor-check` whose
    /// password is not protected without it, by the plugin's command and not
    /// by its name.
    #[test]
    fn password_without_tls_is_reported_for_every_protocol_check() {
        let dir = tempfile::tempdir().unwrap();
        let pass = pass_file(&dir, 0o600);
        let emfor_check = fake_program(&dir, "emfor-check");
        for kind in ["mysql", "pgsql", "amqp", "redis", "ldap"] {
            let command = format!("[\"{emfor_check}\", \"{kind}\"]");
            let w = credential_warnings(&plugin_credential_config(&pass, kind, Some(&command), ""));
            assert_eq!(w.len(), 1, "{kind}: {w:?}");
            assert!(
                w[0].starts_with("service a.svc:") && w[0].contains("password"),
                "{kind}: {w:?}"
            );
            let w = credential_warnings(&plugin_credential_config(
                &pass,
                kind,
                Some(&command),
                "tls = true",
            ));
            assert!(w.is_empty(), "{kind} with tls: {w:?}");
        }
    }

    /// A plugin called something else still runs `emfor-check redis`.
    #[test]
    fn renamed_plugin_is_known_by_its_command() {
        let dir = tempfile::tempdir().unwrap();
        let pass = pass_file(&dir, 0o600);
        let command = format!("[\"{}\", \"redis\"]", fake_program(&dir, "emfor-check"));
        let w = credential_warnings(&plugin_credential_config(
            &pass,
            "redis-check",
            Some(&command),
            "",
        ));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("clear text"), "{w:?}");
    }

    /// `mongodb` (SCRAM sends no password), and a plugin called `pgsql` that
    /// is not `emfor-check`, are not reported.
    #[test]
    fn other_checks_are_not_reported() {
        let dir = tempfile::tempdir().unwrap();
        let pass = pass_file(&dir, 0o600);
        let mongo = format!("[\"{}\", \"mongodb\"]", fake_program(&dir, "emfor-check"));
        let other = format!("[\"{}\"]", fake_program(&dir, "other-plugin"));
        let w = credential_warnings(&plugin_credential_config(
            &pass,
            "mongodb",
            Some(&mongo),
            "",
        ));
        assert!(w.is_empty(), "{w:?}");
        let w = credential_warnings(&plugin_credential_config(&pass, "pgsql", Some(&other), ""));
        assert!(w.is_empty(), "{w:?}");
    }

    /// A plugin that is not `emfor-check` but is named `redis` or `ldap` is
    /// still reported by its name, as before the kind was read from the
    /// command.
    #[test]
    fn plugin_named_redis_that_is_not_emfor_check_is_still_reported() {
        let dir = tempfile::tempdir().unwrap();
        let pass = pass_file(&dir, 0o600);
        let other = format!("[\"{}\"]", fake_program(&dir, "other-plugin"));
        let w = credential_warnings(&plugin_credential_config(&pass, "redis", Some(&other), ""));
        assert_eq!(w.len(), 1, "{w:?}");
    }

    /// The table "which modes warn" of the spec: a mode that lets other
    /// users write warns, and each line carries the four things that let the
    /// reader of `emfor check-config` act on it.
    // spectra-scenario: state-store / Daemon starts with a world-writable mode / which modes warn
    #[test]
    fn store_mode_warnings_follow_the_other_users_write_bit() {
        // (file_mode, lock_file_mode, [(option, mode shown, recommended)])
        type Warned = &'static [(&'static str, &'static str, &'static str)];
        let rows: [(u32, u32, Warned); 6] = [
            (0o640, 0o660, &[]),
            (0o644, 0o660, &[]),
            (0o644, 0o666, &[("lock_file_mode", "0666", "0660")]),
            (0o666, 0o660, &[("file_mode", "0666", "0640")]),
            (
                0o666,
                0o666,
                &[
                    ("file_mode", "0666", "0640"),
                    ("lock_file_mode", "0666", "0660"),
                ],
            ),
            (0o662, 0o640, &[("file_mode", "0662", "0640")]),
        ];
        for (file_mode, lock_file_mode, expected) in rows {
            let store = StoreConfig {
                file_mode,
                lock_file_mode,
                ..StoreConfig::default()
            };
            let w = store_mode_warnings(&store);
            assert_eq!(
                w.len(),
                expected.len(),
                "{file_mode:o} {lock_file_mode:o}: {w:?}"
            );
            for (line, (option, shown, recommended)) in w.iter().zip(expected) {
                let at = format!("{file_mode:o} {lock_file_mode:o}: {line}");
                // The option and its mode.
                assert!(
                    line.starts_with(&format!("[store] {option} = \"{shown}\"")),
                    "{at}"
                );
                // What the mode allows.
                let can = if *option == "lock_file_mode" {
                    "stop the daemon's commits"
                } else {
                    "change what every lookup returns"
                };
                assert!(line.contains(can), "{at}");
                // What to set instead, and the group.
                assert!(line.contains(&format!("Use \"{recommended}\"")), "{at}");
                assert!(line.contains("in the group of the emfor daemon"), "{at}");
                // Where to read more, and when it takes effect.
                assert!(line.contains("\"Who can use the store\""), "{at}");
                assert!(line.contains("at the next start"), "{at}");
                // One line.
                assert!(!line.contains('\n'), "{at}");
            }
        }
        // The default settings do not warn, and the mode bit for other
        // users to read alone does not either.
        assert!(store_mode_warnings(&StoreConfig::default()).is_empty());
    }

    /// ldap is protected by `tls` or by `starttls`.
    #[test]
    fn ldap_password_needs_tls_or_starttls() {
        let dir = tempfile::tempdir().unwrap();
        let pass = pass_file(&dir, 0o600);
        let text = |name: &str, extra: &str| {
            format!(
                "[services.\"{name}\"]\ncheck = \"ldap\"\nbackends = [\"10.0.0.1:389\"]\n\
                 [services.\"{name}\".check_options]\npassword_file = \"{}\"\n{extra}\n",
                pass.display()
            )
        };
        let all = [
            text("a.svc", ""),
            text("b.svc", "starttls = true"),
            text("c.svc", "tls = true"),
        ]
        .join("\n");
        let config = config::parse(&all, &plugin_registry()).unwrap();
        let w = credential_warnings(&config);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].starts_with("service a.svc:") && w[0].contains("neither tls nor starttls"),
            "{w:?}"
        );
    }

    /// A config with no secret files and no passwords has no warnings.
    #[test]
    fn no_credentials_no_warnings() {
        assert!(warnings(false, "a.svc", &["10.0.0.1:80"]).is_empty());
        let config = config::parse(
            "[services.\"a.svc\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n",
            &Registry::builtin(),
        )
        .unwrap();
        assert!(credential_warnings(&config).is_empty());
    }

    /// Spec dns-server "Rate limit": a warning for each listen address
    /// that is not loopback, none for loopback ones.
    // spectra-scenario: dns-server / Listen beyond loopback
    #[test]
    fn non_loopback_listen_is_warned_about() {
        let d = DnsConfig {
            listen: vec![
                "127.0.0.1:5300".parse().unwrap(),
                "[::1]:5300".parse().unwrap(),
                "10.0.0.5:53".parse().unwrap(),
                "0.0.0.0:5300".parse().unwrap(),
            ],
            ..DnsConfig::default()
        };
        let w = listen_warnings(&d);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(
            w[0].contains("10.0.0.5:53") && w[0].contains("not a loopback address"),
            "{w:?}"
        );
        assert!(w[1].contains("0.0.0.0:5300"), "{w:?}");
        assert!(listen_warnings(&DnsConfig::default()).is_empty());
    }

    // spectra-scenario: metrics-endpoint / Warning for a non-loopback address without allow
    #[test]
    fn non_loopback_metrics_listen_is_warned_about_unless_allow_is_set() {
        let mk = |listen: &[&str], allow: Option<&str>| config::MetricsConfig {
            listen: listen.iter().map(|a| a.parse().unwrap()).collect(),
            allow: allow.map(|a| vec![config::Cidr::parse(a).unwrap()]),
        };
        let w = metrics_warnings(&mk(&["127.0.0.1:5301", "10.0.0.5:5301"], None));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("10.0.0.5:5301") && w[0].contains("allow"),
            "{w:?}"
        );
        assert!(metrics_warnings(&mk(&["10.0.0.5:5301"], Some("10.0.0.0/8"))).is_empty());
        assert!(metrics_warnings(&mk(&["127.0.0.1:5301", "[::1]:5301"], None)).is_empty());
        assert!(metrics_warnings(&config::MetricsConfig::default()).is_empty());
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
    // spectra-scenario: state-store / Metrics table written at start
    // spectra-scenario: state-store / DNS table written at start
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
    // spectra-scenario: dns-server / Mixed ports in the zone / which services are reported
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
