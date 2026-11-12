//! `emfor status`: show what the daemon publishes in the state store.
//!
//! Reads the store read-only, the same way clients do, and never writes it:
//! no writer lock, and no `-lock` file is created when the store is missing.
//! Reading (I/O) is separate from presenting: [`Snapshot`] plus the current
//! time go through pure functions for the text, JSON, exit status and the
//! `--watch` comparison.

use std::collections::{HashSet, VecDeque};
use std::fmt::Write as _;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use emfor_format::{
    HealthState, KEY_HEARTBEAT, KEY_SCHEMA_VERSION, KEY_SITE, KEY_STARTED_AT, SCHEMA_VERSION,
    SERVICE_KEY_PREFIX, ServiceRecord,
};
use heed::types::Str;
use heed::{Database, Env, EnvFlags, EnvOpenOptions, WithoutTls};

use crate::config::{self, StoreConfig};
use crate::schedule::unix_ms;

/// A heartbeat older than this means the daemon is not updating the store;
/// the same as the clients' default `max_staleness`.
pub const STALE_AFTER_MS: u64 = 3000;
/// How many state changes `--watch` lists.
const WATCH_CHANGES: usize = 10;

const EXIT_PROBLEM: u8 = 1;
const EXIT_FAILURE: u8 = 2;

#[derive(Debug, clap::Args)]
pub struct StatusArgs {
    /// Show only this service.
    #[arg(long, value_name = "NAME")]
    pub service: Option<String>,
    /// Print one JSON object instead of text.
    #[arg(long, conflicts_with = "watch")]
    pub json: bool,
    /// Redraw every second until interrupted (needs a terminal).
    #[arg(long)]
    pub watch: bool,
}

/// What one read transaction found in the store.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// `meta/heartbeat`; 0 after a clean shutdown.
    pub heartbeat_ms: u64,
    pub started_at_ms: Option<u64>,
    pub site: Option<String>,
    /// In name order.
    pub services: Vec<ServiceRecord>,
}

/// Whether the daemon is updating the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Daemon {
    Running,
    Stale,
    Stopped,
}

impl Daemon {
    pub fn as_str(self) -> &'static str {
        match self {
            Daemon::Running => "running",
            Daemon::Stale => "stale",
            Daemon::Stopped => "stopped",
        }
    }
}

/// The daemon state and heartbeat age (`None` when stopped). A heartbeat in
/// the future (clock stepped back) has age 0.
pub fn daemon_state(heartbeat_ms: u64, now_ms: u64) -> (Daemon, Option<u64>) {
    if heartbeat_ms == 0 {
        return (Daemon::Stopped, None);
    }
    let age = now_ms.saturating_sub(heartbeat_ms);
    if age > STALE_AFTER_MS {
        (Daemon::Stale, Some(age))
    } else {
        (Daemon::Running, Some(age))
    }
}

/// A failure that ends `emfor status` with exit status 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure(pub String);

/// The state store, opened read-only.
pub struct Reader {
    env: Env<WithoutTls>,
    db: Database<Str, Str>,
}

