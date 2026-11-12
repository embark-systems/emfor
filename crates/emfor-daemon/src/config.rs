//! TOML config: parsing, defaults and per-service overrides, validation.
//!
//! The file is walked key by key (instead of `serde` with
//! `deny_unknown_fields`) so that every violation is reported, each with its
//! TOML path, rather than stopping at the first one.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use emfor_format::{Balance, OnAllDown};

use crate::checker::exec::parse_command;
use crate::checker::{Checker, Registry};

pub const DEFAULT_CONFIG_PATH: &str = "/etc/emfor/emfor.toml";

/// One validation problem, located by TOML path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub path: String,
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreConfig {
    pub path: PathBuf,
    pub map_size: usize,
    pub file_mode: u32,
    pub lock_file_mode: u32,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/var/lib/emfor/state.mdb"),
            map_size: 64 * 1024 * 1024,
            file_mode: 0o644,
            lock_file_mode: 0o666,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogConfig {
    pub level: String,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
        }
    }
}

/// The `[dns]` table. The DNS server runs only when it is present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsConfig {
    pub listen: Vec<SocketAddr>,
    /// Normalized zone name, without trailing dot.
    pub zone: String,
    /// Seconds.
    pub ttl: u32,
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            listen: vec![SocketAddr::from(([127, 0, 0, 1], 5300))],
            zone: "svc.emfor".into(),
            ttl: 1,
        }
    }
}

/// True when `name` equals `zone` or is under it. Both normalized.
pub fn in_zone(name: &str, zone: &str) -> bool {
    name == zone
        || name
            .strip_suffix(zone)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

/// Effective check settings of a service.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub interval: Duration,
    pub timeout: Duration,
    pub rise: u32,
    pub fall: u32,
    /// Fraction of `interval`, 0.0 to 0.5.
    pub jitter: f64,
    pub on_all_down: OnAllDown,
    pub balance: Balance,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(2),
            timeout: Duration::from_secs(1),
            rise: 2,
            fall: 3,
            jitter: 0.10,
            on_all_down: OnAllDown::Open,
            balance: Balance::RoundRobin,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Backend {
    /// Normalized backend string (`ip:port`, IPv6 as `[addr]:port`).
    pub id: String,
    pub addr: SocketAddr,
    /// `weight`, 1 to 100 (default 1); used by `weighted`.
    pub weight: u32,
    /// Lowercased `site` label; used by `locality`.
    pub site: Option<String>,
}

impl Backend {
    /// A backend with weight 1 and no site.
    pub fn new(addr: SocketAddr) -> Backend {
        Backend {
            id: addr.to_string(),
            addr,
            weight: 1,
            site: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServiceConfig {
    /// Normalized service name.
    pub name: String,
    pub check: String,
    /// Identity of the checker implementation: the `check` name for a
    /// built-in, `plugin:<name>:<command as JSON>` for a plugin.
    pub checker_id: String,
    pub check_options: toml::Table,
    pub checker: Arc<dyn Checker>,
    pub settings: Settings,
    pub backends: Vec<Backend>,
    /// Per-service override of `[dns] ttl`, in seconds.
    pub dns_ttl: Option<u32>,
}

impl ServiceConfig {
    /// True when the checker (`check`, or a plugin's `command`) or
    /// `check_options` differ, which resets backend state on reload.
    pub fn check_changed(&self, other: &ServiceConfig) -> bool {
        self.checker_id != other.checker_id || self.check_options != other.check_options
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Top-level `site`, lowercased: the data centre this host is in.
    pub site: Option<String>,
    pub store: StoreConfig,
    pub log: LogConfig,
    pub dns: Option<DnsConfig>,
    /// Sorted by name.
    pub services: Vec<ServiceConfig>,
}

impl Config {
    pub fn backend_count(&self) -> usize {
        self.services.iter().map(|s| s.backends.len()).sum()
    }
}

/// Read and validate a config file.
pub fn load(path: &Path, registry: &Registry) -> Result<Config, Vec<ConfigError>> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        vec![ConfigError {
            path: String::new(),
            message: format!("cannot read {}: {e}", path.display()),
        }]
    })?;
    parse(&text, registry)
}

/// Parse and validate config text.
pub fn parse(text: &str, registry: &Registry) -> Result<Config, Vec<ConfigError>> {
    let root: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        vec![ConfigError {
            path: String::new(),
            message: format!("invalid TOML: {}", e.message()),
        }]
    })?;
    let mut v = Validator::default();
    let config = v.root(&root, registry);
    if v.errors.is_empty() {
        Ok(config)
    } else {
        Err(v.errors)
    }
}

/// One valid `[plugins."<name>"]` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSpec {
    pub name: String,
    pub command: Vec<String>,
}

/// Names a plugin must not take.
const RESERVED_PLUGIN_NAMES: &[&str] = &["tcp", "http", "exec"];

