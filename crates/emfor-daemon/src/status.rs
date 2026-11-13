//! `emfor status`: show what the daemon publishes in the state store.
//!
//! Reads the store read-only, the same way clients do, and never writes it:
//! no writer lock, and no `-lock` file is created when the store is missing.
//! Reading (I/O) is separate from presenting: [`Snapshot`] plus the current
//! time go through pure functions for the text, JSON, exit status and the
//! `--watch` comparison.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fmt::Write as _;

/// `write!` into a `String`. `fmt::Write` for `String` never returns an
/// error, so there is nothing to handle.
macro_rules! append {
    ($out:expr, $($arg:tt)*) => {{
        let _ = write!($out, $($arg)*);
    }};
}
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, terminal};
use emfor_format::{
    HIST_KEY_PREFIX, HealthState, HistoryEntry, KEY_DNS_LISTEN, KEY_HEARTBEAT, KEY_METRICS_LISTEN,
    KEY_PEERS, KEY_SCHEMA_VERSION, KEY_SITE, KEY_STARTED_AT, SCHEMA_VERSION, SERVICE_KEY_PREFIX,
    ServiceHistory, ServiceRecord,
};
use futures::StreamExt;
use heed::types::Str;
use heed::{Database, Env, EnvFlags, EnvOpenOptions, WithoutTls};
use serde::Deserialize;

use crate::config::{self, StoreConfig};
use crate::schedule::unix_ms;

/// A heartbeat older than this means the daemon is not updating the store;
/// the same as the clients' default `max_staleness`.
pub const STALE_AFTER_MS: u64 = 3000;
/// How many state changes `--watch` lists.
const WATCH_CHANGES: usize = 10;
/// A backend with at least this many healthy/unhealthy changes within
/// [`FLAP_WINDOW_MS`] is flapping.
pub const FLAP_THRESHOLD: usize = 5;
pub const FLAP_WINDOW_MS: u64 = 600_000;

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
    /// List each backend's recorded state changes, newest first.
    #[arg(long, conflicts_with_all = ["json", "watch"])]
    pub history: bool,
}

/// What one read transaction found in the store.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// `meta/heartbeat`; 0 after a clean shutdown.
    pub heartbeat_ms: u64,
    pub started_at_ms: Option<u64>,
    pub site: Option<String>,
    /// `meta/dns_listen`; `None` when absent or not an array of strings.
    pub dns_listen: Option<Vec<String>>,
    /// `meta/metrics_listen`; `None` when absent or not an array of strings.
    pub metrics_listen: Option<Vec<String>>,
    /// `meta/peers` when it holds a JSON object; shown by `--json` as is.
    pub peers: Option<serde_json::Value>,
    /// In name order.
    pub services: Vec<ServiceRecord>,
    /// Backend state history by service name; unreadable values are left out.
    pub history: BTreeMap<String, ServiceHistory>,
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
        let dns_listen = listen_value(get(KEY_DNS_LISTEN)?);
        let metrics_listen = listen_value(get(KEY_METRICS_LISTEN)?);
        let peers = peers_value(get(KEY_PEERS)?);
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
        let mut history = BTreeMap::new();
        for item in self.db.prefix_iter(&rtxn, HIST_KEY_PREFIX).map_err(lmdb)? {
            let (key, value) = item.map_err(lmdb)?;
            // Display only: a damaged history must not hide the state.
            if let Ok(h) = serde_json::from_str::<ServiceHistory>(value) {
                history.insert(key[HIST_KEY_PREFIX.len()..].to_owned(), h);
            }
        }
        // Only listed services: a key left behind (for example by an older
        // daemon, which does not prune history) is not shown.
        history.retain(|name, _| services.iter().any(|s| s.name == *name));
        Ok(Snapshot {
            heartbeat_ms,
            started_at_ms,
            site,
            dns_listen,
            metrics_listen,
            peers,
            services,
            history,
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
    snap.history.retain(|n, _| *n == want);
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

/// How many of `entries` go between healthy and unhealthy (either way) at
/// most [`FLAP_WINDOW_MS`] before `now_ms`.
pub fn flap_count(entries: &[HistoryEntry], now_ms: u64) -> usize {
    use HealthState::{Healthy, Unhealthy};
    entries
        .iter()
        .filter(|e| matches!((e.from, e.to), (Healthy, Unhealthy) | (Unhealthy, Healthy)))
        .filter(|e| now_ms.saturating_sub(e.at) <= FLAP_WINDOW_MS)
        .count()
}

/// The flapping backends of `snap` as (service, backend id, changes), in
/// service name order and then store order.
pub fn flapping(snap: &Snapshot, now_ms: u64) -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    for svc in &snap.services {
        let Some(h) = snap.history.get(&svc.name) else {
            continue;
        };
        for b in &svc.backends {
            let n = h.backends.get(&b.id).map_or(0, |e| flap_count(e, now_ms));
            if n >= FLAP_THRESHOLD {
                out.push((svc.name.clone(), b.id.clone(), n));
            }
        }
    }
    out
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

/// A `meta/dns_listen` or `meta/metrics_listen` value; display only, so
/// anything but a JSON array of strings reads as absent.
fn listen_value(value: Option<&str>) -> Option<Vec<String>> {
    value.and_then(|v| serde_json::from_str(v).ok())
}

/// A `meta/peers` value; anything but a JSON object reads as absent.
fn peers_value(value: Option<&str>) -> Option<serde_json::Value> {
    value
        .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok())
        .filter(serde_json::Value::is_object)
}

/// `meta/peers` as the text output reads it (spec state-store, "Peer group
/// state in the store").
#[derive(Debug, Deserialize)]
struct PeerState {
    members: Vec<PeerMember>,
    digest_mismatch: Vec<PeerMismatch>,
    disagreements: Vec<PeerDisagreement>,
}

#[derive(Debug, Deserialize)]
struct PeerMember {
    addr: String,
    live: bool,
}

#[derive(Debug, Deserialize)]
struct PeerMismatch {
    service: String,
    member: String,
}

#[derive(Debug, Deserialize)]
struct PeerDisagreement {
    service: String,
    backend: String,
}

/// The peer state to show: only while the daemon is running and when
/// `meta/peers` has the expected shape.
fn peer_state(snap: &Snapshot, now_ms: u64) -> Option<PeerState> {
    if daemon_state(snap.heartbeat_ms, now_ms).0 != Daemon::Running {
        return None;
    }
    serde_json::from_value(snap.peers.clone()?).ok()
}

/// The `peers:` lines (spec "Status text output").
fn peer_lines(state: &PeerState, snap: &Snapshot) -> Vec<String> {
    let live = state.members.iter().filter(|m| m.live).count();
    let mut line = format!("peers: {live}/{} live", state.members.len());
    let not_live: Vec<&str> = state
        .members
        .iter()
        .filter(|m| !m.live)
        .map(|m| m.addr.as_str())
        .collect();
    if !not_live.is_empty() {
        append!(line, ", not live {}", not_live.join(" "));
    }
    let mut lines = vec![line];
    for svc in &snap.services {
        let at: Vec<&str> = state
            .digest_mismatch
            .iter()
            .filter(|m| m.service == svc.name)
            .map(|m| m.member.as_str())
            .collect();
        if !at.is_empty() {
            lines.push(format!(
                "peers: check definition of {} differs at {}",
                svc.name,
                at.join(" ")
            ));
        }
    }
    lines
}

/// The `store:` and `daemon:` lines.
fn header(snap: &Snapshot, store: &Path, now_ms: u64, style: Style) -> Vec<String> {
    let mut lines = vec![format!("store: {}", store.display())];
    let (daemon, hb_age) = daemon_state(snap.heartbeat_ms, now_ms);
    let mut line = format!("daemon: {}", daemon.as_str());
    if let Some(a) = hb_age {
        append!(line, ", heartbeat {} ago", age(a));
    }
    if daemon == Daemon::Running {
        if let Some(started) = snap.started_at_ms {
            append!(line, ", up {}", age(now_ms.saturating_sub(started)));
        }
        if let Some(site) = &snap.site {
            append!(line, ", site {site}");
        }
        for (name, listen) in [("dns", &snap.dns_listen), ("metrics", &snap.metrics_listen)] {
            if let Some(listen) = listen {
                append!(line, ", {name} {}", listen.join(" "));
            }
        }
        lines.push(line);
    } else {
        lines.push(paint(style, RED, &line));
    }
    lines
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
    let mut lines = header(snap, store, now_ms, style);
    lines.extend(services_lines(snap, now_ms, style, marked));
    lines
}

/// The services part of [`text`]: everything after the `store:` and
/// `daemon:` lines.
fn services_lines(
    snap: &Snapshot,
    now_ms: u64,
    style: Style,
    marked: &HashSet<Row>,
) -> Vec<String> {
    let peers = peer_state(snap, now_ms);
    let mut lines = peers
        .as_ref()
        .map(|p| peer_lines(p, snap))
        .unwrap_or_default();
    let flaps = flapping(snap, now_ms);

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
            if peers.as_ref().is_some_and(|p| {
                p.disagreements
                    .iter()
                    .any(|d| d.service == svc.name && d.backend == backend.id)
            }) {
                parts.push(paint(style, YELLOW, "local-up"));
            }
            if let Some((.., n)) = flaps
                .iter()
                .find(|(name, id, _)| *name == svc.name && *id == backend.id)
            {
                let marker = format!("FLAPPING ({n} changes in 10m)");
                parts.push(paint(style, YELLOW, &marker));
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
        "dns_listen": snap.dns_listen,
        "metrics_listen": snap.metrics_listen,
        "services": snap.services,
        "history": snap.history,
        "peers": snap.peers,
        "flapping": flapping(snap, now_ms)
            .into_iter()
            .map(|(service, backend, changes)| {
                serde_json::json!({"service": service, "backend": backend, "changes": changes})
            })
            .collect::<Vec<_>>(),
    })
}