impl Reader {
    pub fn open(cfg: &StoreConfig) -> Result<Reader, Failure> {
        let path = &cfg.path;
        let failed = |e: &dyn std::fmt::Display| {
            Failure(format!("cannot open store {}: {e}", path.display()))
        };
        // LMDB would create `<path>-lock` even when the data file is missing,
        // and a lock file owned by another user can keep the daemon from
        // starting; check first, like the clients.
        match std::fs::metadata(path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(failed(&"no such file"));
            }
            Err(e) => return Err(failed(&e)),
        }
        // SAFETY: opened read-only; only the daemon writes the file and
        // nothing truncates it.
        let env = unsafe {
            let mut o = EnvOpenOptions::new().read_txn_without_tls();
            o.map_size(cfg.map_size);
            o.flags(EnvFlags::READ_ONLY | EnvFlags::NO_SUB_DIR);
            o.open(path)
        }
        .map_err(|e| failed(&e))?;
        let rtxn = env.read_txn().map_err(|e| failed(&e))?;
        let db = env
            .open_database::<Str, Str>(&rtxn, None)
            .map_err(|e| failed(&e))?
            .ok_or_else(|| failed(&"no database"))?;
        rtxn.commit().map_err(|e| failed(&e))?;
        Ok(Reader { env, db })
    }

    /// Read the meta keys and every service record in one transaction.
    pub fn read(&self) -> Result<Snapshot, Failure> {
        let lmdb = |e: heed::Error| Failure(format!("cannot read store: {e}"));
        let rtxn = self.env.read_txn().map_err(lmdb)?;
        let get = |key: &str| self.db.get(&rtxn, key).map_err(lmdb);
        let number = |v: Option<&str>| v.and_then(|s| s.parse::<u64>().ok());

        match get(KEY_SCHEMA_VERSION)? {
            Some(v) if v == SCHEMA_VERSION.to_string() => {}
            v => {
                return Err(Failure(format!(
                    "store format version {}; this emfor reads version {SCHEMA_VERSION}",
                    v.unwrap_or("(missing)")
                )));
            }
        }
        let heartbeat_ms = number(get(KEY_HEARTBEAT)?).unwrap_or(0);
        let started_at_ms = number(get(KEY_STARTED_AT)?);
        let site = get(KEY_SITE)?.map(str::to_owned);
        let mut services = Vec::new();
        for item in self
            .db
            .prefix_iter(&rtxn, SERVICE_KEY_PREFIX)
            .map_err(lmdb)?
        {
            let (key, value) = item.map_err(lmdb)?;
            let record: ServiceRecord = serde_json::from_str(value)
                .map_err(|e| Failure(format!("cannot read {key}: {e}")))?;
            services.push(record);
        }
        services.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Snapshot {
            heartbeat_ms,
            started_at_ms,
            site,
            services,
        })
    }
}

/// Keep only the service named `name` (case and one trailing dot ignored).
pub fn filter(mut snap: Snapshot, name: Option<&str>) -> Result<Snapshot, Failure> {
    let Some(name) = name else {
        return Ok(snap);
    };
    let want = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
    snap.services.retain(|s| s.name == want);
    if snap.services.is_empty() {
        return Err(Failure(format!("service {name} not found")));
    }
    Ok(snap)
}

/// 1 when the daemon is stale or stopped or a shown service is all down,
/// otherwise 0.
pub fn exit_status(snap: &Snapshot, now_ms: u64) -> u8 {
    let (daemon, _) = daemon_state(snap.heartbeat_ms, now_ms);
    if daemon != Daemon::Running || snap.services.iter().any(|s| s.all_down) {
        EXIT_PROBLEM
    } else {
        0
    }
}

/// `0.8s`, `1m01s`, `3h12m`, `2d03h`.
pub fn age(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        format!("{s}.{}s", ms % 1000 / 100)
    } else if s < 3600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else if s < 86400 {
        format!("{}h{:02}m", s / 3600, s % 3600 / 60)
    } else {
        format!("{}d{:02}h", s / 86400, s % 86400 / 3600)
    }
}

/// `1.2ms` below one second, `1.4s` from there; rounded to one decimal.
pub fn latency(us: u64) -> String {
    let tenth_ms = (us + 50) / 100;
    if tenth_ms < 10_000 {
        format!("{}.{}ms", tenth_ms / 10, tenth_ms % 10)
    } else {
        let tenth_s = (us + 50_000) / 100_000;
        format!("{}.{}s", tenth_s / 10, tenth_s % 10)
    }
}

/// A row of the text output, for `--watch` highlighting.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Row {
    Service(String),
    Backend(String, String),
}

/// How text output is styled.
#[derive(Debug, Clone, Copy, Default)]
pub struct Style {
    /// Color states (terminal without `NO_COLOR`).
    pub color: bool,
}

const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const BOLD_RED: &str = "\x1b[1;31m";
const REVERSE: &str = "\x1b[7m";
const RESET: &str = "\x1b[0m";

fn paint(style: Style, code: &str, text: &str) -> String {
    if style.color {
        format!("{code}{text}{RESET}")
    } else {
        text.to_owned()
    }
}