fn valid_plugin_name(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Parse only the `[plugins]` table, without starting anything: the valid
/// declarations, plus every declaration error. [`parse`] reports the same
/// errors, so callers that also call `parse` use only the specs.
pub fn parse_plugins(text: &str) -> (Vec<PluginSpec>, Vec<ConfigError>) {
    let Ok(root) = text.parse::<toml::Table>() else {
        return (Vec::new(), Vec::new());
    };
    let mut v = Validator::default();
    let specs = v.plugins(&root);
    (specs, v.errors)
}

/// Parse only the `[store]` table. `None` when the TOML is unreadable or the
/// table itself is invalid. Used on reload to report a `[store]` change even
/// when other parts of the file fail validation.
pub fn parse_store(text: &str) -> Option<StoreConfig> {
    let root: toml::Table = text.parse().ok()?;
    let mut v = Validator::default();
    let store = v.store(&root);
    v.errors.is_empty().then_some(store)
}

/// Parse only `[dns] listen`: `Some(None)` without a `[dns]` table,
/// `Some(Some(addrs))` with one, `None` when the TOML is unreadable or the
/// listen value itself is invalid. Errors in other `[dns]` keys are ignored,
/// so a listen change is reported together with them on reload.
pub fn parse_dns_listen(text: &str) -> Option<Option<Vec<SocketAddr>>> {
    let root: toml::Table = text.parse().ok()?;
    let mut v = Validator::default();
    let Some(t) = v.table(&root, "dns") else {
        return v.errors.is_empty().then_some(None);
    };
    let listen = v.dns_listen(t);
    v.errors.is_empty().then_some(Some(listen))
}

/// Lowercase and strip one trailing dot.
pub fn normalize_service_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    lower.strip_suffix('.').map(str::to_string).unwrap_or(lower)
}