/// `--history`: the header, then each backend's changes newest first.
pub fn history_text(snap: &Snapshot, store: &Path, now_ms: u64, style: Style) -> Vec<String> {
    let mut lines = header(snap, store, now_ms, style);
    for svc in &snap.services {
        lines.push(String::new());
        lines.push(svc.name.clone());
        let h = snap.history.get(&svc.name);
        for b in &svc.backends {
            lines.push(format!("  {}", b.id));
            let entries = h
                .and_then(|h| h.backends.get(&b.id))
                .map_or(&[][..], Vec::as_slice);
            if entries.is_empty() {
                lines.push("    no changes recorded".to_owned());
            }
            for e in entries.iter().rev() {
                let mut l = format!("    {} {} -> {}", datetime(e.at), e.from, e.to);
                if let Some(r) = &e.reason {
                    append!(l, "  {r}");
                }
                lines.push(l);
            }
        }
    }
    lines
}

/// What `--watch` remembers between frames.
#[derive(Debug, Default)]
pub struct Watch {
    prev: Option<(Daemon, Snapshot)>,
    /// Daemon state changes seen during this run: (Unix ms, text), newest first.
    daemon_changes: VecDeque<(u64, String)>,
}

impl Watch {
    /// Compare `snap` with the previous frame: returns the rows to
    /// highlight and records daemon state changes at `now_ms`.
    pub fn update(&mut self, snap: &Snapshot, now_ms: u64) -> HashSet<Row> {
        let mut marked = HashSet::new();
        let (daemon, _) = daemon_state(snap.heartbeat_ms, now_ms);
        if let Some((prev_daemon, prev)) = self.prev.take() {
            if prev_daemon != daemon {
                let line = format!("daemon {} -> {}", prev_daemon.as_str(), daemon.as_str());
                self.daemon_changes.push_front((now_ms, line));
                self.daemon_changes.truncate(WATCH_CHANGES);
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
                }
            }
        }
        self.prev = Some((daemon, snap.clone()));
        marked
    }

    /// The most recent changes, newest first: backend changes from the
    /// stored history of the last snapshot read, merged with the daemon
    /// changes seen during this run. Changes from before the local day of
    /// `now_ms` carry the date.
    pub fn changes(&self, now_ms: u64) -> Vec<String> {
        let mut all: Vec<(u64, String)> = self.daemon_changes.iter().cloned().collect();
        if let Some((_, snap)) = &self.prev {
            for (name, h) in &snap.history {
                for (id, entries) in &h.backends {
                    all.extend(
                        entries
                            .iter()
                            .map(|e| (e.at, format!("{name} {id} {} -> {}", e.from, e.to))),
                    );
                }
            }
        }
        // Stable: equal times keep daemon changes first.
        all.sort_by_key(|a| std::cmp::Reverse(a.0));
        let today = local(now_ms).map(|t| t.0);
        all.into_iter()
            .take(WATCH_CHANGES)
            .map(|(at, line)| match local(at) {
                Some((date, time)) if Some(&date) != today.as_ref() => {
                    format!("{date} {time} {line}")
                }
                _ => format!("{} {line}", clock(at)),
            })
            .collect()
    }
}