/// Show `line` in reverse video, also after any color reset inside it.
fn reverse(line: &str) -> String {
    format!(
        "{REVERSE}{}{RESET}",
        line.replace(RESET, &format!("{RESET}{REVERSE}"))
    )
}

/// The text output, one string per line. Rows in `marked` are shown in
/// reverse video.
pub fn text(
    snap: &Snapshot,
    store: &Path,
    now_ms: u64,
    style: Style,
    marked: &HashSet<Row>,
) -> Vec<String> {
    let mut lines = vec![format!("store: {}", store.display())];
    let (daemon, hb_age) = daemon_state(snap.heartbeat_ms, now_ms);
    let mut line = format!("daemon: {}", daemon.as_str());
    if let Some(a) = hb_age {
        write!(line, ", heartbeat {} ago", age(a)).unwrap();
    }
    if daemon == Daemon::Running {
        if let Some(started) = snap.started_at_ms {
            write!(line, ", up {}", age(now_ms.saturating_sub(started))).unwrap();
        }
        if let Some(site) = &snap.site {
            write!(line, ", site {site}").unwrap();
        }
        lines.push(line);
    } else {
        lines.push(paint(style, RED, &line));
    }

    // Backend cells: id, state, checked, latency, pool; reason unpadded.
    let rows: Vec<Vec<[String; 5]>> = snap
        .services
        .iter()
        .map(|svc| {
            svc.backends
                .iter()
                .map(|b| {
                    let checked = match b.checked_at {
                        Some(t) => format!("{} ago", age(now_ms.saturating_sub(t))),
                        None => "never".to_owned(),
                    };
                    let in_pool = svc
                        .pool
                        .as_ref()
                        .is_some_and(|p| p.iter().any(|id| id == &b.id));
                    [
                        b.id.clone(),
                        b.state.as_str().to_owned(),
                        checked,
                        b.latency_us.map_or_else(|| "-".to_owned(), latency),
                        if in_pool {
                            "pool".into()
                        } else {
                            String::new()
                        },
                    ]
                })
                .collect()
        })
        .collect();
    let mut width = [0usize; 5];
    for cells in rows.iter().flatten() {
        for (w, c) in width.iter_mut().zip(cells) {
            *w = (*w).max(c.len());
        }
    }
    let show_pool = width[4] > 0;

    for (svc, backends) in snap.services.iter().zip(&rows) {
        lines.push(String::new());
        let mut head = format!(
            "{}  {}  on_all_down={}",
            svc.name,
            svc.balance.as_str(),
            match svc.on_all_down {
                emfor_format::OnAllDown::Open => "open",
                emfor_format::OnAllDown::Closed => "closed",
            }
        );
        if svc.all_down {
            head.push_str("  ");
            head.push_str(&paint(style, BOLD_RED, "ALL DOWN"));
        }
        if marked.contains(&Row::Service(svc.name.clone())) {
            head = reverse(&head);
        }
        lines.push(head);
        for (backend, cells) in svc.backends.iter().zip(backends) {
            let mut parts = Vec::new();
            for (i, c) in cells.iter().enumerate() {
                if i == 4 && !show_pool {
                    continue;
                }
                let padded = format!("{c:<w$}", w = width[i]);
                parts.push(if i == 1 {
                    match backend.state {
                        HealthState::Healthy => padded,
                        HealthState::Unhealthy => paint(style, RED, &padded),
                        HealthState::Unknown => paint(style, YELLOW, &padded),
                    }
                } else {
                    padded
                });
            }
            if let Some(r) = &backend.reason {
                parts.push(r.clone());
            }
            let mut row = format!("  {}", parts.join("  ").trim_end());
            if marked.contains(&Row::Backend(svc.name.clone(), backend.id.clone())) {
                row = reverse(&row);
            }
            lines.push(row);
        }
    }
    lines
}