/// A site label: 1-63 characters of [a-z0-9-], not starting or ending with `-`.
fn valid_site(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

const SITE_RULE: &str =
    "invalid site label: 1-63 characters of [a-z0-9-], not starting or ending with -";

fn valid_service_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// TOML key quoted when it is not a bare key, as in `services."api.svc.emfor"`.
pub(crate) fn key_path(parent: &str, key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    let k = if bare {
        key.to_string()
    } else {
        format!("\"{key}\"")
    };
    if parent.is_empty() {
        k
    } else {
        format!("{parent}.{k}")
    }
}

/// Settings explicitly present in one table (`[defaults]` or a service).
#[derive(Default)]
struct PartialSettings {
    interval: Option<Duration>,
    timeout: Option<Duration>,
    rise: Option<u32>,
    fall: Option<u32>,
    jitter: Option<f64>,
    on_all_down: Option<OnAllDown>,
    balance: Option<Balance>,
}

impl PartialSettings {
    fn over(&self, base: &Settings) -> Settings {
        Settings {
            interval: self.interval.unwrap_or(base.interval),
            timeout: self.timeout.unwrap_or(base.timeout),
            rise: self.rise.unwrap_or(base.rise),
            fall: self.fall.unwrap_or(base.fall),
            jitter: self.jitter.unwrap_or(base.jitter),
            on_all_down: self.on_all_down.unwrap_or(base.on_all_down),
            balance: self.balance.unwrap_or(base.balance),
        }
    }
}

const SETTING_KEYS: &[&str] = &[
    "interval",
    "timeout",
    "rise",
    "fall",
    "jitter",
    "on_all_down",
    "balance",
];

#[derive(Default)]
struct Validator {
    errors: Vec<ConfigError>,
    /// Every name under `[plugins]`, valid or not. A service using one that
    /// is not in the registry (its declaration or start failed) gets no
    /// extra error: the plugin's own error explains it.
    declared_plugins: HashSet<String>,
}

impl Validator {
    fn err(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.errors.push(ConfigError {
            path: path.into(),
            message: message.into(),
        });
    }

    fn unknown_keys(&mut self, table: &toml::Table, parent: &str, allowed: &[&str]) {
        for key in table.keys() {
            if !allowed.contains(&key.as_str()) {
                self.err(key_path(parent, key), "unknown key");
            }
        }
    }

    fn table<'a>(&mut self, root: &'a toml::Table, key: &str) -> Option<&'a toml::Table> {
        match root.get(key) {
            None => None,
            Some(toml::Value::Table(t)) => Some(t),
            Some(_) => {
                self.err(key, "must be a table");
                None
            }
        }
    }

    fn string<'a>(&mut self, table: &'a toml::Table, parent: &str, key: &str) -> Option<&'a str> {
        match table.get(key) {
            None => None,
            Some(toml::Value::String(s)) => Some(s),
            Some(_) => {
                self.err(key_path(parent, key), "must be a string");
                None
            }
        }
    }

    fn root(&mut self, root: &toml::Table, registry: &Registry) -> Config {
        self.unknown_keys(
            root,
            "",
            &[
                "site", "store", "log", "dns", "defaults", "plugins", "services",
            ],
        );
        let site = self.site(root, "");
        self.plugins(root);
        let store = self.store(root);
        let log = self.log(root);
        let dns = self.dns(root);

        let defaults_table = self.table(root, "defaults");
        let defaults_partial = match defaults_table {
            Some(t) => {
                self.unknown_keys(t, "defaults", SETTING_KEYS);
                self.settings(t, "defaults")
            }
            None => PartialSettings::default(),
        };
        let defaults = defaults_partial.over(&Settings::default());
        if defaults.timeout > defaults.interval {
            self.err("defaults.timeout", "timeout must not exceed interval");
        }

        let mut services = Vec::new();
        if let Some(svcs) = self.table(root, "services") {
            let mut seen: HashMap<String, String> = HashMap::new();
            let ordered: BTreeMap<&String, &toml::Value> = svcs.iter().collect();
            for (raw_name, value) in ordered {
                let path = key_path("services", raw_name);
                let name = normalize_service_name(raw_name);
                if !valid_service_name(&name) {
                    self.err(&path, "invalid service name: labels of [a-z0-9-], 1-63 characters each, 253 characters total");
                } else if let Some(first) = seen.insert(name.clone(), raw_name.clone()) {
                    self.err(
                        &path,
                        format!("duplicate service \"{name}\" (also defined as \"{first}\")"),
                    );
                }
                let toml::Value::Table(t) = value else {
                    self.err(&path, "must be a table");
                    continue;
                };
                if let Some(s) = self.service(t, &path, name, &defaults, registry) {
                    services.push(s);
                }
            }
        }
        services.sort_by(|a, b| a.name.cmp(&b.name));
        Config {
            site,
            store,
            log,
            dns,
            services,
        }
    }

    /// `[plugins]`: the valid declarations, sorted by name (errors recorded).
    fn plugins(&mut self, root: &toml::Table) -> Vec<PluginSpec> {
        let mut specs = Vec::new();
        let Some(plugins) = self.table(root, "plugins") else {
            return specs;
        };
        let ordered: BTreeMap<&String, &toml::Value> = plugins.iter().collect();
        for (name, value) in ordered {
            self.declared_plugins.insert(name.clone());
            let path = key_path("plugins", name);
            let mut ok = true;
            if RESERVED_PLUGIN_NAMES.contains(&name.as_str()) {
                self.err(&path, "name is reserved for a built-in checker");
                ok = false;
            } else if !valid_plugin_name(name) {
                self.err(&path, "invalid plugin name: 1-63 characters of [a-z0-9_-]");
                ok = false;
            }
            let toml::Value::Table(t) = value else {
                self.err(&path, "must be a table");
                continue;
            };
            let before = self.errors.len();
            self.unknown_keys(t, &path, &["command"]);
            match parse_command(t.get("command")) {
                Ok(command) if ok && self.errors.len() == before => specs.push(PluginSpec {
                    name: name.clone(),
                    command,
                }),
                Ok(_) => {}
                Err(m) => self.err(key_path(&path, "command"), m),
            }
        }
        specs
    }

    /// `[dns] listen`, or the default when absent or invalid (errors recorded).
    fn dns_listen(&mut self, t: &toml::Table) -> Vec<SocketAddr> {
        match t.get("listen") {
            None => {}
            Some(toml::Value::Array(a)) if a.is_empty() => {
                self.err("dns.listen", "listen must not be empty")
            }
            Some(toml::Value::Array(a)) => {
                let mut out = Vec::new();
                for (i, item) in a.iter().enumerate() {
                    let ipath = format!("dns.listen[{i}]");
                    match item.as_str().map(str::parse::<SocketAddr>) {
                        Some(Ok(addr)) if addr.port() != 0 => {
                            if out.contains(&addr) {
                                self.err(ipath, format!("duplicate listen address \"{addr}\""));
                            } else {
                                out.push(addr);
                            }
                        }
                        _ => self.err(
                            ipath,
                            "listen address must be an IP literal with port: IPv4:port or [IPv6]:port, port 1-65535",
                        ),
                    }
                }
                return out;
            }
            Some(_) => self.err("dns.listen", "must be an array of \"ip:port\" strings"),
        }
        DnsConfig::default().listen
    }

    fn dns(&mut self, root: &toml::Table) -> Option<DnsConfig> {
        let t = self.table(root, "dns")?;
        let mut dns = DnsConfig::default();
        self.unknown_keys(t, "dns", &["listen", "zone", "ttl"]);
        dns.listen = self.dns_listen(t);
        if let Some(z) = self.string(t, "dns", "zone") {
            let zone = normalize_service_name(z);
            if valid_service_name(&zone) {
                dns.zone = zone;
            } else {
                self.err(
                    "dns.zone",
                    "invalid zone name: labels of [a-z0-9-], 1-63 characters each, 253 characters total",
                );
            }
        }
        if let Some(ttl) = self.dns_seconds(t, "dns", "ttl") {
            dns.ttl = ttl;
        }
        Some(dns)
    }

    fn store(&mut self, root: &toml::Table) -> StoreConfig {
        let mut store = StoreConfig::default();
        let Some(t) = self.table(root, "store") else {
            return store;
        };
        self.unknown_keys(
            t,
            "store",
            &["path", "map_size", "file_mode", "lock_file_mode"],
        );
        if let Some(p) = self.string(t, "store", "path") {
            if p.is_empty() {
                self.err("store.path", "must not be empty");
            } else {
                store.path = PathBuf::from(p);
            }
        }
        if let Some(s) = self.string(t, "store", "map_size") {
            match parse_size(s) {
                Some(n) if n >= 1024 * 1024 => store.map_size = n,
                Some(_) => self.err("store.map_size", "must be at least 1MiB"),
                None => self.err("store.map_size", "invalid size, use e.g. \"64MiB\""),
            }
        }
        for (key, slot) in [
            ("file_mode", &mut store.file_mode),
            ("lock_file_mode", &mut store.lock_file_mode),
        ] {
            if let Some(s) = self.string(t, "store", key) {
                match u32::from_str_radix(s, 8) {
                    Ok(m) if m <= 0o777 => *slot = m,
                    _ => self.err(
                        format!("store.{key}"),
                        "must be an octal mode string such as \"0644\"",
                    ),
                }
            }
        }
        store
    }

    fn log(&mut self, root: &toml::Table) -> LogConfig {
        let mut log = LogConfig::default();
        let Some(t) = self.table(root, "log") else {
            return log;
        };
        self.unknown_keys(t, "log", &["level"]);
        if let Some(l) = self.string(t, "log", "level") {
            if ["trace", "debug", "info", "warn", "error"].contains(&l) {
                log.level = l.to_string();
            } else {
                self.err(
                    "log.level",
                    "must be one of trace, debug, info, warn, error",
                );
            }
        }
        log
    }

    fn duration(&mut self, t: &toml::Table, parent: &str, key: &str) -> Option<Duration> {
        let s = self.string(t, parent, key)?;
        match humantime::parse_duration(s) {
            Ok(d) => Some(d),
            Err(_) => {
                self.err(
                    key_path(parent, key),
                    format!("invalid duration \"{s}\", use e.g. \"500ms\" or \"2s\""),
                );
                None
            }
        }
    }

    fn count(&mut self, t: &toml::Table, parent: &str, key: &str) -> Option<u32> {
        match t.get(key)? {
            toml::Value::Integer(n) if *n >= 1 && *n <= u32::MAX as i64 => Some(*n as u32),
            toml::Value::Integer(_) => {
                self.err(key_path(parent, key), format!("{key} must be at least 1"));
                None
            }
            _ => {
                self.err(key_path(parent, key), "must be an integer");
                None
            }
        }
    }

    fn settings(&mut self, t: &toml::Table, parent: &str) -> PartialSettings {
        let mut p = PartialSettings::default();
        if let Some(d) = self.duration(t, parent, "interval") {
            if d < Duration::from_millis(100) {
                self.err(key_path(parent, "interval"), "interval below 100ms");
            } else {
                p.interval = Some(d);
            }
        }
        if let Some(d) = self.duration(t, parent, "timeout") {
            if d.is_zero() {
                self.err(
                    key_path(parent, "timeout"),
                    "timeout must be greater than zero",
                );
            } else {
                p.timeout = Some(d);
            }
        }
        p.rise = self.count(t, parent, "rise");
        p.fall = self.count(t, parent, "fall");
        if let Some(s) = self.string(t, parent, "jitter") {
            match s
                .strip_suffix('%')
                .and_then(|n| n.trim().parse::<f64>().ok())
            {
                Some(pct) if (0.0..=50.0).contains(&pct) => p.jitter = Some(pct / 100.0),
                Some(_) => self.err(key_path(parent, "jitter"), "jitter above 50% or below 0%"),
                None => self.err(
                    key_path(parent, "jitter"),
                    format!("invalid jitter \"{s}\", use e.g. \"10%\""),
                ),
            }
        }
        if let Some(s) = self.string(t, parent, "on_all_down") {
            match s {
                "open" => p.on_all_down = Some(OnAllDown::Open),
                "closed" => p.on_all_down = Some(OnAllDown::Closed),
                _ => self.err(
                    key_path(parent, "on_all_down"),
                    "must be \"open\" or \"closed\"",
                ),
            }
        }
        if let Some(s) = self.string(t, parent, "balance") {
            match s {
                "round-robin" => p.balance = Some(Balance::RoundRobin),
                "sticky" => p.balance = Some(Balance::Sticky),
                "failover" => p.balance = Some(Balance::Failover),
                "weighted" => p.balance = Some(Balance::Weighted),
                "locality" => p.balance = Some(Balance::Locality),
                _ => self.err(
                    key_path(parent, "balance"),
                    "balance must be round-robin, sticky, failover, weighted, or locality",
                ),
            }
        }
        p
    }

    /// An optional `site` label, lowercased.
    fn site(&mut self, t: &toml::Table, parent: &str) -> Option<String> {
        let s = self.string(t, parent, "site")?.to_ascii_lowercase();
        if valid_site(&s) {
            Some(s)
        } else {
            self.err(key_path(parent, "site"), SITE_RULE);
            None
        }
    }

    /// A DNS TTL: whole seconds from 0s to 3600s.
    fn dns_seconds(&mut self, t: &toml::Table, parent: &str, key: &str) -> Option<u32> {
        let d = self.duration(t, parent, key)?;
        if d.subsec_nanos() != 0 {
            self.err(
                key_path(parent, key),
                format!("{key} must be whole seconds"),
            );
            None
        } else if d.as_secs() > 3600 {
            self.err(key_path(parent, key), format!("{key} above 3600s"));
            None
        } else {
            Some(d.as_secs() as u32)
        }
    }

    fn service(
        &mut self,
        t: &toml::Table,
        path: &str,
        name: String,
        defaults: &Settings,
        registry: &Registry,
    ) -> Option<ServiceConfig> {
        let mut allowed = vec!["check", "backends", "check_options", "dns_ttl"];
        allowed.extend_from_slice(SETTING_KEYS);
        self.unknown_keys(t, path, &allowed);

        let partial = self.settings(t, path);
        let settings = partial.over(defaults);
        if (partial.interval.is_some() || partial.timeout.is_some())
            && settings.timeout > settings.interval
        {
            self.err(
                key_path(path, "timeout"),
                "timeout must not exceed interval",
            );
        }

        let backends = self.backends(t, path);
        let dns_ttl = self.dns_seconds(t, path, "dns_ttl");

        let check_options = match t.get("check_options") {
            None => toml::Table::new(),
            Some(toml::Value::Table(o)) => o.clone(),
            Some(_) => {
                self.err(key_path(path, "check_options"), "must be a table");
                toml::Table::new()
            }
        };
        let check = match t.get("check") {
            None => {
                self.err(key_path(path, "check"), "missing required key");
                None
            }
            Some(toml::Value::String(c)) => Some(c.clone()),
            Some(_) => {
                self.err(key_path(path, "check"), "must be a string");
                None
            }
        }?;
        let Some(factory) = registry.get(&check) else {
            if self.declared_plugins.contains(&check) {
                return None; // the plugin's own error is reported
            }
            self.err(
                key_path(path, "check"),
                format!("unknown checker `{check}`"),
            );
            return None;
        };
        let checker = match factory.validate(&check_options) {
            Ok(c) => c,
            Err(errs) => {
                let opts_path = key_path(path, "check_options");
                for e in errs {
                    if e.key.is_empty() {
                        self.err(&opts_path, e.message);
                    } else {
                        self.err(format!("{opts_path}.{}", e.key), e.message);
                    }
                }
                return None;
            }
        };
        Some(ServiceConfig {
            name,
            checker_id: factory.checker_id(),
            check,
            check_options,
            checker,
            settings,
            backends: backends?,
            dns_ttl,
        })
    }

    fn backends(&mut self, t: &toml::Table, path: &str) -> Option<Vec<Backend>> {
        let bpath = key_path(path, "backends");
        let items = match t.get("backends") {
            None => {
                self.err(&bpath, "missing required key");
                return None;
            }
            Some(toml::Value::Array(a)) if a.is_empty() => {
                self.err(&bpath, "must not be empty");
                return None;
            }
            Some(toml::Value::Array(a)) => a,
            Some(_) => {
                self.err(
                    &bpath,
                    "must be an array of \"ip:port\" strings or { addr, weight, site } tables",
                );
                return None;
            }
        };
        let before = self.errors.len();
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for (i, item) in items.iter().enumerate() {
            let ipath = format!("{bpath}[{i}]");
            let (s, weight, site) = match item {
                toml::Value::String(s) => (s.as_str(), 1, None),
                toml::Value::Table(bt) => {
                    self.unknown_keys(bt, &ipath, &["addr", "weight", "site"]);
                    let weight = match bt.get("weight") {
                        None => 1,
                        Some(toml::Value::Integer(w)) if (1..=100).contains(w) => *w as u32,
                        Some(_) => {
                            self.err(key_path(&ipath, "weight"), "weight must be from 1 to 100");
                            1
                        }
                    };
                    let site = self.site(bt, &ipath);
                    match bt.get("addr") {
                        Some(toml::Value::String(a)) => (a.as_str(), weight, site),
                        Some(_) => {
                            self.err(key_path(&ipath, "addr"), "must be a string");
                            continue;
                        }
                        None => {
                            self.err(&ipath, "backend table needs addr");
                            continue;
                        }
                    }
                }
                _ => {
                    self.err(ipath, "must be a string or a table");
                    continue;
                }
            };
            match s.parse::<SocketAddr>() {
                Ok(addr) if addr.port() != 0 => {
                    if !seen.insert(addr) {
                        self.err(ipath, format!("duplicate backend \"{s}\""));
                    } else {
                        out.push(Backend {
                            weight,
                            site,
                            ..Backend::new(addr)
                        });
                    }
                }
                _ => self.err(
                    ipath,
                    format!("backend \"{s}\" must be an IP literal with port: IPv4:port or [IPv6]:port, port 1-65535"),
                ),
            }
        }
        (self.errors.len() == before).then_some(out)
    }
}