/// What a key does in `--watch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    LineUp,
    LineDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Quit,
}

/// The `--watch` action of a key, or `None` for keys it ignores. Raw mode
/// turns Ctrl-C into a key instead of `SIGINT`, so it is mapped here.
pub fn key_action(key: &KeyEvent) -> Option<Action> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        KeyCode::PageUp => Action::PageUp,
        KeyCode::PageDown | KeyCode::Char(' ') => Action::PageDown,
        KeyCode::Up => Action::LineUp,
        KeyCode::Down => Action::LineDown,
        KeyCode::Home => Action::Top,
        KeyCode::End => Action::Bottom,
        KeyCode::Char('q' | 'Q') => Action::Quit,
        KeyCode::Char('c' | 'C') if ctrl => Action::Quit,
        _ => return None,
    })
}

/// The `--watch` scroll position: the index of the first content line
/// shown below the fixed header.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Scroll {
    pub offset: usize,
}

impl Scroll {
    /// Keep the position within `content_len` lines shown `page` at a time.
    pub fn clamp(&mut self, content_len: usize, page: usize) {
        self.offset = self.offset.min(content_len.saturating_sub(page));
    }

    /// Move for `action`, then clamp. `Quit` leaves the position alone.
    pub fn apply(&mut self, action: Action, content_len: usize, page: usize) {
        let page = page.max(1);
        self.offset = match action {
            Action::LineUp => self.offset.saturating_sub(1),
            Action::LineDown => self.offset.saturating_add(1),
            Action::PageUp => self.offset.saturating_sub(page),
            Action::PageDown => self.offset.saturating_add(page),
            Action::Top => 0,
            Action::Bottom => usize::MAX,
            Action::Quit => self.offset,
        };
        self.clamp(content_len, page);
    }
}

/// How many content lines fit below `header_len` header lines in a
/// terminal of `rows` rows: all rows left when the content fits, else one
/// row less for the footer, and never less than one.
pub fn page_height(header_len: usize, content_len: usize, rows: usize) -> usize {
    let left = rows.saturating_sub(header_len);
    if content_len <= left {
        left.max(1)
    } else {
        left.saturating_sub(1).max(1)
    }
}

/// One `--watch` screen: the header, the part of `content` at the
/// (clamped) scroll position and, when the content does not fit, a footer
/// with the shown range. At most `rows` lines.
pub fn compose(
    header: &[String],
    content: &[String],
    rows: usize,
    scroll: &mut Scroll,
    style: Style,
) -> Vec<String> {
    let page = page_height(header.len(), content.len(), rows);
    scroll.clamp(content.len(), page);
    let mut lines = header.to_vec();
    let end = (scroll.offset + page).min(content.len());
    lines.extend_from_slice(&content[scroll.offset..end]);
    if header.len() + content.len() > rows {
        let footer = format!(
            "-- lines {}-{end} of {}  PgUp/PgDn Up/Down Home/End  q quit --",
            scroll.offset + 1,
            content.len()
        );
        lines.push(if style.color {
            reverse(&footer)
        } else {
            footer
        });
    }
    lines.truncate(rows.max(1));
    lines
}

/// Local (`YYYY-MM-DD`, `HH:MM:SS`) of Unix milliseconds.
fn local(ms: u64) -> Option<(String, String)> {
    let t = (ms / 1000) as libc::time_t;
    // SAFETY: localtime_r only writes into `tm`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return None;
    }
    Some((
        format!(
            "{:04}-{:02}-{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday
        ),
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec),
    ))
}

/// `HH:MM:SS` in local time (UTC when the local time is unknown).
fn clock(ms: u64) -> String {
    match local(ms) {
        Some((_, time)) => time,
        None => {
            let s = ms / 1000 % 86400;
            format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
        }
    }
}

/// `YYYY-MM-DD HH:MM:SS` in local time.
fn datetime(ms: u64) -> String {
    match local(ms) {
        Some((date, time)) => format!("{date} {time}"),
        None => format!("@{} {}", ms / 1000, clock(ms)),
    }
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
        let lines = if args.history {
            history_text(&snap, &store.path, now, style)
        } else {
            text(&snap, &store.path, now, style, &HashSet::new())
        };
        lines.iter().try_for_each(|l| writeln!(out, "{l}"))
    };
    if written.is_err() {
        return ExitCode::from(EXIT_FAILURE);
    }
    ExitCode::from(exit_status(&snap, now))
}

/// Give the terminal back: line wrapping on, cursor shown, main screen,
/// raw mode off. Safe to run more than once; errors are ignored because
/// there is nowhere left to report them.
fn restore_terminal() {
    let _ = crossterm::execute!(
        std::io::stdout(),
        terminal::EnableLineWrap,
        cursor::Show,
        terminal::LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
}

type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

/// The terminal set up for `--watch`: raw mode, alternate screen, hidden
/// cursor and no line wrapping, so that each line takes exactly one row.
/// Dropping it restores the terminal; while it lives, a panic restores the
/// terminal before the panic message is printed.
struct TerminalGuard {
    previous_hook: Arc<PanicHook>,
}

impl TerminalGuard {
    fn enter() -> std::io::Result<TerminalGuard> {
        terminal::enable_raw_mode()?;
        if let Err(e) = crossterm::execute!(
            std::io::stdout(),
            terminal::EnterAlternateScreen,
            cursor::Hide,
            terminal::DisableLineWrap
        ) {
            restore_terminal();
            return Err(e);
        }
        let previous_hook: Arc<PanicHook> = Arc::new(std::panic::take_hook());
        let hook = Arc::clone(&previous_hook);
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            hook(info);
        }));
        Ok(TerminalGuard { previous_hook })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
        // The hook cannot be changed while unwinding (that would abort);
        // it already restored the terminal then.
        if !std::thread::panicking() {
            let previous = Arc::clone(&self.previous_hook);
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
    }
}