/// The `--json` object.
pub fn json(snap: &Snapshot, store: &Path, now_ms: u64) -> serde_json::Value {
    let (daemon, hb_age) = daemon_state(snap.heartbeat_ms, now_ms);
    serde_json::json!({
        "store": store.display().to_string(),
        "daemon": daemon.as_str(),
        "heartbeat_ms": snap.heartbeat_ms,
        "heartbeat_age_ms": hb_age,
        "started_at_ms": snap.started_at_ms,
        "site": snap.site,
        "services": snap.services,
    })
}

/// What `--watch` remembers between frames.
#[derive(Debug, Default)]
pub struct Watch {
    prev: Option<(Daemon, Snapshot)>,
    changes: VecDeque<String>,
}

impl Watch {
    /// Compare `snap` with the previous frame: returns the rows to
    /// highlight and records health state changes (and daemon state
    /// changes), labelled with `clock`.
    pub fn update(&mut self, snap: &Snapshot, now_ms: u64, clock: &str) -> HashSet<Row> {
        let mut marked = HashSet::new();
        let (daemon, _) = daemon_state(snap.heartbeat_ms, now_ms);
        if let Some((prev_daemon, prev)) = self.prev.take() {
            if prev_daemon != daemon {
                self.push(format!(
                    "{clock} daemon {} -> {}",
                    prev_daemon.as_str(),
                    daemon.as_str()
                ));
            }
            for svc in &snap.services {
                let Some(old) = prev.services.iter().find(|s| s.name == svc.name) else {
                    continue;
                };
                if old.all_down != svc.all_down {
                    marked.insert(Row::Service(svc.name.clone()));
                }
                for b in &svc.backends {
                    let Some(ob) = old.backends.iter().find(|o| o.id == b.id) else {
                        continue;
                    };
                    if ob.state != b.state || ob.reason != b.reason {
                        marked.insert(Row::Backend(svc.name.clone(), b.id.clone()));
                    }
                    if ob.state != b.state {
                        self.push(format!(
                            "{clock} {} {} {} -> {}",
                            svc.name, b.id, ob.state, b.state
                        ));
                    }
                }
            }
        }
        self.prev = Some((daemon, snap.clone()));
        marked
    }

    fn push(&mut self, line: String) {
        self.changes.push_front(line);
        self.changes.truncate(WATCH_CHANGES);
    }

    /// Recorded changes, newest first.
    pub fn changes(&self) -> impl Iterator<Item = &str> {
        self.changes.iter().map(String::as_str)
    }
}

/// `HH:MM:SS` in local time.
fn clock(now_ms: u64) -> String {
    let t = (now_ms / 1000) as libc::time_t;
    // SAFETY: localtime_r only writes into `tm`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        let s = now_ms / 1000 % 86400;
        return format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60);
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

fn style_for_terminal() -> Style {
    Style {
        color: std::env::var_os("NO_COLOR").is_none(),
    }
}