/// Parse `"64MiB"`, `"512KiB"`, `"1GiB"`, or a plain byte count.
fn parse_size(s: &str) -> Option<usize> {
    let s = s.trim();
    let (num, mult) = [
        ("GiB", 1usize << 30),
        ("MiB", 1 << 20),
        ("KiB", 1 << 10),
        ("B", 1),
    ]
    .iter()
    .find_map(|(suf, m)| s.strip_suffix(suf).map(|n| (n.trim(), *m)))
    .unwrap_or((s, 1));
    num.parse::<usize>().ok()?.checked_mul(mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(text: &str) -> Config {
        parse(text, &Registry::builtin()).unwrap_or_else(|e| panic!("errors: {e:?}"))
    }

    fn parse_err(text: &str) -> Vec<ConfigError> {
        parse(text, &Registry::builtin()).expect_err("expected validation errors")
    }

    const SVC: &str = r#"
[services."api.svc.emfor"]
check = "tcp"
backends = ["10.0.0.1:80"]
"#;

    #[test]
    fn effective_settings_resolution() {
        // Rows of the "effective settings resolution" example.
        let rr = Balance::RoundRobin;
        let rows: &[(&str, &str, u64, u32, Balance)] = &[
            ("", "", 2, 3, rr),
            ("[defaults]\ninterval = \"5s\"\n", "", 5, 3, rr),
            (
                "[defaults]\ninterval = \"5s\"\nfall = 5\n",
                "interval = \"1s\"\n",
                1,
                5,
                rr,
            ),
            (
                "[defaults]\nbalance = \"failover\"\n",
                "",
                2,
                3,
                Balance::Failover,
            ),
            (
                "[defaults]\nbalance = \"failover\"\n",
                "balance = \"sticky\"\n",
                2,
                3,
                Balance::Sticky,
            ),
        ];
        for (defaults, svc_extra, interval, fall, balance) in rows {
            let c = parse_ok(&format!("{defaults}{SVC}{svc_extra}"));
            let s = &c.services[0].settings;
            assert_eq!(
                s.interval,
                Duration::from_secs(*interval),
                "{defaults:?} {svc_extra:?}"
            );
            assert_eq!(s.fall, *fall);
            assert_eq!(s.balance, *balance, "{defaults:?} {svc_extra:?}");
        }
    }

    #[test]
    fn service_inherits_and_overrides() {
        let c = parse_ok(&format!(
            "[defaults]\ninterval = \"5s\"\n{SVC}interval = \"1s\"\n"
        ));
        let s = c.services[0].settings;
        assert_eq!(s.interval, Duration::from_secs(1));
        assert_eq!(s.timeout, Duration::from_secs(1));
        assert_eq!((s.rise, s.fall), (2, 3));
        assert!((s.jitter - 0.10).abs() < 1e-9);
        assert_eq!(s.on_all_down, OnAllDown::Open);
        assert_eq!(s.balance, Balance::RoundRobin);
    }

    #[test]
    fn service_balance_overrides_default() {
        let c = parse_ok(&format!(
            "[defaults]\nbalance = \"failover\"\n{SVC}balance = \"round-robin\"\n[services.b]\ncheck = \"tcp\"\nbackends = [\"10.0.0.2:80\"]\n"
        ));
        assert_eq!(c.services[0].name, "api.svc.emfor");
        assert_eq!(c.services[0].settings.balance, Balance::RoundRobin);
        assert_eq!(c.services[1].settings.balance, Balance::Failover);
    }

    #[test]
    fn dns_ttl_is_per_service_only() {
        let c = parse_ok(&format!("{SVC}dns_ttl = \"5s\"\n"));
        assert_eq!(c.services[0].dns_ttl, Some(5));
        assert_eq!(parse_ok(SVC).services[0].dns_ttl, None);
        let e = parse_err(&format!("[defaults]\ndns_ttl = \"5s\"\n{SVC}"));
        assert_eq!(e[0].path, "defaults.dns_ttl");
        assert_eq!(e[0].message, "unknown key");
        assert!(parse_ok(&format!("{SVC}dns_ttl = \"0s\"\n")).services[0].dns_ttl == Some(0));
        assert!(parse_ok(&format!("{SVC}dns_ttl = \"1h\"\n")).services[0].dns_ttl == Some(3600));
    }

    #[test]
    fn built_in_defaults_without_any_tables() {
        let c = parse_ok(SVC);
        assert_eq!(c.services[0].settings, Settings::default());
        assert_eq!(c.store, StoreConfig::default());
        assert_eq!(c.log.level, "info");
    }

    #[test]
    fn name_normalization() {
        let c = parse_ok(
            "[services.\"API.svc.emfor.\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n",
        );
        assert_eq!(c.services[0].name, "api.svc.emfor");
    }

    #[test]
    fn duplicate_after_normalization() {
        let e = parse_err(&format!(
            "{SVC}\n[services.\"API.SVC.EMFOR\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.2:80\"]\n"
        ));
        assert!(
            e.iter()
                .any(|e| e.message.contains("duplicate service \"api.svc.emfor\"")),
            "{e:?}"
        );
    }

    #[test]
    fn invalid_service_names() {
        for bad in ["-api.svc", "api..svc", "api_svc", "a.b-"] {
            let e = parse_err(&format!(
                "[services.\"{bad}\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n"
            ));
            assert!(
                e.iter()
                    .any(|e| e.message.starts_with("invalid service name")),
                "{bad}: {e:?}"
            );
        }
    }

    #[test]
    fn multiple_errors_reported_together() {
        let e = parse_err(
            "[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\ninterval = \"2s\"\ntimeout = \"3s\"\nbackends = [\"10.0.0.1:70000\"]\n",
        );
        assert!(
            e.iter()
                .any(|e| e.path == "services.\"api.svc.emfor\".timeout"),
            "{e:?}"
        );
        assert!(
            e.iter()
                .any(|e| e.path == "services.\"api.svc.emfor\".backends[0]"),
            "{e:?}"
        );
    }

    #[test]
    fn validation_outcomes_table() {
        // Rows of the "validation outcomes" example: (service lines, expected message fragment or None for ok).
        let rows: &[(&str, Option<&str>)] = &[
            (
                "interval = \"50ms\"\ntimeout = \"10ms\"\n",
                Some("interval below 100ms"),
            ),
            ("interval = \"1s\"\ntimeout = \"1s\"\n", None),
            (
                "interval = \"1s\"\ntimeout = \"2s\"\n",
                Some("timeout must not exceed interval"),
            ),
            ("jitter = \"60%\"\n", Some("jitter above 50%")),
            (
                "backends = [\"db.local:5432\"]\n",
                Some("must be an IP literal with port"),
            ),
            ("backends = [\"[2001:db8::1]:8080\"]\n", None),
            (
                "backends = [{ addr = \"10.0.0.1:80\", weight = 3, site = \"dc1\" }]\n",
                None,
            ),
            (
                "backends = [{ weight = 3 }]\n",
                Some("backend table needs addr"),
            ),
            (
                "backends = [{ addr = \"10.0.0.1:80\", wieght = 3 }]\n",
                Some("unknown key"),
            ),
            (
                "backends = [{ addr = \"10.0.0.1:80\", weight = 0 }]\n",
                Some("weight must be from 1 to 100"),
            ),
            (
                "backends = [{ addr = \"10.0.0.1:80\", weight = 101 }]\n",
                Some("weight must be from 1 to 100"),
            ),
            (
                "backends = [\"10.0.0.1:80\", { addr = \"10.0.0.1:80\" }]\n",
                Some("duplicate backend"),
            ),
            (
                "backends = [{ addr = \"10.0.0.1:80\", site = \"dc_1\" }]\n",
                Some("invalid site label"),
            ),
            ("balance = \"weighted\"\n", None),
            ("balance = \"locality\"\n", None),
            ("chekc = \"tcp\"\n", Some("unknown key")),
            ("check = \"redis\"\n", Some("unknown checker `redis`")),
            (
                "balance = \"least-conn\"\n",
                Some("balance must be round-robin, sticky, failover, weighted, or locality"),
            ),
            (
                "dns_ttl = \"1500ms\"\n",
                Some("dns_ttl must be whole seconds"),
            ),
            ("dns_ttl = \"2h\"\n", Some("dns_ttl above 3600s")),
        ];
        for (lines, expected) in rows {
            let mut svc = String::from("[services.\"api.svc.emfor\"]\n");
            svc.push_str(lines);
            if !lines.contains("check ") && !lines.contains("check=") {
                svc.push_str("check = \"tcp\"\n");
            }
            if !lines.contains("backends") {
                svc.push_str("backends = [\"10.0.0.1:80\"]\n");
            }
            match (parse(&svc, &Registry::builtin()), expected) {
                (Ok(_), None) => {}
                (Err(e), Some(frag)) => assert!(
                    e.iter().any(|e| e.message.contains(frag)),
                    "{lines:?}: {e:?}"
                ),
                (r, _) => panic!("{lines:?}: unexpected {r:?}"),
            }
        }
    }

    #[test]
    fn top_level_site() {
        let svc = "[services.a]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n";
        assert_eq!(parse_ok(svc).site, None);
        assert_eq!(
            parse_ok(&format!("site = \"DC-1\"\n{svc}")).site.as_deref(),
            Some("dc-1")
        );
        let e = parse_err(&format!("site = \"-dc1\"\n{svc}"));
        assert!(
            e.iter()
                .any(|e| e.path == "site" && e.message.contains("invalid site label")),
            "{e:?}"
        );
    }

    #[test]
    fn mixed_backend_forms() {
        let c = parse_ok(
            "[services.a]\ncheck = \"tcp\"\nbackends = [\"10.0.0.10:8080\", { addr = \"10.0.0.11:8080\", weight = 3, site = \"DC1\" }]\n",
        );
        let b = &c.services[0].backends;
        assert_eq!(
            (b[0].id.as_str(), b[0].weight, b[0].site.as_deref()),
            ("10.0.0.10:8080", 1, None)
        );
        assert_eq!(
            (b[1].id.as_str(), b[1].weight, b[1].site.as_deref()),
            ("10.0.0.11:8080", 3, Some("dc1"))
        );
    }

    #[test]
    fn backend_table_error_paths() {
        let e = parse_err(
            "[services.a]\ncheck = \"tcp\"\nbackends = [{ addr = \"10.0.0.1:80\", weight = 0 }]\n",
        );
        assert!(
            e.iter().any(|e| e.path == "services.a.backends[0].weight"),
            "{e:?}"
        );
    }

    #[test]
    fn unknown_key_path_and_check_options_errors() {
        let e = parse_err(&format!("{SVC}intervall = \"1s\"\n"));
        assert_eq!(e[0].path, "services.\"api.svc.emfor\".intervall");
        assert_eq!(
            e[0].to_string(),
            "services.\"api.svc.emfor\".intervall: unknown key"
        );

        let e = parse_err(&format!(
            "{SVC}\n[services.\"api.svc.emfor\".check_options]\npath = \"/\"\n"
        ));
        assert_eq!(e[0].path, "services.\"api.svc.emfor\".check_options.path");
    }

    #[test]
    fn store_and_log_tables() {
        let c = parse_ok(&format!(
            "[store]\npath = \"/tmp/x.mdb\"\nmap_size = \"16MiB\"\nfile_mode = \"0640\"\nlock_file_mode = \"0660\"\n[log]\nlevel = \"debug\"\n{SVC}"
        ));
        assert_eq!(c.store.path, PathBuf::from("/tmp/x.mdb"));
        assert_eq!(c.store.map_size, 16 << 20);
        assert_eq!((c.store.file_mode, c.store.lock_file_mode), (0o640, 0o660));
        assert_eq!(c.log.level, "debug");
        let e = parse_err(&format!("[store]\nfile_mode = \"999\"\n{SVC}"));
        assert_eq!(e[0].path, "store.file_mode");
        // The store table alone, even when the rest of the file is invalid.
        let s = parse_store("[store]\npath = \"/tmp/y.mdb\"\n[services.a]\nrise = 0\n").unwrap();
        assert_eq!(s.path, PathBuf::from("/tmp/y.mdb"));
        assert_eq!(parse_store("[store]\nfile_mode = \"999\"\n"), None);
        assert_eq!(parse_dns_listen("[services.a]\nrise = 0\n"), Some(None));
        assert_eq!(
            parse_dns_listen("[dns]\n[services.a]\nrise = 0\n"),
            Some(Some(DnsConfig::default().listen))
        );
        assert_eq!(parse_dns_listen("[dns]\nlisten = []\n"), None);
        // Other [dns] errors do not hide a listen change.
        assert_eq!(
            parse_dns_listen("[dns]\nlisten = [\"127.0.0.1:5301\"]\nttl = \"2h\"\n"),
            Some(Some(vec!["127.0.0.1:5301".parse().unwrap()]))
        );
        assert_eq!(parse_dns_listen("dns = 1\n"), None);
    }

    #[test]
    fn dns_table() {
        assert_eq!(parse_ok(SVC).dns, None);
        assert_eq!(
            parse_ok(&format!("[dns]\n{SVC}")).dns,
            Some(DnsConfig::default())
        );
        let c = parse_ok(&format!(
            "[dns]\nlisten = [\"127.0.0.1:53\", \"[::1]:5300\"]\nzone = \"SVC.Emfor.\"\nttl = \"30s\"\n{SVC}"
        ));
        let d = c.dns.unwrap();
        assert_eq!(
            d.listen,
            [
                "127.0.0.1:53".parse().unwrap(),
                "[::1]:5300".parse().unwrap()
            ]
        );
        assert_eq!(d.zone, "svc.emfor");
        assert_eq!(d.ttl, 30);
        let rows: &[(&str, &str, &str)] = &[
            ("ttl = \"2h\"", "dns.ttl", "ttl above 3600s"),
            ("ttl = \"1500ms\"", "dns.ttl", "ttl must be whole seconds"),
            ("listen = []", "dns.listen", "listen must not be empty"),
            (
                "listen = [\"localhost:5300\"]",
                "dns.listen[0]",
                "listen address must be an IP literal with port",
            ),
            (
                "listen = [\"127.0.0.1:0\"]",
                "dns.listen[0]",
                "listen address must be an IP literal with port",
            ),
            (
                "listen = [\"127.0.0.1:53\", \"127.0.0.1:53\"]",
                "dns.listen[1]",
                "duplicate listen address",
            ),
            ("zone = \"svc..emfor\"", "dns.zone", "invalid zone name"),
            ("zones = \"x\"", "dns.zones", "unknown key"),
        ];
        for (line, path, frag) in rows {
            let e = parse_err(&format!("[dns]\n{line}\n{SVC}"));
            assert!(
                e.iter()
                    .any(|e| e.path == *path && e.message.contains(frag)),
                "{line}: {e:?}"
            );
        }
    }

    #[test]
    fn zone_membership() {
        assert!(in_zone("svc.emfor", "svc.emfor"));
        assert!(in_zone("a.b.svc.emfor", "svc.emfor"));
        assert!(!in_zone("xsvc.emfor", "svc.emfor"));
        assert!(!in_zone("db.internal", "svc.emfor"));
    }

    #[test]
    fn rise_zero_and_backend_ids() {
        let e = parse_err(&format!("{SVC}rise = 0\n"));
        assert!(e[0].message.contains("rise must be at least 1"));
        let c = parse_ok(
            "[services.a]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\", \"[2001:DB8::1]:8080\"]\n",
        );
        let ids: Vec<_> = c.services[0]
            .backends
            .iter()
            .map(|b| b.id.as_str())
            .collect();
        assert_eq!(ids, ["10.0.0.1:80", "[2001:db8::1]:8080"]);
        let e = parse_err(
            "[services.a]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\", \"10.0.0.1:80\"]\n",
        );
        assert!(e[0].message.contains("duplicate backend"));
    }

    #[test]
    fn plugin_declaration_validation() {
        // Rows of the "plugin declaration validation" example; /bin/sh
        // stands in for an existing executable.
        let rows: &[(&str, Option<(&str, &str)>)] = &[
            ("[plugins.\"redis\"]\ncommand = [\"/bin/sh\"]\n", None),
            (
                "[plugins.\"redis\"]\n",
                Some(("plugins.redis.command", "missing required key")),
            ),
            (
                "[plugins.\"redis\"]\ncommand = []\n",
                Some(("plugins.redis.command", "must not be empty")),
            ),
            (
                "[plugins.\"redis\"]\ncommand = [\"emfor-redis\"]\n",
                Some(("plugins.redis.command", "not an absolute path")),
            ),
            (
                "[plugins.\"http\"]\ncommand = [\"/bin/sh\"]\n",
                Some(("plugins.http", "name is reserved for a built-in checker")),
            ),
            (
                "[plugins.\"Redis\"]\ncommand = [\"/bin/sh\"]\n",
                Some(("plugins.Redis", "invalid plugin name")),
            ),
            (
                "[plugins.\"redis\"]\ncommand = [\"/bin/sh\"]\nargs = [\"-v\"]\n",
                Some(("plugins.redis.args", "unknown key")),
            ),
        ];
        for (text, want) in rows {
            let (specs, errors) = parse_plugins(text);
            match want {
                None => {
                    assert!(errors.is_empty(), "{text}: {errors:?}");
                    assert_eq!(
                        specs,
                        vec![PluginSpec {
                            name: "redis".into(),
                            command: vec!["/bin/sh".into()],
                        }]
                    );
                    parse_ok(text);
                }
                Some((path, msg)) => {
                    assert!(specs.is_empty(), "{text}: {specs:?}");
                    assert_eq!(errors.len(), 1, "{text}: {errors:?}");
                    assert_eq!(errors[0].path, *path, "{text}");
                    assert!(errors[0].message.contains(msg), "{text}: {}", errors[0]);
                    // `parse` reports the same error, once.
                    assert_eq!(parse_err(text), errors, "{text}");
                }
            }
        }
    }

    #[test]
    fn service_using_a_broken_plugin_gets_no_extra_error() {
        let text = "[plugins.redis]\ncommand = []\n\n[services.a]\ncheck = \"redis\"\nbackends = [\"10.0.0.1:80\"]\n";
        let e = parse_err(text);
        assert_eq!(e.len(), 1, "{e:?}");
        assert_eq!(e[0].path, "plugins.redis.command");
        // An undeclared name is still an unknown checker.
        let e = parse_err("[services.a]\ncheck = \"redis\"\nbackends = [\"10.0.0.1:80\"]\n");
        assert!(e[0].message.contains("unknown checker `redis`"), "{e:?}");
    }

    /// Factory standing in for a plugin with a given checker id.
    struct FakePlugin(&'static str);

    impl crate::checker::CheckerFactory for FakePlugin {
        fn kind(&self) -> &str {
            "redis"
        }
        fn checker_id(&self) -> String {
            self.0.to_string()
        }
        fn validate(
            &self,
            _: &toml::Table,
        ) -> Result<Arc<dyn Checker>, Vec<crate::checker::OptionError>> {
            Ok(Arc::new(crate::checker::tcp::TcpChecker))
        }
    }

    #[test]
    fn plugin_command_change_is_a_check_change() {
        let text =
            "[services.a]\ncheck = \"redis\"\nbackends = [\"10.0.0.1:80\"]\ninterval = \"3s\"\n";
        let with = |id: &'static str| {
            let mut r = Registry::builtin();
            r.register(Arc::new(FakePlugin(id)));
            parse(text, &r).unwrap().services.remove(0)
        };
        let old = with("plugin:redis:[\"/a\"]");
        assert_eq!(old.checker_id, "plugin:redis:[\"/a\"]");
        assert!(!old.check_changed(&with("plugin:redis:[\"/a\"]")));
        assert!(old.check_changed(&with("plugin:redis:[\"/b\"]")));
        assert_eq!(parse_ok(SVC).services[0].checker_id, "tcp");
    }
}