/// Terminal height in rows; 24 when it cannot be told.
fn terminal_rows() -> usize {
    match terminal::size() {
        Ok((_, rows)) if rows > 0 => usize::from(rows),
        _ => 24,
    }
}

/// Write `lines` from the top-left corner, clearing what the previous
/// frame left behind. No newline after the last line, so a full screen
/// does not scroll.
fn draw(out: &mut impl Write, lines: &[String]) {
    let mut frame = String::from("\x1b[H");
    frame.push_str(&lines.join("\x1b[K\r\n"));
    frame.push_str("\x1b[K\x1b[J");
    let _ = out.write_all(frame.as_bytes());
    let _ = out.flush();
}

/// `--watch`: redraw every second and scroll with the keys until `q`,
/// Ctrl-C or a signal.
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
        // `kill` and a closed ssh session (SIGHUP) end the watch and give
        // the terminal back; in raw mode Ctrl-C arrives as a key instead
        // of SIGINT, but `kill -INT` still works.
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut int), Ok(mut term), Ok(mut hup)) = (
            signal(SignalKind::interrupt()),
            signal(SignalKind::terminate()),
            signal(SignalKind::hangup()),
        ) else {
            eprintln!("cannot install signal handlers");
            return ExitCode::from(EXIT_FAILURE);
        };
        let guard = match TerminalGuard::enter() {
            Ok(g) => g,
            Err(e) => {
                eprintln!("cannot use the terminal: {e}");
                return ExitCode::from(EXIT_FAILURE);
            }
        };
        let mut out = std::io::stdout();
        let mut events = EventStream::new();
        let mut events_open = true;
        let mut state = Watch::default();
        let mut reader: Result<Reader, Failure> = Err(Failure(String::new()));
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut rows = terminal_rows();
        let mut scroll = Scroll::default();
        // The last frame read from the store; keys and resizes redraw it
        // without reading again, so highlights stay for one refresh.
        let mut top: Vec<String> = Vec::new();
        let mut content: Vec<String> = Vec::new();
        loop {
            tokio::select! {
                _ = int.recv() => break,
                _ = term.recv() => break,
                _ = hup.recv() => break,
                _ = tick.tick() => {
                    rows = terminal_rows();
                    (top, content) = read_frame(store, service, &mut reader, &mut state, style);
                }
                event = events.next(), if events_open => match event {
                    Some(Ok(Event::Key(key))) => match key_action(&key) {
                        Some(Action::Quit) => break,
                        Some(action) => {
                            let page = page_height(top.len(), content.len(), rows);
                            scroll.apply(action, content.len(), page);
                        }
                        None => continue,
                    },
                    Some(Ok(Event::Resize(_, r))) => rows = usize::from(r.max(1)),
                    Some(Ok(_)) => continue,
                    // No more keys; keep refreshing, signals still end it.
                    Some(Err(_)) | None => {
                        events_open = false;
                        continue;
                    }
                },
            }
            draw(&mut out, &compose(&top, &content, rows, &mut scroll, style));
        }
        drop(guard);
        ExitCode::SUCCESS
    })
}