/// Run `emfor status`.
pub fn main(config_path: &Path, args: &StatusArgs) -> ExitCode {
    let store = match config::load_store(config_path) {
        Ok(s) => s,
        Err(errors) => {
            for e in &errors {
                eprintln!("{e}");
            }
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    if args.watch {
        return watch(&store, args.service.as_deref());
    }
    let snap = match Reader::open(&store)
        .and_then(|r| r.read())
        .and_then(|s| filter(s, args.service.as_deref()))
    {
        Ok(s) => s,
        Err(Failure(message)) => {
            eprintln!("{message}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    let now = unix_ms();
    let mut out = std::io::stdout().lock();
    let written = if args.json {
        writeln!(out, "{:#}", json(&snap, &store.path, now))
    } else {
        let style = if out.is_terminal() {
            style_for_terminal()
        } else {
            Style::default()
        };
        text(&snap, &store.path, now, style, &HashSet::new())
            .iter()
            .try_for_each(|l| writeln!(out, "{l}"))
    };
    if written.is_err() {
        return ExitCode::from(EXIT_FAILURE);
    }
    ExitCode::from(exit_status(&snap, now))
}

/// `--watch`: redraw every second until Ctrl-C.
fn watch(store: &StoreConfig, service: Option<&str>) -> ExitCode {
    if !std::io::stdout().is_terminal() {
        eprintln!("--watch needs a terminal");
        return ExitCode::from(EXIT_FAILURE);
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot start async runtime: {e}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    let style = style_for_terminal();
    runtime.block_on(async {
        let mut out = std::io::stdout();
        // Alternate screen, hidden cursor.
        let _ = write!(out, "\x1b[?1049h\x1b[?25l");
        let mut state = Watch::default();
        let mut reader: Result<Reader, Failure> = Err(Failure(String::new()));
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);
        loop {
            tokio::select! {
                _ = &mut ctrl_c => break,
                _ = tick.tick() => {}
            }
            let now = unix_ms();
            if reader.is_err() {
                reader = Reader::open(store);
            }
            let read = reader.as_ref().map_err(Clone::clone).and_then(Reader::read);
            if read.is_err() {
                // Open the store afresh next time, in case it was replaced.
                reader = Err(Failure(String::new()));
            }
            let read = read.and_then(|s| filter(s, service));
            let mut lines = match &read {
                Ok(snap) => {
                    let marked = state.update(snap, now, &clock(now));
                    text(snap, &store.path, now, style, &marked)
                }
                Err(Failure(message)) => vec![
                    format!("store: {}", store.path.display()),
                    paint(style, RED, &format!("error: {message}")),
                ],
            };
            let changes: Vec<&str> = state.changes().collect();
            if !changes.is_empty() {
                lines.push(String::new());
                lines.push("recent changes:".to_owned());
                lines.extend(changes.iter().map(|c| format!("  {c}")));
            }
            let mut frame = String::from("\x1b[H");
            for l in &lines {
                frame.push_str(l);
                frame.push_str("\x1b[K\r\n");
            }
            frame.push_str("\x1b[J");
            let _ = out.write_all(frame.as_bytes());
            let _ = out.flush();
        }
        let _ = write!(out, "\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
    });
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: u64 = 1_800_000_000_000;

    fn backend(
        id: &str,
        state: &str,
        checked_ago: Option<u64>,
        latency: Option<u64>,
        reason: Option<&str>,
    ) -> serde_json::Value {
        let (ip, port) = id.rsplit_once(':').unwrap();
        json!({
            "id": id, "ip": ip, "port": port.parse::<u16>().unwrap(), "weight": 1,
            "state": state, "checked_at": checked_ago.map(|a| NOW - a),
            "reason": reason, "latency_us": latency,
        })
    }

    fn service(name: &str, all_down: bool, backends: Vec<serde_json::Value>) -> ServiceRecord {
        serde_json::from_value(json!({
            "name": name, "on_all_down": "open", "balance": "round-robin",
            "all_down": all_down, "backends": backends,
        }))
        .unwrap()
    }

    fn api() -> ServiceRecord {
        service(
            "api.svc.emfor",
            false,
            vec![
                backend("10.0.0.11:8080", "healthy", Some(800), Some(1200), None),
                backend(
                    "10.0.0.12:8080",
                    "unhealthy",
                    Some(900),
                    None,
                    Some("connect: Connection refused"),
                ),
            ],
        )
    }

    fn snap(heartbeat_ago: Option<u64>, services: Vec<ServiceRecord>) -> Snapshot {
        Snapshot {
            heartbeat_ms: heartbeat_ago.map_or(0, |a| NOW - a),
            started_at_ms: Some(NOW - 3_600_000),
            site: Some("tpe1".into()),
            services,
        }
    }

    fn render(s: &Snapshot) -> Vec<String> {
        text(
            s,
            Path::new("/var/lib/emfor/state.mdb"),
            NOW,
            Style::default(),
            &HashSet::new(),
        )
    }

    #[test]
    fn age_and_latency_formatting() {
        for (ms, want) in [
            (800, "0.8s"),
            (61_000, "1m01s"),
            ((3 * 3600 + 12 * 60) * 1000, "3h12m"),
            ((2 * 86400 + 3 * 3600) * 1000, "2d03h"),
        ] {
            assert_eq!(age(ms), want);
        }
        for (us, want) in [(350, "0.4ms"), (1200, "1.2ms"), (1_400_000, "1.4s")] {
            assert_eq!(latency(us), want);
        }
    }

    #[test]
    fn daemon_states() {
        assert_eq!(daemon_state(0, NOW), (Daemon::Stopped, None));
        assert_eq!(daemon_state(NOW - 400, NOW), (Daemon::Running, Some(400)));
        assert_eq!(daemon_state(NOW - 3000, NOW), (Daemon::Running, Some(3000)));
        assert_eq!(daemon_state(NOW - 5000, NOW), (Daemon::Stale, Some(5000)));
        // Clock stepped back: heartbeat in the future.
        assert_eq!(daemon_state(NOW + 900, NOW), (Daemon::Running, Some(0)));
    }

    #[test]
    fn one_healthy_and_one_failing_backend() {
        let lines = render(&snap(Some(400), vec![api()]));
        assert_eq!(
            lines,
            [
                "store: /var/lib/emfor/state.mdb",
                "daemon: running, heartbeat 0.4s ago, up 1h00m, site tpe1",
                "",
                "api.svc.emfor  round-robin  on_all_down=open",
                "  10.0.0.11:8080  healthy    0.8s ago  1.2ms",
                "  10.0.0.12:8080  unhealthy  0.9s ago  -      connect: Connection refused",
            ]
        );
        assert!(lines.iter().all(|l| !l.contains('\x1b')));
    }

    #[test]
    fn stopped_stale_and_all_down() {
        let db = service(
            "db.svc.emfor",
            true,
            vec![backend(
                "10.0.3.1:5432",
                "unhealthy",
                None,
                None,
                Some("auth: 28P01: bad"),
            )],
        );
        let lines = render(&snap(None, vec![api(), db.clone()]));
        assert_eq!(lines[1], "daemon: stopped");
        assert!(lines.contains(&"api.svc.emfor  round-robin  on_all_down=open".to_owned()));
        assert!(
            lines.contains(&"db.svc.emfor  round-robin  on_all_down=open  ALL DOWN".to_owned())
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("10.0.3.1:5432") && l.contains("never"))
        );

        let stale = render(&snap(Some(5000), vec![db]));
        assert_eq!(stale[1], "daemon: stale, heartbeat 5.0s ago");
    }

    #[test]
    fn fastest_pool_members_are_marked() {
        let mut svc = api();
        svc.balance = emfor_format::Balance::Fastest;
        svc.pool = Some(vec!["10.0.0.11:8080".into()]);
        let lines = render(&snap(Some(100), vec![svc]));
        assert!(lines[3].starts_with("api.svc.emfor  fastest"));
        assert!(lines[4].ends_with("1.2ms  pool"), "{:?}", lines[4]);
        assert!(!lines[5].contains("pool"));
    }

    #[test]
    fn colors_only_when_asked() {
        let s = snap(Some(100), vec![api()]);
        let plain = render(&s);
        let colored = text(
            &s,
            Path::new("/s"),
            NOW,
            Style { color: true },
            &HashSet::new(),
        );
        assert!(plain.iter().all(|l| !l.contains('\x1b')));
        assert!(colored.iter().any(|l| l.contains("\x1b[31munhealthy")));
    }

    #[test]
    fn exit_statuses() {
        let db_down = service(
            "db.svc.emfor",
            true,
            vec![backend("10.0.3.1:5432", "unhealthy", Some(100), None, None)],
        );
        let cases = [
            (snap(Some(100), vec![api()]), 0),
            (snap(Some(100), vec![api(), db_down.clone()]), 1),
            (
                filter(
                    snap(Some(100), vec![api(), db_down.clone()]),
                    Some("api.svc.emfor"),
                )
                .unwrap(),
                0,
            ),
            (snap(Some(5000), vec![api()]), 1),
            (snap(None, vec![api()]), 1),
        ];
        for (s, want) in cases {
            assert_eq!(exit_status(&s, NOW), want, "{s:?}");
        }
    }

    #[test]
    fn filter_by_service() {
        let s = snap(Some(100), vec![api()]);
        assert_eq!(
            filter(s.clone(), Some("API.svc.emfor."))
                .unwrap()
                .services
                .len(),
            1
        );
        assert_eq!(
            filter(s, Some("nope.svc.emfor")),
            Err(Failure("service nope.svc.emfor not found".into()))
        );
    }

    #[test]
    fn json_fields() {
        let v = json(&snap(Some(400), vec![api()]), Path::new("/s.mdb"), NOW);
        assert_eq!(v["store"], "/s.mdb");
        assert_eq!(v["daemon"], "running");
        assert_eq!(v["heartbeat_age_ms"], 400);
        assert_eq!(v["started_at_ms"], NOW - 3_600_000);
        assert_eq!(v["site"], "tpe1");
        assert_eq!(v["services"][0]["name"], "api.svc.emfor");
        assert_eq!(
            v["services"][0]["backends"][1]["reason"],
            "connect: Connection refused"
        );
        let stopped = json(&snap(None, vec![]), Path::new("/s.mdb"), NOW);
        assert_eq!(stopped["daemon"], "stopped");
        assert!(stopped["heartbeat_age_ms"].is_null());
    }

    #[test]
    fn watch_marks_changes_and_lists_transitions() {
        let mut w = Watch::default();
        let before = snap(Some(100), vec![api()]);
        assert!(w.update(&before, NOW, "14:03:11").is_empty());

        // Only ages move: nothing to highlight.
        let mut later = before.clone();
        later.services[0].backends[0].checked_at = Some(NOW);
        later.services[0].backends[0].latency_us = Some(1500);
        assert!(w.update(&later, NOW, "14:03:12").is_empty());

        let mut down = later.clone();
        let b = &mut down.services[0].backends[0];
        b.state = HealthState::Unhealthy;
        b.reason = Some("connect: Connection refused".into());
        down.services[0].all_down = true;
        let marked = w.update(&down, NOW, "14:03:12");
        assert_eq!(
            marked,
            HashSet::from([
                Row::Service("api.svc.emfor".into()),
                Row::Backend("api.svc.emfor".into(), "10.0.0.11:8080".into()),
            ])
        );
        assert_eq!(
            w.changes().next(),
            Some("14:03:12 api.svc.emfor 10.0.0.11:8080 healthy -> unhealthy")
        );
        let lines = text(&down, Path::new("/s"), NOW, Style::default(), &marked);
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with(REVERSE) && l.contains("10.0.0.11:8080"))
        );
        // With colors, the whole row stays reversed after the colored state.
        let colored = text(&down, Path::new("/s"), NOW, Style { color: true }, &marked);
        let reversed: Vec<_> = colored.iter().filter(|l| l.starts_with(REVERSE)).collect();
        assert_eq!(reversed.len(), 2);
        for l in reversed {
            let body = &l[..l.len() - RESET.len()];
            let resumed = format!("{RESET}{REVERSE}");
            assert_eq!(
                body.matches(RESET).count(),
                body.matches(&resumed).count(),
                "reverse video ends early: {l:?}"
            );
        }

        // The daemon stops: a daemon change, newest first.
        let mut stopped = down.clone();
        stopped.heartbeat_ms = 0;
        w.update(&stopped, NOW, "14:03:13");
        assert_eq!(
            w.changes().next(),
            Some("14:03:13 daemon running -> stopped")
        );
    }

    #[test]
    fn watch_keeps_the_last_ten_changes() {
        let mut w = Watch::default();
        let mut s = snap(Some(100), vec![api()]);
        w.update(&s, NOW, "00:00:00");
        for i in 0..12 {
            let b = &mut s.services[0].backends[0];
            b.state = if i % 2 == 0 {
                HealthState::Unhealthy
            } else {
                HealthState::Healthy
            };
            w.update(&s, NOW, &format!("00:00:{i:02}"));
        }
        let changes: Vec<_> = w.changes().collect();
        assert_eq!(changes.len(), 10);
        assert!(changes[0].starts_with("00:00:11 "), "{changes:?}");
        assert!(changes[9].starts_with("00:00:02 "), "{changes:?}");
    }
}