/// Read the store for one `--watch` refresh: the fixed top lines and the
/// scrollable content (services, then the recent changes).
fn read_frame(
    store: &StoreConfig,
    service: Option<&str>,
    reader: &mut Result<Reader, Failure>,
    state: &mut Watch,
    style: Style,
) -> (Vec<String>, Vec<String>) {
    let now = unix_ms();
    if reader.is_err() {
        *reader = Reader::open(store);
    }
    let read = reader.as_ref().map_err(Clone::clone).and_then(Reader::read);
    if read.is_err() {
        // Open the store afresh next time, in case it was replaced.
        *reader = Err(Failure(String::new()));
    }
    let (top, mut content) = match read.and_then(|s| filter(s, service)) {
        Ok(snap) => {
            let marked = state.update(&snap, now);
            (
                header(&snap, &store.path, now, style),
                services_lines(&snap, now, style, &marked),
            )
        }
        Err(Failure(message)) => (
            vec![format!("store: {}", store.path.display())],
            vec![paint(style, RED, &format!("error: {message}"))],
        ),
    };
    let changes = state.changes(now);
    if !changes.is_empty() {
        content.push(String::new());
        content.push("recent changes:".to_owned());
        content.extend(changes.iter().map(|c| format!("  {c}")));
    }
    (top, content)
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
            dns_listen: None,
            metrics_listen: None,
            peers: None,
            services,
            history: BTreeMap::new(),
        }
    }

    use HealthState::{Healthy, Unhealthy, Unknown};

    fn entry(ago_ms: u64, from: HealthState, to: HealthState) -> HistoryEntry {
        HistoryEntry {
            at: NOW - ago_ms,
            from,
            to,
            reason: (to == Unhealthy).then(|| "connect: Connection refused".to_owned()),
        }
    }

    /// Alternating healthy/unhealthy entries at the given ages, oldest first.
    fn alternating(ages_ms: &[u64]) -> Vec<HistoryEntry> {
        let mut ages = ages_ms.to_vec();
        ages.sort_unstable_by(|a, b| b.cmp(a));
        ages.iter()
            .enumerate()
            .map(|(i, &a)| {
                if i % 2 == 0 {
                    entry(a, Healthy, Unhealthy)
                } else {
                    entry(a, Unhealthy, Healthy)
                }
            })
            .collect()
    }

    fn with_history(mut s: Snapshot, svc: &str, id: &str, entries: Vec<HistoryEntry>) -> Snapshot {
        s.history
            .entry(svc.to_owned())
            .or_default()
            .backends
            .insert(id.to_owned(), entries);
        s
    }

    const MIN: u64 = 60_000;

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

    /// Spec "Status text output", example "metrics part of the daemon
    /// line", and scenario "Metrics addresses on the daemon line".
    #[test]
    fn metrics_on_the_daemon_line() {
        let rows: [(Option<&str>, Option<u64>, &str); 5] = [
            (
                Some(r#"["127.0.0.1:5301"]"#),
                Some(400),
                ", site tpe1, metrics 127.0.0.1:5301",
            ),
            (
                Some(r#"["127.0.0.1:5301","[::1]:5301"]"#),
                Some(400),
                ", site tpe1, metrics 127.0.0.1:5301 [::1]:5301",
            ),
            (None, Some(400), ", site tpe1"),
            (Some("not json"), Some(400), ", site tpe1"),
            (Some(r#"["127.0.0.1:5301"]"#), None, "daemon: stopped"),
        ];
        for (value, heartbeat_ago, end) in rows {
            let mut s = snap(heartbeat_ago, vec![]);
            s.metrics_listen = listen_value(value);
            let line = &render(&s)[1];
            assert!(line.ends_with(end), "{value:?}: {line}");
        }
        let mut s = snap(Some(400), vec![]);
        s.started_at_ms = Some(NOW - (3 * 60 + 12) * 60_000);
        s.metrics_listen = Some(vec!["127.0.0.1:5301".into(), "[::1]:5301".into()]);
        assert_eq!(
            render(&s)[1],
            "daemon: running, heartbeat 0.4s ago, up 3h12m, site tpe1, metrics 127.0.0.1:5301 [::1]:5301"
        );
        assert_eq!(listen_value(Some("[1]")), None);
    }

    /// Spec "Status text output", example "DNS part of the daemon line", and
    /// scenario "DNS and metrics addresses on the daemon line".
    #[test]
    fn dns_on_the_daemon_line() {
        let rows: [(Option<&str>, Option<u64>, Option<&str>); 5] = [
            (
                Some(r#"["127.0.0.1:5300"]"#),
                Some(400),
                Some(", dns 127.0.0.1:5300"),
            ),
            (
                Some(r#"["127.0.0.1:5300","[::1]:5300"]"#),
                Some(400),
                Some(", dns 127.0.0.1:5300 [::1]:5300"),
            ),
            (None, Some(400), None),
            (Some("not json"), Some(400), None),
            (Some(r#"["127.0.0.1:5300"]"#), None, None),
        ];
        for (value, heartbeat_ago, want) in rows {
            let mut s = snap(heartbeat_ago, vec![]);
            s.dns_listen = listen_value(value);
            let line = &render(&s)[1];
            match want {
                Some(w) => assert!(line.ends_with(w), "{value:?}: {line}"),
                None => assert!(!line.contains("dns"), "{value:?}: {line}"),
            }
        }
        let mut s = snap(Some(400), vec![]);
        s.started_at_ms = Some(NOW - (3 * 60 + 12) * 60_000);
        s.dns_listen = Some(vec!["127.0.0.1:5300".into()]);
        s.metrics_listen = Some(vec!["127.0.0.1:5301".into()]);
        assert_eq!(
            render(&s)[1],
            "daemon: running, heartbeat 0.4s ago, up 3h12m, site tpe1, dns 127.0.0.1:5300, metrics 127.0.0.1:5301"
        );
    }

    /// Spec "Status filtering and JSON output", scenario "JSON DNS
    /// addresses".
    #[test]
    fn json_dns_listen() {
        let mut s = snap(Some(400), vec![]);
        assert_eq!(
            json(&s, Path::new("/s.mdb"), NOW)["dns_listen"],
            serde_json::Value::Null
        );
        s.dns_listen = Some(vec!["127.0.0.1:5300".into()]);
        assert_eq!(
            json(&s, Path::new("/s.mdb"), NOW)["dns_listen"],
            serde_json::json!(["127.0.0.1:5300"])
        );
    }

    const PEERS: &str = r#"{"listen":"10.0.0.5:5302","members":[{"addr":"10.0.0.6:5302","live":true},{"addr":"10.0.0.7:5302","live":false},{"addr":"10.0.0.8:5302","live":true}],"digest_mismatch":[{"service":"api.svc.emfor","member":"10.0.0.8:5302"}],"disagreements":[{"service":"api.svc.emfor","backend":"10.0.0.11:8080","down":2,"fresh":3}]}"#;

    /// Spec "Status text output", scenario "Peer group lines".
    #[test]
    fn peer_group_lines() {
        let mut s = snap(Some(400), vec![api()]);
        s.peers = peers_value(Some(PEERS));
        let lines = render(&s);
        assert_eq!(lines[2], "peers: 2/3 live, not live 10.0.0.7:5302");
        assert_eq!(
            lines[3],
            "peers: check definition of api.svc.emfor differs at 10.0.0.8:5302"
        );
        let b11 = lines.iter().find(|l| l.contains("10.0.0.11:8080")).unwrap();
        assert!(b11.ends_with("1.2ms  local-up"), "{b11:?}");
        let b12 = lines.iter().find(|l| l.contains("10.0.0.12:8080")).unwrap();
        assert!(!b12.contains("local-up"));
        // A service not shown has no mismatch line.
        let mut only_db = snap(Some(400), vec![service("db.svc.emfor", false, vec![])]);
        only_db.peers = s.peers.clone();
        assert_eq!(
            render(&only_db)[2],
            "peers: 2/3 live, not live 10.0.0.7:5302"
        );
        assert!(!render(&only_db)[3].starts_with("peers:"));
    }

    /// Spec "Status text output", example "peers line".
    #[test]
    fn peers_line_table() {
        let both_live = r#"{"listen":"10.0.0.5:5302","members":[{"addr":"10.0.0.6:5302","live":true},{"addr":"10.0.0.7:5302","live":true}],"digest_mismatch":[],"disagreements":[]}"#;
        let none_live = both_live.replace("true", "false");
        let rows: [(Option<&str>, Option<u64>, Option<&str>); 5] = [
            (Some(both_live), Some(400), Some("peers: 2/2 live")),
            (
                Some(&none_live),
                Some(400),
                Some("peers: 0/2 live, not live 10.0.0.6:5302 10.0.0.7:5302"),
            ),
            (None, Some(400), None),
            (Some("not json"), Some(400), None),
            (Some(both_live), None, None),
        ];
        for (value, heartbeat_ago, want) in rows {
            let mut s = snap(heartbeat_ago, vec![api()]);
            s.peers = peers_value(value);
            let lines = render(&s);
            let got = lines
                .iter()
                .find(|l| l.starts_with("peers:"))
                .map(String::as_str);
            assert_eq!(got, want, "{value:?} {heartbeat_ago:?}");
        }
        // Stopped: no marker either.
        let mut s = snap(None, vec![api()]);
        s.peers = peers_value(Some(PEERS));
        assert!(render(&s).iter().all(|l| !l.contains("local-up")));
    }

    /// Spec "Status filtering and JSON output", scenario "JSON peer state".
    #[test]
    fn json_peer_state() {
        let value = r#"{"listen":"127.0.0.1:5302","members":[{"addr":"127.0.0.1:5303","live":true}],"digest_mismatch":[],"disagreements":[]}"#;
        let mut s = snap(Some(400), vec![api()]);
        assert_eq!(
            json(&s, Path::new("/s.mdb"), NOW)["peers"],
            serde_json::Value::Null
        );
        s.peers = peers_value(Some(value));
        let s = filter(s, Some("api.svc.emfor")).unwrap();
        assert_eq!(
            json(&s, Path::new("/s.mdb"), NOW)["peers"],
            serde_json::from_str::<serde_json::Value>(value).unwrap()
        );
    }

    /// Spec "Status filtering and JSON output", scenario "JSON metrics
    /// addresses".
    #[test]
    fn json_metrics_listen() {
        let mut s = snap(Some(400), vec![]);
        assert_eq!(
            json(&s, Path::new("/s.mdb"), NOW)["metrics_listen"],
            serde_json::Value::Null
        );
        s.metrics_listen = Some(vec!["127.0.0.1:5301".into()]);
        assert_eq!(
            json(&s, Path::new("/s.mdb"), NOW)["metrics_listen"],
            serde_json::json!(["127.0.0.1:5301"])
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
        assert!(w.update(&before, NOW).is_empty());

        // Only ages move: nothing to highlight.
        let mut later = before.clone();
        later.services[0].backends[0].checked_at = Some(NOW);
        later.services[0].backends[0].latency_us = Some(1500);
        assert!(w.update(&later, NOW).is_empty());

        let mut down = later.clone();
        let b = &mut down.services[0].backends[0];
        b.state = HealthState::Unhealthy;
        b.reason = Some("connect: Connection refused".into());
        down.services[0].all_down = true;
        let down = with_history(
            down,
            "api.svc.emfor",
            "10.0.0.11:8080",
            vec![entry(1000, Healthy, Unhealthy)],
        );
        let marked = w.update(&down, NOW);
        assert_eq!(
            marked,
            HashSet::from([
                Row::Service("api.svc.emfor".into()),
                Row::Backend("api.svc.emfor".into(), "10.0.0.11:8080".into()),
            ])
        );
        assert_eq!(
            w.changes(NOW).first().map(String::as_str),
            Some(&*format!(
                "{} api.svc.emfor 10.0.0.11:8080 healthy -> unhealthy",
                clock(NOW - 1000)
            ))
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
        w.update(&stopped, NOW + 1000);
        assert_eq!(
            w.changes(NOW + 1000).first().map(String::as_str),
            Some(&*format!("{} daemon running -> stopped", clock(NOW + 1000)))
        );
    }

    #[test]
    fn watch_keeps_the_last_ten_changes() {
        let mut w = Watch::default();
        let ages: Vec<u64> = (1..=12).map(|i| i * 1000).collect();
        let s = with_history(
            snap(Some(100), vec![api()]),
            "api.svc.emfor",
            "10.0.0.11:8080",
            alternating(&ages),
        );
        w.update(&s, NOW);
        let changes = w.changes(NOW);
        assert_eq!(changes.len(), 10);
        assert!(changes[0].starts_with(&clock(NOW - 1000)), "{changes:?}");
        assert!(changes[9].starts_with(&clock(NOW - 10_000)), "{changes:?}");
    }

    /// Spec "Status watch mode", example "merged change list", and scenario
    /// "Earlier changes shown at start".
    #[test]
    fn watch_merges_history_and_daemon_changes() {
        let yesterday = NOW - 2 * 86_400_000;
        let mut s = snap(Some(100), vec![api()]);
        let h = s.history.entry("api.svc.emfor".into()).or_default();
        h.backends.insert(
            "10.0.0.12:8080".into(),
            vec![
                HistoryEntry {
                    at: yesterday,
                    ..entry(0, Healthy, Unhealthy)
                },
                entry(45 * MIN, Unhealthy, Healthy),
            ],
        );
        let mut w = Watch::default();
        w.update(&s, NOW - 5000);
        // The first frame already lists the stored changes.
        assert_eq!(w.changes(NOW - 5000).len(), 2);
        s.heartbeat_ms = NOW - 5000;
        w.update(&s, NOW);
        assert_eq!(
            w.changes(NOW),
            [
                format!("{} daemon running -> stale", clock(NOW)),
                format!(
                    "{} api.svc.emfor 10.0.0.12:8080 unhealthy -> healthy",
                    clock(NOW - 45 * MIN)
                ),
                format!(
                    "{} api.svc.emfor 10.0.0.12:8080 healthy -> unhealthy",
                    datetime(yesterday)
                ),
            ]
        );
    }

    #[test]
    fn datetime_format() {
        let d = datetime(NOW);
        let b = d.as_bytes();
        assert_eq!(d.len(), 19, "{d}");
        assert_eq!(
            (b[4], b[7], b[10], b[13], b[16]),
            (b'-', b'-', b' ', b':', b':')
        );
        assert!(d.ends_with(&clock(NOW)));
    }

    /// Spec "Flapping backends", example "counting rule".
    #[test]
    fn flap_counting_rule() {
        let mut unknown_first = alternating(&[2 * MIN, 3 * MIN, 4 * MIN, 5 * MIN]);
        unknown_first.push(entry(MIN, Unknown, Healthy));
        let cases = [
            (
                alternating(&[MIN, 2 * MIN, 3 * MIN, 4 * MIN, 5 * MIN]),
                Some(5),
            ),
            (alternating(&[MIN, 2 * MIN, 3 * MIN, 4 * MIN]), None),
            (unknown_first, None),
            (
                alternating(&[MIN, 2 * MIN, 3 * MIN, 4 * MIN, 10 * MIN + 1000]),
                None,
            ),
            (
                alternating(&[MIN, 2 * MIN, 3 * MIN, 4 * MIN, 10 * MIN]),
                Some(5),
            ),
        ];
        for (i, (entries, want)) in cases.into_iter().enumerate() {
            let s = with_history(
                snap(Some(100), vec![api()]),
                "api.svc.emfor",
                "10.0.0.12:8080",
                entries,
            );
            let got = flapping(&s, NOW).first().map(|f| f.2);
            assert_eq!(got, want, "row {}", i + 1);
            let line = render(&s)[5].clone();
            match want {
                Some(n) => assert!(
                    line.contains(&format!("-      FLAPPING ({n} changes in 10m)  connect:")),
                    "{line}"
                ),
                None => assert!(!line.contains("FLAPPING"), "{line}"),
            }
        }
        // No history: not flapping.
        assert!(flapping(&snap(Some(100), vec![api()]), NOW).is_empty());
    }

    /// Spec "Flapping backends", scenarios "Backend marked as flapping" and
    /// "Flapping does not change the exit status".
    #[test]
    fn flapping_keeps_the_exit_status() {
        let ages: Vec<u64> = (1..=6).map(|i| i * 80_000).collect();
        let base = snap(Some(100), vec![api()]);
        let flappy = with_history(
            base.clone(),
            "api.svc.emfor",
            "10.0.0.11:8080",
            alternating(&ages),
        );
        assert!(render(&flappy)[4].contains("FLAPPING (6 changes in 10m)"));
        assert_eq!(exit_status(&flappy, NOW), 0);
        assert_eq!(exit_status(&flappy, NOW), exit_status(&base, NOW));
    }

    /// Spec "Status filtering and JSON output": `history` and `flapping`.
    #[test]
    fn json_history_and_flapping() {
        let ages: Vec<u64> = (1..=6).map(|i| i * 80_000).collect();
        let db = service("db.svc.emfor", false, vec![]);
        let s = with_history(
            snap(Some(100), vec![api(), db]),
            "api.svc.emfor",
            "10.0.0.12:8080",
            alternating(&ages),
        );
        let v = json(&s, Path::new("/s.mdb"), NOW);
        assert_eq!(
            v["history"]["api.svc.emfor"],
            serde_json::to_value(&s.history["api.svc.emfor"]).unwrap()
        );
        assert_eq!(
            v["flapping"],
            json!([{"service": "api.svc.emfor", "backend": "10.0.0.12:8080", "changes": 6}])
        );
        let only_db = json(
            &filter(s, Some("db.svc.emfor")).unwrap(),
            Path::new("/s.mdb"),
            NOW,
        );
        assert_eq!(only_db["history"], json!({}));
        assert_eq!(only_db["flapping"], json!([]));
        let none = json(&snap(Some(100), vec![api()]), Path::new("/s.mdb"), NOW);
        assert_eq!(
            (&none["history"], &none["flapping"]),
            (&json!({}), &json!([]))
        );
    }

    /// Spec "Status history output", scenario "History of one service".
    #[test]
    fn history_of_one_service() {
        let up = NOW - 22_000;
        let s = with_history(
            snap(Some(100), vec![api()]),
            "api.svc.emfor",
            "10.0.0.12:8080",
            vec![
                HistoryEntry {
                    at: up,
                    from: Unknown,
                    to: Healthy,
                    reason: None,
                },
                entry(0, Healthy, Unhealthy),
            ],
        );
        let lines = history_text(&s, Path::new("/s.mdb"), NOW, Style::default());
        assert_eq!(
            lines[2..],
            [
                "".to_owned(),
                "api.svc.emfor".to_owned(),
                "  10.0.0.11:8080".to_owned(),
                "    no changes recorded".to_owned(),
                "  10.0.0.12:8080".to_owned(),
                format!(
                    "    {} healthy -> unhealthy  connect: Connection refused",
                    datetime(NOW)
                ),
                format!("    {} unknown -> healthy", datetime(up)),
            ]
        );
        assert!(lines[1].starts_with("daemon: running"));
    }

    /// Spec "Status reads the store read-only" with history: what one read
    /// returns; a damaged value and the history of a service without a
    /// record are left out.
    #[test]
    fn reader_reads_history() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::store::tests::cfg(dir.path());
        let store = crate::store::Store::open(&cfg).unwrap();
        let mut h = ServiceHistory::default();
        h.push("10.0.0.12:8080", entry(0, Healthy, Unhealthy));
        let rec = serde_json::to_string(&api()).unwrap();
        let hist = serde_json::to_string(&h).unwrap();
        store
            .write(|txn, db| {
                db.put(txn, KEY_SCHEMA_VERSION, "1")?;
                db.put(txn, KEY_HEARTBEAT, "1")?;
                db.put(txn, "svc/api.svc.emfor", &rec)?;
                db.put(txn, "hist/api.svc.emfor", &hist)?;
                db.put(txn, "hist/old.svc.emfor", "not json")?;
                db.put(txn, "hist/gone.svc.emfor", &hist)?;
                Ok(())
            })
            .unwrap();
        // heed refuses a second open of the same file with other flags.
        drop(store);
        let snap = Reader::open(&cfg).unwrap().read().unwrap();
        assert_eq!(snap.history.keys().collect::<Vec<_>>(), ["api.svc.emfor"]);
        assert_eq!(snap.history["api.svc.emfor"], h);
    }

    fn lines(prefix: &str, n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("{prefix}{i}")).collect()
    }

    fn top() -> Vec<String> {
        vec!["store: x".to_owned(), "daemon: running".to_owned()]
    }

    /// The footer of a frame, or `None` when there is none.
    fn footer(frame: &[String]) -> Option<&str> {
        frame
            .last()
            .map(String::as_str)
            .filter(|l| l.starts_with("-- lines "))
    }

    /// Spec "Status watch mode", scenario "Output fits the terminal".
    #[test]
    fn watch_frame_that_fits_has_no_footer() {
        let content = lines("c", 13);
        let mut scroll = Scroll { offset: 5 };
        let frame = compose(&top(), &content, 24, &mut scroll, Style::default());
        assert_eq!(frame.len(), 15);
        assert_eq!(frame[..2], top()[..]);
        assert_eq!(frame[2..], content[..]);
        assert_eq!(scroll.offset, 0);
        assert_eq!(footer(&frame), None);
    }

    /// Spec "Status watch mode", scenario "Scrolling a long list" and its
    /// keys table.
    #[test]
    fn watch_scrolls_a_long_list() {
        let content = lines("c", 60);
        let first = compose(
            &top(),
            &content,
            24,
            &mut Scroll::default(),
            Style::default(),
        );
        assert_eq!(first.len(), 24);
        assert_eq!(
            footer(&first),
            Some("-- lines 1-21 of 60  PgUp/PgDn Up/Down Home/End  q quit --")
        );
        use Action::*;
        let cases: &[(&[Action], &str)] = &[
            (&[], "1-21"),
            (&[PageDown], "22-42"),
            (&[PageDown, PageDown], "40-60"),
            (&[PageDown, PageUp], "1-21"),
            (&[LineDown], "2-22"),
            (&[LineUp], "1-21"),
            (&[Bottom], "40-60"),
            (&[Bottom, Top], "1-21"),
            (&[Bottom, LineDown], "40-60"),
        ];
        for (keys, range) in cases {
            let mut scroll = Scroll::default();
            for &k in *keys {
                let page = page_height(2, content.len(), 24);
                assert_eq!(page, 21);
                scroll.apply(k, content.len(), page);
            }
            let frame = compose(&top(), &content, 24, &mut scroll, Style::default());
            assert_eq!(frame.len(), 24, "{keys:?}");
            assert_eq!(frame[..2], top()[..], "{keys:?}");
            let (a, b) = range.split_once('-').unwrap();
            let a: usize = a.parse().unwrap();
            assert_eq!(frame[2], format!("c{a}"), "{keys:?}");
            assert_eq!(frame[22], format!("c{b}"), "{keys:?}");
            assert_eq!(
                footer(&frame),
                Some(
                    format!("-- lines {range} of 60  PgUp/PgDn Up/Down Home/End  q quit --")
                        .as_str()
                ),
                "{keys:?}"
            );
        }
    }

    /// Spec "Status watch mode", scenario "Position kept across refreshes
    /// and limited when the list shrinks".
    #[test]
    fn watch_keeps_and_limits_the_position() {
        let mut scroll = Scroll::default();
        scroll.apply(Action::PageDown, 60, page_height(2, 60, 24));
        for _ in 0..3 {
            let frame = compose(&top(), &lines("c", 60), 24, &mut scroll, Style::default());
            assert!(footer(&frame).unwrap().starts_with("-- lines 22-42 of 60 "));
        }
        let frame = compose(&top(), &lines("c", 30), 24, &mut scroll, Style::default());
        assert!(footer(&frame).unwrap().starts_with("-- lines 10-30 of 30 "));
        assert_eq!(frame[2], "c10");
        // Shrinking until it fits goes back to the top.
        let frame = compose(&top(), &lines("c", 5), 24, &mut scroll, Style::default());
        assert_eq!(scroll.offset, 0);
        assert_eq!(frame[2..], lines("c", 5)[..]);
    }

    #[test]
    fn watch_frame_in_a_tiny_terminal() {
        let content = lines("c", 60);
        let mut scroll = Scroll::default();
        // One content row and the footer below the header.
        let frame = compose(&top(), &content, 4, &mut scroll, Style::default());
        assert_eq!(frame[2], "c1");
        assert!(footer(&frame).unwrap().starts_with("-- lines 1-1 of 60 "));
        scroll.apply(Action::PageDown, content.len(), page_height(2, 60, 4));
        assert_eq!(scroll.offset, 1);
        // Too small for the footer: never more lines than rows.
        let frame = compose(
            &top(),
            &content,
            3,
            &mut Scroll::default(),
            Style::default(),
        );
        assert_eq!(frame, ["store: x", "daemon: running", "c1"]);
        assert_eq!(
            compose(
                &top(),
                &content,
                0,
                &mut Scroll::default(),
                Style::default()
            )
            .len(),
            1
        );
    }

    #[test]
    fn watch_footer_in_reverse_video_only_with_color() {
        let content = lines("c", 60);
        let colored = compose(
            &top(),
            &content,
            24,
            &mut Scroll::default(),
            Style { color: true },
        );
        assert!(colored[23].starts_with(REVERSE));
        let plain = compose(
            &top(),
            &content,
            24,
            &mut Scroll::default(),
            Style::default(),
        );
        assert!(!plain[23].contains('\x1b'));
    }

    #[test]
    fn watch_keys() {
        let none = KeyModifiers::NONE;
        let cases = [
            (KeyCode::PageUp, none, Some(Action::PageUp)),
            (KeyCode::PageDown, none, Some(Action::PageDown)),
            (KeyCode::Char(' '), none, Some(Action::PageDown)),
            (KeyCode::Up, none, Some(Action::LineUp)),
            (KeyCode::Down, none, Some(Action::LineDown)),
            (KeyCode::Home, none, Some(Action::Top)),
            (KeyCode::End, none, Some(Action::Bottom)),
            (KeyCode::Char('q'), none, Some(Action::Quit)),
            (KeyCode::Char('Q'), KeyModifiers::SHIFT, Some(Action::Quit)),
            (
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
                Some(Action::Quit),
            ),
            (KeyCode::Char('c'), none, None),
            (KeyCode::Char('x'), none, None),
            (KeyCode::Esc, none, None),
            (KeyCode::Left, none, None),
        ];
        for (code, mods, want) in cases {
            assert_eq!(
                key_action(&KeyEvent::new(code, mods)),
                want,
                "{code:?} {mods:?}"
            );
        }
        let released = KeyEvent::new_with_kind(KeyCode::Char('q'), none, KeyEventKind::Release);
        assert_eq!(key_action(&released), None);
        let repeated = KeyEvent::new_with_kind(KeyCode::Down, none, KeyEventKind::Repeat);
        assert_eq!(key_action(&repeated), Some(Action::LineDown));
    }
}
