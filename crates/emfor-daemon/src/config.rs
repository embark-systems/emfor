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

/// The config file read when `--config` is absent: `/etc/emfor/emfor.toml`,
/// or `<EMFOR_PREFIX>/etc/emfor/emfor.toml` when built with `EMFOR_PREFIX`.
/// Fixed at compile time; rustc records the variable, so changing it
/// rebuilds the crate.
pub const DEFAULT_CONFIG_PATH: &str = {
    const PREFIX: &str = match prefix_dir(option_env!("EMFOR_PREFIX")) {
        Ok(p) => p,
        // A const panic can pass a `&str` through but cannot format the
        // bad value, so the message does not show it.
        Err(e) => panic!("{}", e),
    };
    // `format!` is not available in a const; copy the two parts by hand.
    const LEN: usize = PREFIX.len() + CONFIG_SUFFIX.len();
    const BYTES: [u8; LEN] = {
        let (p, s) = (PREFIX.as_bytes(), CONFIG_SUFFIX.as_bytes());
        let mut out = [0u8; LEN];
        let mut i = 0;
        while i < LEN {
            out[i] = if i < p.len() { p[i] } else { s[i - p.len()] };
            i += 1;
        }
        out
    };
    match std::str::from_utf8(&BYTES) {
        Ok(path) => path,
        // Two `&str` joined are always valid UTF-8.
        Err(_) => unreachable!(),
    }
};

/// What `DEFAULT_CONFIG_PATH` appends to the install prefix.
const CONFIG_SUFFIX: &str = "/etc/emfor/emfor.toml";

/// The install prefix to put before `CONFIG_SUFFIX` for an `EMFOR_PREFIX`
/// value (`None` when unset): `""` for unset, empty, `/` or `/usr`, any
/// other absolute path without one trailing `/`. A relative prefix is an
/// error.
const fn prefix_dir(prefix: Option<&str>) -> Result<&str, &'static str> {
    let Some(p) = prefix else { return Ok("") };
    if p.is_empty() {
        return Ok("");
    }
    if p.as_bytes()[0] != b'/' {
        return Err("EMFOR_PREFIX must be an absolute path");
    }
    let p = match p.as_bytes() {
        [.., b'/'] => p.split_at(p.len() - 1).0,
        _ => p,
    };
    match p.as_bytes() {
        b"" | b"/usr" => Ok(""),
        _ => Ok(p),
    }
}

/// One validation problem, located by TOML path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub path: String,
    pub message: String,
    /// The config file the key was last set in, when the config is split
    /// over `include_dir`. Best effort: an error that combines values from
    /// two files names the later one.
    pub file: Option<String>,
}

impl ConfigError {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        ConfigError {
            path: path.into(),
            message: message.into(),
            file: None,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)?;
        } else {
            write!(f, "{}: {}", self.path, self.message)?;
        }
        match &self.file {
            Some(file) => write!(f, " (in {file})"),
            None => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreConfig {
    pub path: PathBuf,
    pub map_size: usize,
    pub file_mode: u32,
    pub lock_file_mode: u32,
    /// Flush every commit to disk. Off by default (`MDB_NOSYNC`): the store
    /// is rebuilt on every start, so it is not worth waiting for the disk.
    pub sync: bool,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/var/lib/emfor/state.mdb"),
            map_size: 64 * 1024 * 1024,
            // Only the daemon's user and group can use the store; the
            // applications' users have to be in that group. 0644 and 0666
            // (every local user) have to be written out.
            file_mode: 0o640,
            lock_file_mode: 0o660,
            sync: false,
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
    /// UDP responses a second per client network; 0 is no limit.
    pub rate_limit: u32,
}

/// Default `[dns] rate_limit`.
pub const DEFAULT_DNS_RATE_LIMIT: u32 = 100;
/// Largest `[dns] rate_limit`.
const MAX_DNS_RATE_LIMIT: i64 = 1_000_000;

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            listen: vec![SocketAddr::from(([127, 0, 0, 1], 5300))],
            zone: "svc.emfor".into(),
            ttl: 1,
            rate_limit: DEFAULT_DNS_RATE_LIMIT,
        }
    }
}

/// The `[metrics]` table. The metrics endpoint runs only when it is present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsConfig {
    pub listen: Vec<SocketAddr>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            listen: vec![SocketAddr::from(([127, 0, 0, 1], 5301))],
        }
    }
}

/// The `[peers]` table: this daemon's peer group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeersConfig {
    /// This daemon's UDP address, also its identity in the group; one of
    /// `members`.
    pub listen: SocketAddr,
    /// Every member, this daemon included, in config order.
    pub members: Vec<SocketAddr>,
    pub secret_file: PathBuf,
    /// `previous_secret_file`: the secret `secret_file` held before its last
    /// change, used at start when that change is recent (see
    /// [`crate::peers::Keys::resumed`]).
    pub previous_secret_file: Option<PathBuf>,
    /// `encrypt`: send `EMP2` (encrypted) rather than `EMP1` messages.
    pub encrypt: bool,
    /// `accept_plaintext`: accept `EMP1` messages; `false` needs `encrypt`.
    pub accept_plaintext: bool,
}

impl PeersConfig {
    /// Members other than this daemon.
    pub fn others(&self) -> impl Iterator<Item = SocketAddr> + '_ {
        self.members.iter().copied().filter(|m| *m != self.listen)
    }
}

/// Fewest and most entries of `[peers] members`.
pub const MIN_MEMBERS: usize = 2;
pub const MAX_MEMBERS: usize = 64;
/// Shortest peer secret, in bytes.
pub const MIN_SECRET_LEN: usize = 32;

/// The most a secret file may hold, one trailing line feed included. A
/// secret needs 32 bytes; nothing needs more than this.
pub const MAX_SECRET_LEN: usize = 65536;

/// Read a peer secret: the file's bytes without one trailing line feed.
/// Errors are the messages config validation reports for `secret_file`.
pub fn read_secret(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    if !path.is_absolute() {
        return Err("must be an absolute path".into());
    }
    // Open first and check the file that was opened (not the name, which
    // could be pointed at another file in between), as `read_trusted` does.
    // O_NONBLOCK: opening a FIFO for reading must not wait for a writer; it
    // has no effect on a regular file.
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let meta = f
        .metadata()
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    let euid = nix::unistd::geteuid().as_raw();
    if !owner_trusted(meta.uid(), euid) {
        return Err(format!(
            "{} is owned by uid {}, not root or the user running emfor (uid {euid})",
            path.display(),
            meta.uid()
        ));
    }
    if meta.mode() & 0o004 != 0 {
        return Err(format!(
            "{} must not be readable by other users (chmod o-r)",
            path.display()
        ));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(format!(
            "{} must not be writable by group or other users (chmod go-w)",
            path.display()
        ));
    }
    // At most one byte more than allowed: a file that is much larger (the
    // path was pointed at a log, say) is not read into memory.
    let mut bytes = Vec::new();
    f.by_ref()
        .take(MAX_SECRET_LEN as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if bytes.len() > MAX_SECRET_LEN {
        return Err(format!(
            "{} is too large (more than {MAX_SECRET_LEN} bytes)",
            path.display()
        ));
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.len() < MIN_SECRET_LEN {
        return Err(format!(
            "secret must be at least {MIN_SECRET_LEN} bytes (has {})",
            bytes.len()
        ));
    }
    Ok(bytes)
}

/// Check definition digest of a service: the first 16 bytes, in lowercase
/// hex, of the SHA-256 of
/// `{"checker":<checker_id>,"options":<check_options>,"timeout_ms":<n>}`
/// with every object's keys sorted and no whitespace. Only what decides the
/// meaning of one check result is included, so hosts that differ in
/// `interval`, `rise`, `backends` and the like still share results.
pub fn check_digest(checker_id: &str, check_options: &toml::Table, timeout: Duration) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;

    /// The value with every object's keys sorted, whatever the map type keeps.
    fn canonical(v: serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(m) => {
                let sorted: BTreeMap<String, serde_json::Value> =
                    m.into_iter().map(|(k, v)| (k, canonical(v))).collect();
                serde_json::Value::Object(sorted.into_iter().collect())
            }
            serde_json::Value::Array(a) => {
                serde_json::Value::Array(a.into_iter().map(canonical).collect())
            }
            other => other,
        }
    }

    // A TOML table always converts to JSON (datetimes become objects).
    let options = serde_json::to_value(check_options).unwrap_or(serde_json::Value::Null);
    let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
    let mut doc = serde_json::Map::new();
    doc.insert("checker".into(), checker_id.into());
    doc.insert("options".into(), options);
    doc.insert("timeout_ms".into(), timeout_ms.into());
    let text = canonical(serde_json::Value::Object(doc)).to_string();
    let hash = Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(32);
    for b in &hash[..16] {
        let _ = write!(out, "{b:02x}");
    }
    out
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
    /// Take part in the peer group (no effect without `[peers]`).
    pub peers: bool,
    /// How long a backend that recovered from unhealthy takes to get its
    /// full share; zero is off.
    pub slow_start: Duration,
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
            peers: true,
            slow_start: Duration::ZERO,
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
    /// `backup = true`: used only while no primary backend passes.
    pub backup: bool,
    /// `drain = true`: never chosen, but still checked.
    pub drain: bool,
}

impl Backend {
    /// A primary backend with weight 1 and no site.
    pub fn new(addr: SocketAddr) -> Backend {
        Backend {
            id: addr.to_string(),
            addr,
            weight: 1,
            site: None,
            backup: false,
            drain: false,
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
    /// Check definition digest (see [`check_digest`]): peers exchange
    /// results for this service only when their digests are equal.
    pub digest: String,
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
    pub metrics: Option<MetricsConfig>,
    pub peers: Option<PeersConfig>,
    /// Sorted by name.
    pub services: Vec<ServiceConfig>,
    /// The valid `[plugins."<name>"]` declarations, sorted by name.
    pub plugins: Vec<PluginSpec>,
}

impl Config {
    pub fn backend_count(&self) -> usize {
        self.services.iter().map(|s| s.backends.len()).sum()
    }

    /// Names of the services with `peers = false`.
    pub fn services_off(&self) -> Vec<String> {
        self.services
            .iter()
            .filter(|s| !s.settings.peers)
            .map(|s| s.name.to_string())
            .collect()
    }
}

/// The main config file and the files of its `include_dir`, merged.
#[derive(Debug, Default)]
pub struct Layers {
    /// The merged config.
    pub table: toml::Table,
    /// Key path (as in [`key_path`]) to the file that last set it. Empty
    /// when `include_dir` is not set, so errors then carry no file.
    pub origins: HashMap<String, String>,
    /// Every file read: the main file first, then the fragments in order.
    pub files: Vec<PathBuf>,
    /// Problems that do not fail the load, such as a skipped file name.
    pub warnings: Vec<String>,
    /// Whether the main file sets `include_dir`.
    pub included: bool,
}

/// The merged config as TOML, for `emfor check-config --show`. With
/// `include_dir` set, every value is followed by a comment naming the file
/// that last set it (a list or inline table is set as a whole); without it,
/// one first comment names the main file. Plain scalars and lists come before
/// sub-tables, so the text is valid TOML. `include_dir` itself is written as
/// a comment only, since its files are merged already and a saved copy must
/// check without a `conf.d` next to it; comments aside, the text parses to
/// [`Layers::table`] without `include_dir`.
pub fn show_layers(layers: &Layers) -> String {
    let mut out = String::new();
    if !layers.included {
        let main = layers
            .files
            .first()
            .map_or_else(String::new, |f| f.display().to_string());
        out.push_str(&format!("# merged from {}\n", comment_safe(&main)));
    }
    if let Some(toml::Value::String(dir)) = layers.table.get("include_dir") {
        out.push_str(&format!(
            "# include_dir = {} (its files are merged below)\n",
            toml::Value::String(dir.clone())
        ));
    }
    show_table(&layers.table, "", "", layers, &mut out);
    out
}

/// `text` for a comment: a line break or other control character in a file
/// name would end the comment and leave the rest as TOML.
fn comment_safe(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() {
                format!("\\u{:04x}", u32::from(c))
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// A key as TOML writes it: bare when it can be, else a quoted, escaped string.
fn toml_key(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if bare {
        key.to_string()
    } else {
        toml::Value::String(key.to_string()).to_string()
    }
}

/// `path` is the lookup path ([`key_path`], as in `origins`); `shown` is the
/// same path with TOML-escaped keys, for headers.
fn show_table(table: &toml::Table, path: &str, shown: &str, layers: &Layers, out: &mut String) {
    for (key, value) in table {
        if matches!(value, toml::Value::Table(_)) || (path.is_empty() && key == "include_dir") {
            continue;
        }
        out.push_str(&format!("{} = {value}", toml_key(key)));
        if let Some(file) = layers.origins.get(&key_path(path, key)) {
            out.push_str(&format!(" # {}", comment_safe(file)));
        }
        out.push('\n');
    }
    for (key, value) in table {
        if let toml::Value::Table(sub) = value {
            let at = key_path(path, key);
            let name = if shown.is_empty() {
                toml_key(key)
            } else {
                format!("{shown}.{}", toml_key(key))
            };
            // A table that holds only sub-tables needs no header of its own.
            let only_tables =
                !sub.is_empty() && sub.values().all(|v| matches!(v, toml::Value::Table(_)));
            if !only_tables {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("[{name}]\n"));
            }
            show_table(sub, &at, &name, layers, out);
        }
    }
}

/// Keys that only the main config file may set: `emfor status` and
/// `emfor lookup` read the main file alone, and a fragment must not nest.
const MAIN_ONLY_KEYS: &[&str] = &["store", "include_dir"];

/// Read and validate a config file, with its `include_dir` fragments.
pub fn load(path: &Path, registry: &Registry) -> Result<Config, Vec<ConfigError>> {
    parse_layers(&read_layers(path)?, registry)
}

/// Validate merged config layers.
pub fn parse_layers(layers: &Layers, registry: &Registry) -> Result<Config, Vec<ConfigError>> {
    validate(&layers.table, &layers.origins, |v, root| {
        v.root(root, registry)
    })
}

/// Read only the `[store]` table of the main config file, for `emfor
/// status` and `emfor lookup`: the rest of the file and the `include_dir`
/// fragments are not read, so a broken service table does not hide the
/// store of a daemon still running with an older config.
pub fn load_store(path: &Path) -> Result<StoreConfig, Vec<ConfigError>> {
    validate(&read_table(path)?, &HashMap::new(), |v, root| v.store(root))
}

/// Parse and validate config text (a single file, no `include_dir`).
pub fn parse(text: &str, registry: &Registry) -> Result<Config, Vec<ConfigError>> {
    validate(&parse_table(text, "")?, &HashMap::new(), |v, root| {
        v.root(root, registry)
    })
}

fn read_table(path: &Path) -> Result<toml::Table, Vec<ConfigError>> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        vec![ConfigError::new(
            "",
            format!("cannot read {}: {e}", path.display()),
        )]
    })?;
    parse_table(&text, &path.display().to_string())
}

/// Parse one file. A syntax error is located as `name:line:column` when
/// `name` is not empty.
fn parse_table(text: &str, name: &str) -> Result<toml::Table, Vec<ConfigError>> {
    text.parse().map_err(|e: toml::de::Error| {
        let place = match (name.is_empty(), e.span()) {
            (false, Some(span)) => {
                let before = &text.as_bytes()[..span.start.min(text.len())];
                let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
                let col = before.len()
                    - before
                        .iter()
                        .rposition(|&b| b == b'\n')
                        .map_or(0, |i| i + 1)
                    + 1;
                format!("{name}:{line}:{col}: ")
            }
            (false, None) => format!("{name}: "),
            (true, _) => String::new(),
        };
        vec![ConfigError::new(
            "",
            format!("{place}invalid TOML: {}", e.message()),
        )]
    })
}

/// Read the main config file and, when it sets `include_dir`, the selected
/// files of that directory in byte-wise filename order, and merge them: the
/// main file is the lowest layer, a later file overrides an earlier one.
pub fn read_layers(path: &Path) -> Result<Layers, Vec<ConfigError>> {
    let mut warnings = Vec::new();
    read_layers_with(path, &mut warnings).map(|mut l| {
        l.warnings = warnings;
        l
    })
}

/// [`read_layers`] that also hands back the warnings when the load fails: a
/// skipped file can be the reason it failed.
pub fn read_layers_with(
    path: &Path,
    warnings: &mut Vec<String>,
) -> Result<Layers, Vec<ConfigError>> {
    // The main file can declare a plugin `command` as a fragment can, so it
    // is checked as a fragment is. `load_store` (status, lookup) runs no
    // plugin and keeps reading it without the check.
    let main = read_trusted(path)
        .map_err(|e| vec![ConfigError::new("", e)])
        .and_then(|text| parse_table(&text, &path.display().to_string()))?;
    let mut layers = Layers {
        files: vec![path.to_path_buf()],
        ..Layers::default()
    };
    let Some(dir_value) = main.get("include_dir") else {
        layers.table = main;
        return Ok(layers);
    };
    let toml::Value::String(dir_str) = dir_value else {
        return Err(vec![ConfigError::new("include_dir", "must be a string")]);
    };
    if dir_str.is_empty() {
        return Err(vec![ConfigError::new("include_dir", "must not be empty")]);
    }
    layers.included = true;
    let dir = path
        .parent()
        .map_or_else(|| PathBuf::from(dir_str), |p| p.join(dir_str));

    let mut errors = Vec::new();
    let fragments = list_fragments(&dir, warnings, &mut errors);
    let mut tables = vec![(path.display().to_string(), main)];
    for file in fragments {
        let shown = file.display().to_string();
        let text = match read_trusted(&file) {
            Ok(t) => t,
            Err(e) => {
                errors.push(ConfigError::new("", e));
                continue;
            }
        };
        layers.files.push(file);
        match parse_table(&text, &shown) {
            Ok(t) => {
                for key in MAIN_ONLY_KEYS {
                    if t.contains_key(*key) {
                        errors.push(ConfigError::new(
                            "",
                            format!("{shown}: {key} is only allowed in the main config file"),
                        ));
                    }
                }
                tables.push((shown, t));
            }
            Err(e) => errors.extend(e),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    for (shown, t) in tables {
        merge_table(&mut layers.table, t, "", &shown, &mut layers.origins);
    }
    Ok(layers)
}

/// The selected files of the include directory, in byte-wise name order:
/// regular files and links to regular files whose name ends in `.toml` and
/// does not start with `.`. The directory and each file must not be
/// writable by group or others, because a fragment can declare a plugin
/// command.
fn list_fragments(
    dir: &Path,
    warnings: &mut Vec<String>,
    errors: &mut Vec<ConfigError>,
) -> Vec<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let shown = dir.display();
    let meta = match std::fs::metadata(dir) {
        Ok(m) if m.is_dir() => m,
        Ok(_) => {
            errors.push(ConfigError::new(
                "include_dir",
                format!("{shown} is not a directory"),
            ));
            return Vec::new();
        }
        Err(e) => {
            errors.push(ConfigError::new(
                "include_dir",
                format!("cannot read directory {shown}: {e}"),
            ));
            return Vec::new();
        }
    };
    if let Some(why) = untrusted_at(
        dir,
        meta.permissions().mode(),
        std::os::unix::fs::MetadataExt::uid(&meta),
        nix::unistd::geteuid().as_raw(),
    ) {
        errors.push(ConfigError::new("", format!("{shown}: {why}")));
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            errors.push(ConfigError::new(
                "include_dir",
                format!("cannot read directory {shown}: {e}"),
            ));
            return Vec::new();
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                errors.push(ConfigError::new(
                    "include_dir",
                    format!("cannot read directory {shown}: {e}"),
                ));
                continue;
            }
        };
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            warnings.push(format!(
                "{shown}: skipping {:?}: the name is not valid UTF-8",
                entry.file_name()
            ));
            continue;
        };
        if name.starts_with('.') || !name.ends_with(".toml") {
            continue;
        }
        names.push(name);
    }
    names.sort();
    let mut files = Vec::new();
    for name in names {
        let file = dir.join(&name);
        let m = match std::fs::metadata(&file) {
            Ok(m) => m,
            // A dangling link is not a regular file.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                errors.push(ConfigError::new(
                    "",
                    format!("cannot read {}: {e}", file.display()),
                ));
                continue;
            }
        };
        if !m.is_file() {
            continue;
        }
        files.push(file);
    }
    files
}

/// Whether a file owned by `uid` can be trusted by a daemon whose effective
/// user is `euid`: root, or the daemon's own user. Anyone else who owns a
/// file can rewrite it, whatever its mode says.
fn owner_trusted(uid: u32, euid: u32) -> bool {
    uid == 0 || uid == euid
}

/// Why a config directory or file cannot be trusted, or `None`. A fragment
/// can declare a plugin `command`, which runs with the daemon's rights, so
/// whoever can write the directory or the file, or owns it, decides what the
/// daemon runs: group and others must not write it, and its owner must be
/// root or the user the daemon runs as (`euid`). For a link, the arguments
/// describe the file it points to.
fn untrusted(mode: u32, uid: u32, euid: u32) -> Option<String> {
    if mode & 0o022 != 0 {
        return Some(format!(
            "writable by group or others (mode {:04o}); refusing to load it",
            mode & 0o7777
        ));
    }
    if !owner_trusted(uid, euid) {
        return Some(format!(
            "owned by uid {uid}, not root or the user running emfor (uid {euid}); refusing to load it"
        ));
    }
    None
}

/// [`untrusted`] for `path`, with the command that fixes what it found: the
/// mode is the first reason, as in `untrusted`, so `chmod` comes first too.
fn untrusted_at(path: &Path, mode: u32, uid: u32, euid: u32) -> Option<String> {
    let why = untrusted(mode, uid, euid)?;
    let fix = if mode & 0o022 != 0 {
        "chmod g-w,o-w"
    } else {
        "chown root"
    };
    Some(format!("{why}; to fix it: {fix} {}", path.display()))
}

/// Open a config file (the main file or a fragment) and check the file that
/// was opened (not the name, which could be pointed at another file in
/// between), then read it.
fn read_trusted(file: &Path) -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    let shown = file.display();
    let mut f = std::fs::File::open(file).map_err(|e| format!("cannot read {shown}: {e}"))?;
    let meta = f
        .metadata()
        .map_err(|e| format!("cannot read {shown}: {e}"))?;
    if let Some(why) = untrusted_at(
        file,
        meta.mode(),
        meta.uid(),
        nix::unistd::geteuid().as_raw(),
    ) {
        return Err(format!("{shown}: {why}"));
    }
    let mut text = String::new();
    f.read_to_string(&mut text)
        .map_err(|e| format!("cannot read {shown}: {e}"))?;
    Ok(text)
}

/// Merge `over` into `base`: two tables merge key by key; anything else
/// (a scalar, a list, values of different types) is replaced by the later
/// value. `origins` records, for each key `over` sets, the file `name`.
fn merge_table(
    base: &mut toml::Table,
    over: toml::Table,
    parent: &str,
    name: &str,
    origins: &mut HashMap<String, String>,
) {
    for (key, value) in over {
        let path = key_path(parent, &key);
        origins.insert(path.clone(), name.to_string());
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => {
                merge_table(b, o, &path, name, origins);
            }
            (_, value) => {
                record_origins(&value, &path, name, origins);
                base.insert(key, value);
            }
        }
    }
}

/// Record `name` for every key below a value that replaces what was there.
fn record_origins(
    value: &toml::Value,
    path: &str,
    name: &str,
    origins: &mut HashMap<String, String>,
) {
    if let toml::Value::Table(t) = value {
        for (k, v) in t {
            let p = key_path(path, k);
            origins.insert(p.clone(), name.to_string());
            record_origins(v, &p, name, origins);
        }
    }
}

/// `services."a.b".backends[0]` -> `services."a.b".backends` -> `services."a.b"`
/// -> `services`: the key an error path hangs from.
fn parent_path(path: &str) -> Option<&str> {
    if let Some(stripped) = path.strip_suffix(']') {
        return stripped.rfind('[').map(|i| &path[..i]);
    }
    let mut in_quotes = false;
    let mut last_dot = None;
    for (i, b) in path.bytes().enumerate() {
        match b {
            b'"' => in_quotes = !in_quotes,
            b'.' if !in_quotes => last_dot = Some(i),
            _ => {}
        }
    }
    last_dot.map(|i| &path[..i])
}

/// The file that last set `path` or, failing that, its nearest parent key.
pub fn origin_of(origins: &HashMap<String, String>, path: &str) -> Option<String> {
    let mut at = Some(path);
    while let Some(p) = at {
        if let Some(f) = origins.get(p) {
            return Some(f.clone());
        }
        at = parent_path(p);
    }
    None
}

/// Run one validation over the parsed config; any error fails it.
fn validate<T>(
    root: &toml::Table,
    origins: &HashMap<String, String>,
    f: impl FnOnce(&mut Validator, &toml::Table) -> T,
) -> Result<T, Vec<ConfigError>> {
    let mut v = Validator {
        origins: origins.clone(),
        ..Validator::default()
    };
    let out = f(&mut v, root);
    if v.errors.is_empty() {
        Ok(out)
    } else {
        Err(v.errors)
    }
}

/// One valid `[plugins."<name>"]` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSpec {
    pub name: String,
    pub command: Vec<String>,
    /// Variables of the daemon's environment the plugin also gets.
    pub env_pass: Vec<String>,
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
#[cfg(test)]
pub fn parse_plugins(text: &str) -> (Vec<PluginSpec>, Vec<ConfigError>) {
    match text.parse::<toml::Table>() {
        Ok(root) => plugins_in(&root),
        Err(_) => (Vec::new(), Vec::new()),
    }
}

/// `parse_plugins` for an already parsed (merged) config.
pub fn plugins_in(root: &toml::Table) -> (Vec<PluginSpec>, Vec<ConfigError>) {
    let mut v = Validator::default();
    let specs = v.plugins(root);
    (specs, v.errors)
}

/// Parse only the `[store]` table. `None` when the TOML is unreadable or the
/// table itself is invalid. Used on reload to report a `[store]` change even
/// when other parts of the file fail validation.
#[cfg(test)]
pub fn parse_store(text: &str) -> Option<StoreConfig> {
    store_in(&text.parse().ok()?)
}

/// `parse_store` for an already parsed config.
pub fn store_in(root: &toml::Table) -> Option<StoreConfig> {
    let mut v = Validator::default();
    let store = v.store(root);
    v.errors.is_empty().then_some(store)
}

/// Parse only `[dns] listen`: `Some(None)` without a `[dns]` table,
/// `Some(Some(addrs))` with one, `None` when the TOML is unreadable or the
/// listen value itself is invalid. Errors in other `[dns]` keys are ignored,
/// so a listen change is reported together with them on reload.
#[cfg(test)]
pub fn parse_dns_listen(text: &str) -> Option<Option<Vec<SocketAddr>>> {
    dns_listen_in(&text.parse().ok()?)
}

/// `parse_dns_listen` for an already parsed config.
pub fn dns_listen_in(root: &toml::Table) -> Option<Option<Vec<SocketAddr>>> {
    let mut v = Validator::default();
    let Some(t) = v.table(root, "dns") else {
        return v.errors.is_empty().then_some(None);
    };
    let listen = v.listen(t, "dns", DnsConfig::default().listen);
    v.errors.is_empty().then_some(Some(listen))
}

/// Parse only `[metrics] listen`, like `parse_dns_listen`.
#[cfg(test)]
pub fn parse_metrics_listen(text: &str) -> Option<Option<Vec<SocketAddr>>> {
    metrics_listen_in(&text.parse().ok()?)
}

/// `parse_metrics_listen` for an already parsed config.
pub fn metrics_listen_in(root: &toml::Table) -> Option<Option<Vec<SocketAddr>>> {
    let mut v = Validator::default();
    let Some(t) = v.table(root, "metrics") else {
        return v.errors.is_empty().then_some(None);
    };
    let listen = v.listen(t, "metrics", MetricsConfig::default().listen);
    v.errors.is_empty().then_some(Some(listen))
}

/// Parse only `[peers] listen`, like [`dns_listen_in`]: `Some(None)`
/// without a `[peers]` table, `None` when the listen value is missing or
/// invalid.
pub fn peers_listen_in(root: &toml::Table) -> Option<Option<SocketAddr>> {
    let mut v = Validator::default();
    let Some(t) = v.table(root, "peers") else {
        return v.errors.is_empty().then_some(None);
    };
    let listen = v.socket_addr(t.get("listen")?, "peers.listen", "listen address")?;
    Some(Some(listen))
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

/// Longest `[dns] zone`: the SOA's `hostmaster.<zone>` must stay a valid name.
pub const MAX_ZONE_LEN: usize = 253 - "hostmaster.".len();

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
    peers: Option<bool>,
    slow_start: Option<Duration>,
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
            peers: self.peers.unwrap_or(base.peers),
            slow_start: self.slow_start.unwrap_or(base.slow_start),
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
    "peers",
    "slow_start",
];

#[derive(Default)]
struct Validator {
    errors: Vec<ConfigError>,
    /// Every name under `[plugins]`, valid or not. A service using one that
    /// is not in the registry (its declaration or start failed) gets no
    /// extra error: the plugin's own error explains it.
    declared_plugins: HashSet<String>,
    /// Key path to the file that last set it; see [`Layers::origins`].
    origins: HashMap<String, String>,
}

impl Validator {
    fn err(&mut self, path: impl Into<String>, message: impl Into<String>) {
        let path = path.into();
        let file = origin_of(&self.origins, &path);
        self.errors.push(ConfigError {
            path,
            message: message.into(),
            file,
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
                "site",
                "include_dir",
                "store",
                "log",
                "dns",
                "metrics",
                "peers",
                "defaults",
                "plugins",
                "services",
            ],
        );
        let site = self.site(root, "");
        let plugins = self.plugins(root);
        let store = self.store(root);
        let log = self.log(root);
        let dns = self.dns(root);
        let metrics = self.metrics(root);
        let peers = self.peers(root);
        if root.contains_key("peers") && site.is_none() && !root.contains_key("site") {
            self.err("site", "site is required when [peers] is set");
        }

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
                let zone = dns.as_ref().map(|d| d.zone.as_str());
                if let Some(s) = self.service(t, &path, name, &defaults, zone, registry) {
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
            metrics,
            peers,
            services,
            plugins,
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
            self.unknown_keys(t, &path, &["command", "env_pass"]);
            let env_pass = match crate::child::parse_env_pass(t.get("env_pass")) {
                Ok(names) => names,
                Err(m) => {
                    self.err(key_path(&path, "env_pass"), m);
                    Vec::new()
                }
            };
            match parse_command(t.get("command")) {
                Ok(command) if ok && self.errors.len() == before => specs.push(PluginSpec {
                    name: name.clone(),
                    command,
                    env_pass,
                }),
                Ok(_) => {}
                Err(m) => self.err(key_path(&path, "command"), m),
            }
        }
        specs
    }

    /// `[<table>] listen`, or `default` when absent or invalid (errors recorded).
    fn listen(
        &mut self,
        t: &toml::Table,
        table: &str,
        default: Vec<SocketAddr>,
    ) -> Vec<SocketAddr> {
        let path = format!("{table}.listen");
        match t.get("listen") {
            None => {}
            Some(toml::Value::Array(a)) if a.is_empty() => {
                self.err(path, "listen must not be empty")
            }
            Some(toml::Value::Array(a)) => {
                let mut out = Vec::new();
                for (i, item) in a.iter().enumerate() {
                    let ipath = format!("{path}[{i}]");
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
            Some(_) => self.err(path, "must be an array of \"ip:port\" strings"),
        }
        default
    }

    /// One `ip:port` with a port of 1-65535 (error recorded otherwise).
    fn socket_addr(&mut self, v: &toml::Value, path: &str, what: &str) -> Option<SocketAddr> {
        match v.as_str().map(str::parse::<SocketAddr>) {
            Some(Ok(addr)) if addr.port() != 0 => Some(addr),
            _ => {
                self.err(
                    path,
                    format!("{what} must be an IP literal with port: IPv4:port or [IPv6]:port, port 1-65535"),
                );
                None
            }
        }
    }

    /// `[peers]`; `None` without the table or when any of it is invalid
    /// (errors recorded).
    fn peers(&mut self, root: &toml::Table) -> Option<PeersConfig> {
        let t = self.table(root, "peers")?;
        let before = self.errors.len();
        self.unknown_keys(
            t,
            "peers",
            &[
                "listen",
                "members",
                "secret_file",
                "previous_secret_file",
                "encrypt",
                "accept_plaintext",
            ],
        );
        let encrypt = match t.get("encrypt") {
            None => true,
            Some(toml::Value::Boolean(b)) => *b,
            Some(_) => {
                self.err("peers.encrypt", "encrypt must be true or false");
                true
            }
        };
        // Without a value, plaintext is accepted only while this daemon sends
        // it too: `encrypt = true` (the default) refuses it, so that a member
        // that sends `EMP1` is not heard. A value that is set wins.
        let accept_plaintext = match t.get("accept_plaintext") {
            None => !encrypt,
            Some(toml::Value::Boolean(b)) => *b,
            Some(_) => {
                self.err(
                    "peers.accept_plaintext",
                    "accept_plaintext must be true or false",
                );
                true
            }
        };
        if !accept_plaintext && !encrypt {
            // Every member with this config would drop every message.
            self.err(
                "peers.accept_plaintext",
                "accept_plaintext = false needs encrypt = true: this daemon would send the plaintext messages it refuses",
            );
        }
        let listen = match t.get("listen") {
            None => {
                self.err("peers.listen", "missing: this daemon's peer address");
                None
            }
            Some(v) => self.socket_addr(v, "peers.listen", "listen address"),
        };
        if let Some(l) = listen
            && l.ip().is_unspecified()
        {
            self.err(
                "peers.listen",
                "listen must be a specific address, not 0.0.0.0 or ::",
            );
        }
        let mut members = Vec::new();
        match t.get("members") {
            None => self.err("peers.members", "missing: the addresses of every member"),
            Some(toml::Value::Array(a)) => {
                for (i, item) in a.iter().enumerate() {
                    let ipath = format!("peers.members[{i}]");
                    if let Some(addr) = self.socket_addr(item, &ipath, "member") {
                        if members.contains(&addr) {
                            self.err(ipath, format!("duplicate member \"{addr}\""));
                        } else {
                            members.push(addr);
                        }
                    }
                }
                if !(MIN_MEMBERS..=MAX_MEMBERS).contains(&a.len()) {
                    self.err(
                        "peers.members",
                        format!("members must list {MIN_MEMBERS} to {MAX_MEMBERS} addresses"),
                    );
                }
            }
            Some(_) => self.err("peers.members", "must be an array of \"ip:port\" strings"),
        }
        if let Some(l) = listen
            && !l.ip().is_unspecified()
            && t.get("members").is_some()
            && !members.contains(&l)
        {
            self.err("peers.listen", "listen must be one of members");
        }
        // One socket bound to `listen` sends to every member, so they must
        // all be of its address family.
        if let (Some(l), Some(toml::Value::Array(a))) = (listen, t.get("members")) {
            for (i, item) in a.iter().enumerate() {
                if let Some(Ok(m)) = item.as_str().map(str::parse::<SocketAddr>)
                    && m.is_ipv4() != l.is_ipv4()
                {
                    self.err(
                        format!("peers.members[{i}]"),
                        "member must be of the same address family (IPv4 or IPv6) as listen",
                    );
                }
            }
        }
        let secret_file = match self.string(t, "peers", "secret_file") {
            None => {
                if !t.contains_key("secret_file") {
                    self.err(
                        "peers.secret_file",
                        "missing: the file holding the peer secret",
                    );
                }
                None
            }
            Some(s) => {
                let path = PathBuf::from(s);
                match read_secret(&path) {
                    Ok(_) => Some(path),
                    Err(m) => {
                        self.err("peers.secret_file", m);
                        None
                    }
                }
            }
        };
        let previous_secret_file = match self.string(t, "peers", "previous_secret_file") {
            None => None,
            Some(s) => {
                let path = PathBuf::from(s);
                if let Err(m) = read_secret(&path) {
                    self.err("peers.previous_secret_file", m);
                }
                Some(path)
            }
        };
        if self.errors.len() != before {
            return None;
        }
        Some(PeersConfig {
            listen: listen?,
            members,
            secret_file: secret_file?,
            previous_secret_file,
            encrypt,
            accept_plaintext,
        })
    }

    fn metrics(&mut self, root: &toml::Table) -> Option<MetricsConfig> {
        let t = self.table(root, "metrics")?;
        self.unknown_keys(t, "metrics", &["listen"]);
        Some(MetricsConfig {
            listen: self.listen(t, "metrics", MetricsConfig::default().listen),
        })
    }

    fn dns(&mut self, root: &toml::Table) -> Option<DnsConfig> {
        let t = self.table(root, "dns")?;
        let mut dns = DnsConfig::default();
        self.unknown_keys(t, "dns", &["listen", "zone", "ttl", "rate_limit"]);
        dns.listen = self.listen(t, "dns", DnsConfig::default().listen);
        if let Some(z) = self.string(t, "dns", "zone") {
            let zone = normalize_service_name(z);
            if !valid_service_name(&zone) {
                self.err(
                    "dns.zone",
                    "invalid zone name: labels of [a-z0-9-], 1-63 characters each, 253 characters total",
                );
            } else if zone.len() > MAX_ZONE_LEN {
                // The SOA names hostmaster.<zone>, which must fit in 253.
                self.err(
                    "dns.zone",
                    format!("zone name longer than {MAX_ZONE_LEN} characters"),
                );
            } else {
                dns.zone = zone;
            }
        }
        if let Some(ttl) = self.dns_seconds(t, "dns", "ttl") {
            dns.ttl = ttl;
        }
        match t.get("rate_limit") {
            None => {}
            Some(toml::Value::Integer(n)) if (0..=MAX_DNS_RATE_LIMIT).contains(n) => {
                dns.rate_limit = *n as u32;
            }
            Some(toml::Value::Integer(_)) => self.err(
                "dns.rate_limit",
                format!("rate_limit must be 0 to {MAX_DNS_RATE_LIMIT}"),
            ),
            Some(_) => self.err("dns.rate_limit", "must be an integer"),
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
            &["path", "map_size", "file_mode", "lock_file_mode", "sync"],
        );
        match t.get("sync") {
            None => {}
            Some(toml::Value::Boolean(b)) => store.sync = *b,
            Some(_) => self.err("store.sync", "must be true or false"),
        }
        if let Some(p) = self.string(t, "store", "path") {
            if p.is_empty() {
                self.err("store.path", "must not be empty");
            } else {
                store.path = PathBuf::from(p);
            }
        }
        if let Some(s) = self.string(t, "store", "map_size") {
            match parse_size(s) {
                // LMDB refuses a size that is not a multiple of the page
                // size; every common page size (4, 16, 64KiB) divides 64KiB.
                Some(n) if n >= 1024 * 1024 => match n.checked_next_multiple_of(MAP_SIZE_UNIT) {
                    Some(n) => store.map_size = n,
                    None => self.err("store.map_size", "too large"),
                },
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
        match parse_duration(s) {
            Some(d) => Some(d),
            None => {
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
            } else if d > MAX_INTERVAL {
                self.err(key_path(parent, "interval"), "interval above 24h");
            } else {
                p.interval = Some(d);
            }
        }
        if let Some(d) = self.duration(t, parent, "timeout") {
            if d < MIN_TIMEOUT {
                self.err(key_path(parent, "timeout"), "timeout below 10ms");
            } else {
                p.timeout = Some(d);
            }
        }
        if let Some(d) = self.duration(t, parent, "slow_start") {
            if d.is_zero() || (Duration::from_secs(1)..=Duration::from_secs(3600)).contains(&d) {
                p.slow_start = Some(d);
            } else {
                self.err(
                    key_path(parent, "slow_start"),
                    "slow_start must be 0s or from 1s to 1h",
                );
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
                "fastest" => p.balance = Some(Balance::Fastest),
                _ => self.err(
                    key_path(parent, "balance"),
                    "balance must be round-robin, sticky, failover, weighted, locality, or fastest",
                ),
            }
        }
        match t.get("peers") {
            None => {}
            Some(toml::Value::Boolean(b)) => p.peers = Some(*b),
            Some(_) => self.err(key_path(parent, "peers"), "peers must be true or false"),
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
        zone: Option<&str>,
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
        // The DNS server names each backend `<ip label>.<service>.` in SRV
        // answers; build those names now, as the answer would.
        if let (Some(zone), Some(list)) = (zone, &backends)
            && in_zone(&name, zone)
        {
            for (i, b) in list.iter().enumerate() {
                if crate::dns::srv_target(b.addr.ip(), &name).is_none() {
                    self.err(
                        format!("{}[{i}]", key_path(path, "backends")),
                        "service name too long for this backend's SRV target name (<ip label>.<service>, at most 253 characters)",
                    );
                }
            }
        }
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
        let checker_id = factory.checker_id();
        let digest = check_digest(&checker_id, &check_options, settings.timeout);
        Some(ServiceConfig {
            name,
            checker_id,
            check,
            check_options,
            checker,
            settings,
            backends: backends?,
            dns_ttl,
            digest,
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
                    "must be an array of \"ip:port\" strings or { addr, weight, site, backup, drain } tables",
                );
                return None;
            }
        };
        let before = self.errors.len();
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for (i, item) in items.iter().enumerate() {
            let ipath = format!("{bpath}[{i}]");
            let (s, weight, site, backup, drain) = match item {
                toml::Value::String(s) => (s.as_str(), 1, None, false, false),
                toml::Value::Table(bt) => {
                    self.unknown_keys(bt, &ipath, &["addr", "weight", "site", "backup", "drain"]);
                    let drain = match bt.get("drain") {
                        None => false,
                        Some(toml::Value::Boolean(b)) => *b,
                        Some(_) => {
                            self.err(key_path(&ipath, "drain"), "drain must be true or false");
                            false
                        }
                    };
                    let backup = match bt.get("backup") {
                        None => false,
                        Some(toml::Value::Boolean(b)) => *b,
                        Some(_) => {
                            self.err(key_path(&ipath, "backup"), "backup must be true or false");
                            false
                        }
                    };
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
                        Some(toml::Value::String(a)) => (a.as_str(), weight, site, backup, drain),
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
                            backup,
                            drain,
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
        if self.errors.len() == before && out.iter().all(|b| b.backup) {
            self.err(&bpath, "at least one backend must not be a backup");
        }
        (self.errors.len() == before).then_some(out)
    }
}

/// `[store] map_size` is rounded up to a multiple of this.
const MAP_SIZE_UNIT: usize = 64 * 1024;

/// Longest `interval`. A backend checked less than once a day is not
/// monitored in any useful sense, and the bound keeps `fall × interval`
/// (the error window) far from the limits of `Duration` and `Instant`.
const MAX_INTERVAL: Duration = Duration::from_secs(24 * 3600);

/// Shortest `timeout`. Plugins get the timeout in whole milliseconds minus
/// a margin, and no network check is meaningful in less.
const MIN_TIMEOUT: Duration = Duration::from_millis(10);

/// Parse a duration: one or more `<number><unit>` terms, such as `"500ms"`,
/// `"2s"`, `"1.5s"` or `"1h 30m"`, or a plain `"0"`. The number is decimal and
/// may have a fraction; spaces between terms are optional. Units: `ns`, `us`
/// (or `µs`), `ms`, `s`, `m`, `h`, `d`, `w`, and the spelled-out forms
/// `nanos`, `nsec`, `usec`, `millis`, `msec`, `sec`, `secs`, `second`,
/// `seconds`, `min`, `mins`, `minute`, `minutes`, `hr`, `hrs`, `hour`,
/// `hours`, `day`, `days`, `wk`, `wks`, `week`, `weeks`. A fraction smaller
/// `M`, `month`, `months` (30.44 days), `y`, `yr`, `yrs`, `year`, `years`
/// (365.25 days). A fraction smaller than a nanosecond is cut off. It is the
/// syntax the `humantime` crate accepted, which this replaced; no setting
/// allows more than 24 hours, so a month or a year is always refused, but with
/// the message about the bound and not about the syntax.
fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s == "0" {
        return Some(Duration::ZERO);
    }
    let mut rest = s;
    let mut total: u128 = 0; // nanoseconds
    loop {
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits == 0 {
            return None;
        }
        let whole: u128 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let mut frac = "";
        if let Some(after) = rest.strip_prefix('.') {
            let n = after
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after.len());
            if n == 0 {
                return None;
            }
            frac = &after[..n];
            rest = &after[n..];
        }
        rest = rest.trim_start();
        let end = rest
            .find(|c: char| !(c.is_ascii_alphabetic() || c == 'µ'))
            .unwrap_or(rest.len());
        let unit: u128 = match &rest[..end] {
            "ns" | "nsec" | "nanos" => 1,
            "us" | "µs" | "usec" => 1_000,
            "ms" | "msec" | "millis" => 1_000_000,
            "s" | "sec" | "secs" | "second" | "seconds" => 1_000_000_000,
            "m" | "min" | "mins" | "minute" | "minutes" => 60 * 1_000_000_000,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600 * 1_000_000_000,
            "d" | "day" | "days" => 86_400 * 1_000_000_000,
            "w" | "wk" | "wks" | "week" | "weeks" => 7 * 86_400 * 1_000_000_000,
            "M" | "month" | "months" => 2_630_016 * 1_000_000_000,
            "y" | "yr" | "yrs" | "year" | "years" => 31_557_600 * 1_000_000_000,
            _ => return None,
        };
        rest = &rest[end..];
        let mut term = whole.checked_mul(unit)?;
        // The fraction is worth `frac / 10^len` of the unit. Digits past the
        // 18th cannot change a result counted in whole nanoseconds.
        let frac = &frac[..frac.len().min(18)];
        if !frac.is_empty() {
            let scale = 10u128.pow(frac.len() as u32);
            term = term.checked_add(frac.parse::<u128>().ok()?.checked_mul(unit)? / scale)?;
        }
        total = total.checked_add(term)?;
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
    }
    let secs = u64::try_from(total / 1_000_000_000).ok()?;
    Some(Duration::new(secs, (total % 1_000_000_000) as u32))
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
mod duration_tests {
    use super::*;

    #[test]
    fn parses_what_the_manual_shows_and_the_humantime_forms() {
        let ms = Duration::from_millis;
        let cases: &[(&str, Duration)] = &[
            ("0", Duration::ZERO),
            ("500ms", ms(500)),
            ("2s", ms(2000)),
            ("1m", ms(60_000)),
            ("1h", ms(3_600_000)),
            ("24h", ms(86_400_000)),
            ("1d", ms(86_400_000)),
            ("1w", ms(604_800_000)),
            ("1.5s", ms(1500)),
            ("0.25s", ms(250)),
            ("100us", Duration::from_micros(100)),
            ("100µs", Duration::from_micros(100)),
            ("7ns", Duration::from_nanos(7)),
            ("1h 30m", ms(5_400_000)),
            ("1h30m", ms(5_400_000)),
            ("1m 30s 500ms", ms(90_500)),
            (" 2s ", ms(2000)),
            ("2 seconds", ms(2000)),
            ("1 minute", ms(60_000)),
            ("0s", Duration::ZERO),
            ("1month", Duration::from_secs(2_630_016)),
            ("1M", Duration::from_secs(2_630_016)),
            ("200years", Duration::from_secs(200 * 31_557_600)),
            ("0.0000000019s", Duration::from_nanos(1)),
        ];
        for (text, want) in cases {
            assert_eq!(parse_duration(text), Some(*want), "{text:?}");
        }
    }

    #[test]
    fn rejects_what_is_not_a_duration() {
        for text in [
            "",
            " ",
            "s",
            "2",
            "1.s",
            ".5s",
            "-1s",
            "1 x",
            "1sec2",
            "2s,",
            "2s 3",
            "1e3s",
            "+1s",
            "１s",
            // More than a Duration holds.
            "99999999999999999999999999999999999999s",
            "18446744073709551616s",
        ] {
            assert_eq!(parse_duration(text), None, "{text:?}");
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
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

    /// A peer secret file with `content` and `mode`, in its own temp dir.
    pub(crate) fn secret_file(content: &[u8], mode: u32) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("a temporary directory can be created");
        let path = dir.path().join("peers.key");
        std::fs::write(&path, content).expect("the temporary secret file can be written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("the temporary secret file's mode can be set");
        (dir, path)
    }

    /// Spec peer-group "Secret file larger than the limit": 65536 bytes with
    /// the line feed are accepted, one more is an error.
    // spectra-scenario: peer-group / Secret file larger than the limit
    #[test]
    fn secret_file_has_a_size_limit() {
        let mut ok = vec![b'k'; MAX_SECRET_LEN - 1];
        ok.push(b'\n');
        let (_d, path) = secret_file(&ok, 0o600);
        assert_eq!(read_secret(&path).unwrap().len(), MAX_SECRET_LEN - 1);
        let (_d2, big) = secret_file(&vec![b'k'; MAX_SECRET_LEN + 1], 0o600);
        let err = read_secret(&big).unwrap_err();
        assert!(err.contains("is too large"), "{err}");
        assert!(err.contains("65536"), "{err}");
    }

    /// `read_secret` checks the file it opened: a link to a good file works,
    /// and a FIFO is refused without waiting for a writer.
    #[test]
    fn read_secret_follows_links_and_refuses_a_fifo() {
        let (dir, key) = secret_file(&[b'k'; 40], 0o600);
        let link = dir.path().join("link.key");
        std::os::unix::fs::symlink(&key, &link).unwrap();
        assert_eq!(read_secret(&link).unwrap(), vec![b'k'; 40]);

        let fifo = dir.path().join("fifo.key");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(read_secret(&fifo)).unwrap());
        let got = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("read_secret waited on a FIFO");
        assert!(got.unwrap_err().contains("not a regular file"));
    }

    /// The rows of the example table of "Peer secret file owner" in the
    /// peer-group spec: root and the daemon's own user own a trusted secret,
    /// nobody else does.
    // spectra-scenario: peer-group / Secret owned by root or by the checking user
    // spectra-scenario: peer-group / Check-config accepts a secret owned by root for every account / Owner and checking user
    #[test]
    fn peer_secret_owner_is_root_or_the_daemon_user() {
        for (owner, euid, ok) in [
            (0, 998, true),
            (998, 998, true),
            (1000, 998, false),
            (0, 0, true),
            (1000, 0, false),
        ] {
            assert_eq!(owner_trusted(owner, euid), ok, "owner {owner}, euid {euid}");
        }
    }

    /// A secret the daemon's own user owns is read at mode 0600 and 0640,
    /// and through a link; the owner check does not look at the group.
    // spectra-scenario: peer-group / Secret owned by root or by the checking user
    #[test]
    fn read_secret_accepts_a_secret_the_daemon_user_owns() {
        for mode in [0o600, 0o640] {
            let (dir, key) = secret_file(&[b'k'; 40], mode);
            assert_eq!(read_secret(&key).unwrap(), vec![b'k'; 40], "mode {mode:o}");
            let link = dir.path().join("link.key");
            std::os::unix::fs::symlink(&key, &link).unwrap();
            assert_eq!(
                read_secret(&link).unwrap(),
                vec![b'k'; 40],
                "link, {mode:o}"
            );
        }
    }

    /// A secret that is owned by someone else is refused with the owner's
    /// uid. Only root can give a file away, so this runs as root only (the
    /// Docker test run is root); as another user it has nothing to check.
    // spectra-scenario: peer-group / Secret owned by another user
    // spectra-scenario: peer-group / Link to a file of another owner
    #[test]
    fn read_secret_refuses_a_secret_another_user_owns() {
        if nix::unistd::geteuid().as_raw() != 0 {
            return;
        }
        // Uid 4242 is neither root nor the user running this test.
        let (dir, key) = secret_file(&[b'k'; 40], 0o640);
        std::os::unix::fs::chown(&key, Some(4242), None).unwrap();
        let err = read_secret(&key).unwrap_err();
        assert!(err.contains("owned by uid 4242"), "{err}");
        let link = dir.path().join("link.key");
        std::os::unix::fs::symlink(&key, &link).unwrap();
        assert!(
            read_secret(&link)
                .unwrap_err()
                .contains("owned by uid 4242")
        );
    }

    fn peers_conf(site: &str, peers: &str, secret: &Path) -> String {
        format!(
            "{site}[peers]\n{peers}secret_file = \"{}\"\n{SVC}",
            secret.display()
        )
    }

    /// A peer secret that group or others can write is refused, as one that
    /// others can read is.
    // spectra-scenario: peer-group / Writable secret
    #[test]
    fn writable_peer_secret_is_refused() {
        for mode in [0o620, 0o602, 0o660] {
            let (_d, key) = secret_file(&[b'k'; 64], mode);
            let site = "site = \"dc1\"\n\n";
            let e = parse_err(&peers_conf(
                site,
                "listen = \"10.0.0.5:5302\"\nmembers = [\"10.0.0.5:5302\", \"10.0.0.6:5302\"]\n",
                &key,
            ));
            assert!(
                e.iter().any(|e| e.message.contains("must not be writable")),
                "mode {mode:o}: {e:?}"
            );
        }
        let (_d, key) = secret_file(&[b'k'; 64], 0o640);
        let site = "site = \"dc1\"\n\n";
        parse_ok(&peers_conf(
            site,
            "listen = \"10.0.0.5:5302\"\nmembers = [\"10.0.0.5:5302\", \"10.0.0.6:5302\"]\n",
            &key,
        ));
    }

    /// A peer secret, and a previous one, that another user owns are refused
    /// at their own keys. Only root can give a file away, so this runs as
    /// root only (the Docker test run is root).
    // spectra-scenario: peer-group / Secret owned by another user
    // spectra-scenario: peer-group / Previous secret owned by another user
    #[test]
    fn peer_secret_owned_by_another_user_is_refused() {
        if nix::unistd::geteuid().as_raw() != 0 {
            return;
        }
        let (_d, good) = secret_file(&[b'k'; 64], 0o640);
        let (_d2, foreign) = secret_file(&[b'k'; 64], 0o640);
        std::os::unix::fs::chown(&foreign, Some(4242), None).unwrap();
        let site = "site = \"dc1\"\n\n";
        let members =
            "listen = \"10.0.0.5:5302\"\nmembers = [\"10.0.0.5:5302\", \"10.0.0.6:5302\"]\n";

        let e = parse_err(&peers_conf(site, members, &foreign));
        assert!(
            e.iter()
                .any(|e| e.path == "peers.secret_file" && e.message.contains("owned by uid 4242")),
            "{e:?}"
        );

        let previous = format!(
            "{members}previous_secret_file = \"{}\"\n",
            foreign.display()
        );
        let e = parse_err(&peers_conf(site, &previous, &good));
        assert!(
            e.iter().any(|e| e.path == "peers.previous_secret_file"
                && e.message.contains("owned by uid 4242")),
            "{e:?}"
        );
    }

    const LISTEN_MEMBERS: &str =
        "listen = \"127.0.0.1:5302\"\nmembers = [\"127.0.0.1:5302\", \"127.0.0.1:5303\"]\n";

    /// The example "accept_plaintext default" of the peer-group spec: with
    /// nothing set, plaintext is refused while `encrypt` is true (also when it
    /// is not set) and accepted while it is false; a value that is set wins,
    /// and `encrypt = false` with `accept_plaintext = false` is an error.
    // spectra-scenario: peer-group / Default of accept_plaintext follows encrypt
    // spectra-scenario: peer-group / Default of accept_plaintext follows encrypt / accept_plaintext default
    #[test]
    fn peers_accept_plaintext_default_follows_encrypt() {
        let (_d, key) = secret_file(&[b'k'; 64], 0o600);
        let site = "site = \"dc1\"\n\n";
        let rows: [(&str, bool); 6] = [
            ("", false),
            ("encrypt = true\n", false),
            ("encrypt = false\n", true),
            ("accept_plaintext = true\n", true),
            ("encrypt = true\naccept_plaintext = false\n", false),
            ("encrypt = false\naccept_plaintext = true\n", true),
        ];
        for (extra, accepts) in rows {
            let c = parse_ok(&peers_conf(site, &format!("{LISTEN_MEMBERS}{extra}"), &key));
            assert_eq!(
                c.peers.as_ref().map(|p| p.accept_plaintext),
                Some(accepts),
                "{extra:?}"
            );
        }
        let e = parse_err(&peers_conf(
            site,
            &format!("{LISTEN_MEMBERS}encrypt = false\naccept_plaintext = false\n"),
            &key,
        ));
        assert!(
            e.iter().any(|e| e.path == "peers.accept_plaintext"
                && e.message.contains("needs encrypt = true")),
            "{e:?}"
        );
    }

    /// Spec peer-group "Key order does not matter" and "what changes the digest".
    // spectra-scenario: peer-group / Key order does not matter
    // spectra-scenario: peer-group / Plugin env_pass changes the digest / what changes the digest
    #[test]
    fn check_definition_digest() {
        let svc = |extra: &str| {
            parse_ok(&format!(
                "[services.a]\ncheck = \"http\"\nbackends = [\"10.0.0.1:80\"]\n{extra}"
            ))
            .services[0]
                .digest
                .clone()
        };
        let base = svc("check_options = { path = \"/health\", expect_status = [\"200\"] }\n");
        assert_eq!(base.len(), 32);
        assert!(
            base.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert_eq!(
            base,
            svc("check_options = { expect_status = [\"200\"], path = \"/health\" }\n")
        );
        let opts = "check_options = { path = \"/health\", expect_status = [\"200\"] }\n";
        for (extra, same) in [
            ("interval = \"5s\"\n", true),
            ("rise = 3\n", true),
            ("balance = \"failover\"\n", true),
            ("timeout = \"500ms\"\n", false),
        ] {
            assert_eq!(svc(&format!("{opts}{extra}")) == base, same, "{extra}");
        }
        let more = parse_ok(&format!(
            "[services.a]\ncheck = \"http\"\nbackends = [\"10.0.0.1:80\", \"10.0.0.2:80\"]\n{opts}"
        ));
        assert_eq!(more.services[0].digest, base);
        let ready = svc("check_options = { path = \"/ready\", expect_status = [\"200\"] }\n");
        assert_ne!(ready, base);
        let tcp = parse_ok(SVC).services[0].digest.clone();
        let http = parse_ok(
            "[services.\"api.svc.emfor\"]\ncheck = \"http\"\nbackends = [\"10.0.0.1:80\"]\n",
        )
        .services[0]
            .digest
            .clone();
        assert_ne!(tcp, http);
        // The exact text hashed, so other implementations can match it.
        assert_eq!(
            check_digest("tcp", &toml::Table::new(), Duration::from_secs(1)),
            {
                use sha2::{Digest, Sha256};
                let h = Sha256::digest(br#"{"checker":"tcp","options":{},"timeout_ms":1000}"#);
                h[..16]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            }
        );
    }

    /// Spec peer-group "Valid peer group".
    // spectra-scenario: peer-group / Valid peer group
    #[test]
    fn valid_peer_group() {
        let (_d, key) = secret_file(&[b'k'; 64], 0o600);
        let c = parse_ok(&peers_conf("site = \"dc1\"\n", LISTEN_MEMBERS, &key));
        let p = c.peers.expect("[peers] is parsed");
        assert_eq!(p.listen, "127.0.0.1:5302".parse().unwrap());
        assert_eq!(
            p.others().collect::<Vec<_>>(),
            vec!["127.0.0.1:5303".parse::<SocketAddr>().unwrap()]
        );
        assert_eq!(p.secret_file, key);
        assert_eq!(p.previous_secret_file, None);
        assert_eq!(parse_ok(SVC).peers, None);
        let (_d2, old) = secret_file(&[b'o'; 64], 0o600);
        let c = parse_ok(&peers_conf(
            "site = \"dc1\"\n",
            &format!(
                "{LISTEN_MEMBERS}previous_secret_file = \"{}\"\n",
                old.display()
            ),
            &key,
        ));
        let p = c.peers.expect("[peers] is parsed");
        assert_eq!(p.previous_secret_file, Some(old));
    }

    /// Spec peer-group "peer config validation" and the service-config rows.
    // spectra-scenario: service-config / Multiple errors reported together / validation outcomes
    // spectra-scenario: peer-group / No peers table / peer config validation
    #[test]
    fn peer_config_validation() {
        let (_d, key) = secret_file(&[b'k'; 64], 0o600);
        let (_d2, short) = secret_file(
            &[b'k'; 32][..31]
                .iter()
                .chain(b"\n")
                .copied()
                .collect::<Vec<_>>(),
            0o600,
        );
        let (_d3, open) = secret_file(&[b'k'; 64], 0o644);
        let (_d4, exact) = secret_file(
            &[b'k'; 33][..32]
                .iter()
                .chain(b"\n")
                .copied()
                .collect::<Vec<_>>(),
            0o640,
        );
        let site = "site = \"dc1\"\n";
        let m2 = "members = [\"10.0.0.5:5302\", \"10.0.0.6:5302\"]\n";
        let rows: Vec<(String, &str, &str)> = vec![
            (
                peers_conf("", LISTEN_MEMBERS, &key),
                "site",
                "site is required when [peers] is set",
            ),
            (
                peers_conf(site, &format!("listen = \"0.0.0.0:5302\"\n{m2}"), &key),
                "peers.listen",
                "listen must be a specific address",
            ),
            (
                peers_conf(site, &format!("listen = \"localhost:5302\"\n{m2}"), &key),
                "peers.listen",
                "listen address must be an IP literal with port",
            ),
            (
                peers_conf(site, &format!("listen = \"10.0.0.9:5302\"\n{m2}"), &key),
                "peers.listen",
                "listen must be one of members",
            ),
            (
                peers_conf(
                    site,
                    "listen = \"10.0.0.5:5302\"\nmembers = [\"10.0.0.5:5302\"]\n",
                    &key,
                ),
                "peers.members",
                "members must list 2 to 64 addresses",
            ),
            (
                peers_conf(
                    site,
                    "listen = \"10.0.0.5:5302\"\nmembers = [\"10.0.0.5:5302\", \"10.0.0.6:5302\", \"10.0.0.6:5302\"]\n",
                    &key,
                ),
                "peers.members[2]",
                "duplicate member",
            ),
            (
                format!(
                    "{site}[peers]\nlisten = \"10.0.0.5:5302\"\n{m2}secret_file = \"peers.key\"\n{SVC}"
                ),
                "peers.secret_file",
                "must be an absolute path",
            ),
            (
                format!(
                    "{site}[peers]\nlisten = \"10.0.0.5:5302\"\n{m2}secret_file = \"/nonexistent/emfor/peers.key\"\n{SVC}"
                ),
                "peers.secret_file",
                "cannot read",
            ),
            (
                peers_conf(site, &format!("listen = \"10.0.0.5:5302\"\n{m2}"), &short),
                "peers.secret_file",
                "secret must be at least 32 bytes",
            ),
            (
                peers_conf(site, &format!("listen = \"10.0.0.5:5302\"\n{m2}"), &open),
                "peers.secret_file",
                "must not be readable by other users",
            ),
            (
                peers_conf(
                    site,
                    &format!(
                        "listen = \"10.0.0.5:5302\"\n{m2}previous_secret_file = \"{}\"\n",
                        open.display()
                    ),
                    &key,
                ),
                "peers.previous_secret_file",
                "must not be readable by other users",
            ),
            (
                peers_conf(
                    site,
                    &format!("listen = \"10.0.0.5:5302\"\n{m2}port = 5302\n"),
                    &key,
                ),
                "peers.port",
                "unknown key",
            ),
            (
                peers_conf(
                    site,
                    &format!("listen = \"10.0.0.5:5302\"\n{m2}encrypt = \"yes\"\n"),
                    &key,
                ),
                "peers.encrypt",
                "encrypt must be true or false",
            ),
            (
                peers_conf(
                    site,
                    &format!("listen = \"10.0.0.5:5302\"\n{m2}accept_plaintext = 0\n"),
                    &key,
                ),
                "peers.accept_plaintext",
                "accept_plaintext must be true or false",
            ),
            (
                peers_conf(
                    site,
                    &format!(
                        "listen = \"10.0.0.5:5302\"\n{m2}encrypt = false\naccept_plaintext = false\n"
                    ),
                    &key,
                ),
                "peers.accept_plaintext",
                "needs encrypt = true",
            ),
            (
                peers_conf(
                    site,
                    "listen = \"10.0.0.5:5302\"\nmembers = [\"10.0.0.5:5302\", \"[2001:db8::6]:5302\"]\n",
                    &key,
                ),
                "peers.members[1]",
                "same address family",
            ),
        ];
        for (text, path, message) in rows {
            let e = parse_err(&text);
            assert!(
                e.iter()
                    .any(|e| e.path == path && e.message.contains(message)),
                "{path}: {message}: {e:?}"
            );
        }
        let members = (0..65)
            .map(|i| format!("\"10.0.1.{i}:5302\""))
            .collect::<Vec<_>>();
        let e = parse_err(&peers_conf(
            site,
            &format!(
                "listen = \"10.0.1.0:5302\"\nmembers = [{}]\n",
                members.join(", ")
            ),
            &key,
        ));
        assert!(e.iter().any(|e| e.message.contains("2 to 64")), "{e:?}");
        // Exactly 32 bytes after the trailing line feed, group-readable: ok.
        let c = parse_ok(&peers_conf(
            site,
            &format!("listen = \"10.0.0.5:5302\"\n{m2}"),
            &exact,
        ));
        assert!(
            c.peers.as_ref().is_some_and(|p| p.encrypt),
            "encrypt defaults to true"
        );
        let c = parse_ok(&peers_conf(
            site,
            &format!("listen = \"10.0.0.5:5302\"\n{m2}encrypt = false\n"),
            &key,
        ));
        assert!(c.peers.as_ref().is_some_and(|p| !p.encrypt));
        assert!(
            c.peers.as_ref().is_some_and(|p| p.accept_plaintext),
            "accept_plaintext defaults to true when encrypt is false"
        );
        let c = parse_ok(&peers_conf(
            site,
            &format!("listen = \"10.0.0.5:5302\"\n{m2}accept_plaintext = false\n"),
            &key,
        ));
        assert!(c.peers.as_ref().is_some_and(|p| !p.accept_plaintext));
        // Reported together with other errors.
        let e = parse_err(&format!(
            "{}interval = \"50ms\"\n",
            peers_conf("", LISTEN_MEMBERS, &key)
        ));
        assert!(e.iter().any(|e| e.path == "site"), "{e:?}");
        assert!(
            e.iter().any(|e| e.message.contains("interval below 100ms")),
            "{e:?}"
        );
    }

    // spectra-scenario: service-config / Service inherits and overrides settings / effective settings resolution
    // spectra-scenario: backend-selection / Default balance
    #[test]
    fn effective_settings_resolution() {
        // Rows of the "effective settings resolution" example.
        let rr = Balance::RoundRobin;
        let rows: &[(&str, &str, u64, u32, Balance, bool)] = &[
            ("", "", 2, 3, rr, true),
            ("[defaults]\ninterval = \"5s\"\n", "", 5, 3, rr, true),
            (
                "[defaults]\ninterval = \"5s\"\nfall = 5\n",
                "interval = \"1s\"\n",
                1,
                5,
                rr,
                true,
            ),
            (
                "[defaults]\nbalance = \"failover\"\n",
                "",
                2,
                3,
                Balance::Failover,
                true,
            ),
            (
                "[defaults]\nbalance = \"failover\"\n",
                "balance = \"sticky\"\n",
                2,
                3,
                Balance::Sticky,
                true,
            ),
            ("[defaults]\npeers = false\n", "", 2, 3, rr, false),
            (
                "[defaults]\npeers = false\n",
                "peers = true\n",
                2,
                3,
                rr,
                true,
            ),
        ];
        for (defaults, svc_extra, interval, fall, balance, peers) in rows {
            let c = parse_ok(&format!("{defaults}{SVC}{svc_extra}"));
            let s = &c.services[0].settings;
            assert_eq!(
                s.interval,
                Duration::from_secs(*interval),
                "{defaults:?} {svc_extra:?}"
            );
            assert_eq!(s.fall, *fall);
            assert_eq!(s.balance, *balance, "{defaults:?} {svc_extra:?}");
            assert_eq!(s.peers, *peers, "{defaults:?} {svc_extra:?}");
        }
    }

    // spectra-scenario: service-config / Multiple errors reported together / validation outcomes
    #[test]
    fn peers_setting_validation() {
        // The `peers` rows of the "validation outcomes" example.
        assert!(
            !parse_ok(&format!("{SVC}peers = false\n")).services[0]
                .settings
                .peers
        );
        let e = parse_err(&format!("{SVC}peers = \"no\"\n"));
        assert_eq!(e.len(), 1, "{e:?}");
        assert_eq!(e[0].path, "services.\"api.svc.emfor\".peers");
        assert_eq!(e[0].message, "peers must be true or false");
        let e = parse_err(&format!("[defaults]\npeers = 1\n{SVC}"));
        assert_eq!(e[0].path, "defaults.peers");
        assert_eq!(e[0].message, "peers must be true or false");
    }

    // spectra-scenario: service-config / Service inherits and overrides settings
    // spectra-scenario: backend-selection / Default balance
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
        assert!(s.peers);
    }

    // spectra-scenario: backend-selection / Service overrides default
    #[test]
    fn service_balance_overrides_default() {
        let c = parse_ok(&format!(
            "[defaults]\nbalance = \"failover\"\n{SVC}balance = \"round-robin\"\n[services.b]\ncheck = \"tcp\"\nbackends = [\"10.0.0.2:80\"]\n"
        ));
        assert_eq!(c.services[0].name, "api.svc.emfor");
        assert_eq!(c.services[0].settings.balance, Balance::RoundRobin);
        assert_eq!(c.services[1].settings.balance, Balance::Failover);
    }

    /// Spec "Host site and backend weight and site", scenarios "Backup
    /// backends" and "Only backup backends", example "backup values".
    // spectra-scenario: service-config / Backup backends
    // spectra-scenario: service-config / Only backup backends
    // spectra-scenario: service-config / Only backup backends / backup values
    #[test]
    fn backup_backends() {
        let svc = |backends: &str| {
            format!("[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = {backends}\n")
        };
        let c = parse_ok(&svc(
            r#"["10.0.0.10:8080", { addr = "10.0.1.10:8080", backup = true }]"#,
        ));
        let b = &c.services[0].backends;
        assert!(!b[0].backup);
        assert!(b[1].backup);
        for (value, backup) in [
            ("", false),
            (", backup = false", false),
            (", backup = true", true),
        ] {
            let c = parse_ok(&svc(&format!(
                r#"["10.0.0.1:80", {{ addr = "10.0.1.1:80"{value} }}]"#
            )));
            assert_eq!(c.services[0].backends[1].backup, backup, "{value:?}");
        }
        let e = parse_err(&svc(
            r#"["10.0.0.1:80", { addr = "10.0.1.1:80", backup = "yes" }]"#,
        ));
        assert_eq!(e.len(), 1, "{e:?}");
        assert_eq!(e[0].path, r#"services."api.svc.emfor".backends[1].backup"#);
        assert_eq!(e[0].message, "backup must be true or false");
        let e = parse_err(&svc(r#"[{ addr = "10.0.1.10:8080", backup = true }]"#));
        assert_eq!(e.len(), 1, "{e:?}");
        assert_eq!(e[0].path, r#"services."api.svc.emfor".backends"#);
        assert_eq!(e[0].message, "at least one backend must not be a backup");
    }

    /// Spec service-config "Draining a backend": `drain` is a boolean that
    /// defaults to false, a service of only drained backends is valid, and
    /// a wrong type is an error at the key.
    // spectra-scenario: service-config / Drain value is validated
    #[test]
    fn drain_backends() {
        let svc = |backends: &str| {
            format!("[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = {backends}\n")
        };
        let c = parse_ok(&svc(
            r#"["10.0.0.10:8080", { addr = "10.0.0.11:8080", drain = true }]"#,
        ));
        let b = &c.services[0].backends;
        assert!(!b[0].drain);
        assert!(b[1].drain);
        for (value, drain) in [
            ("", false),
            (", drain = false", false),
            (", drain = true", true),
        ] {
            let c = parse_ok(&svc(&format!(
                r#"["10.0.0.1:80", {{ addr = "10.0.1.1:80"{value} }}]"#
            )));
            assert_eq!(c.services[0].backends[1].drain, drain, "{value:?}");
        }
        let e = parse_err(&svc(
            r#"["10.0.0.1:80", { addr = "10.0.1.1:80", drain = "yes" }]"#,
        ));
        assert_eq!(e.len(), 1, "{e:?}");
        assert_eq!(e[0].path, r#"services."api.svc.emfor".backends[1].drain"#);
        assert_eq!(e[0].message, "drain must be true or false");
        // Every backend drained is a valid service.
        let c = parse_ok(&svc(
            r#"[{ addr = "10.0.0.1:80", drain = true }, { addr = "10.0.0.2:80", drain = true }]"#,
        ));
        assert!(c.services[0].backends.iter().all(|b| b.drain));
    }

    /// Spec service-config "Draining a backend": drain is not part of the
    /// service definition digest, and does not restart the check.
    // spectra-scenario: service-config / Drain does not change the digest
    #[test]
    fn drain_does_not_change_digest_or_check() {
        let svc = |d: &str| {
            format!(
                "[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [{{ addr = \"10.0.0.1:80\"{d} }}]\n"
            )
        };
        let a = parse_ok(&svc(""));
        let b = parse_ok(&svc(", drain = true"));
        assert_eq!(a.services[0].digest, b.services[0].digest);
        assert!(!a.services[0].check_changed(&b.services[0]));
    }

    /// Spec "Slow start setting", scenario "Default and override" and
    /// example "slow_start validation".
    // spectra-scenario: service-config / Default and override
    // spectra-scenario: service-config / Out of range
    // spectra-scenario: service-config / Out of range / slow_start validation
    #[test]
    fn slow_start_setting() {
        let c = parse_ok(
            r#"[defaults]
slow_start = "30s"
[services."a.svc.emfor"]
check = "tcp"
backends = ["10.0.0.1:80"]
[services."b.svc.emfor"]
check = "tcp"
backends = ["10.0.0.2:80"]
slow_start = "0s"
"#,
        );
        assert_eq!(c.services[0].settings.slow_start, Duration::from_secs(30));
        assert_eq!(c.services[1].settings.slow_start, Duration::ZERO);
        let rows: &[(&str, Option<Duration>)] = &[
            ("", Some(Duration::ZERO)),
            ("slow_start = \"0s\"\n", Some(Duration::ZERO)),
            ("slow_start = \"500ms\"\n", None),
            ("slow_start = \"1s\"\n", Some(Duration::from_secs(1))),
            ("slow_start = \"30s\"\n", Some(Duration::from_secs(30))),
            ("slow_start = \"1h\"\n", Some(Duration::from_secs(3600))),
            ("slow_start = \"2h\"\n", None),
            ("slow_start = 30\n", None),
        ];
        for (line, want) in rows {
            match (parse(&format!("{SVC}{line}"), &Registry::builtin()), want) {
                (Ok(c), Some(d)) => assert_eq!(c.services[0].settings.slow_start, *d, "{line:?}"),
                (Err(e), None) => assert!(
                    e.iter()
                        .any(|e| e.path == "services.\"api.svc.emfor\".slow_start"),
                    "{line:?}: {e:?}"
                ),
                (r, _) => panic!("{line:?}: unexpected {r:?}"),
            }
        }
        let e = parse_err(&format!("{SVC}slow_start = \"2h\"\n"));
        assert_eq!(e[0].message, "slow_start must be 0s or from 1s to 1h");
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

    // spectra-scenario: service-config / Name normalization
    #[test]
    fn name_normalization() {
        let c = parse_ok(
            "[services.\"API.svc.emfor.\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n",
        );
        assert_eq!(c.services[0].name, "api.svc.emfor");
    }

    // spectra-scenario: service-config / Duplicate after normalization
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

    // spectra-scenario: service-config / Multiple errors reported together
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

    // spectra-scenario: service-config / Multiple errors reported together / validation outcomes
    // spectra-scenario: backend-selection / New methods accepted
    // spectra-scenario: backend-selection / Unknown value rejected
    #[test]
    fn validation_outcomes_table() {
        // Rows of the "validation outcomes" example: (service lines, expected message fragment or None for ok).
        let rows: &[(&str, Option<&str>)] = &[
            (
                "interval = \"50ms\"\ntimeout = \"10ms\"\n",
                Some("interval below 100ms"),
            ),
            ("interval = \"1s\"\ntimeout = \"1s\"\n", None),
            ("interval = \"24h\"\n", None),
            ("interval = \"25h\"\n", Some("interval above 24h")),
            (
                "interval = \"200years\"\nfall = 4294967295\n",
                Some("interval above 24h"),
            ),
            ("timeout = \"10ms\"\n", None),
            ("timeout = \"9ms\"\n", Some("timeout below 10ms")),
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
            ("balance = \"fastest\"\n", None),
            (
                "balance = \"random\"\n",
                Some(
                    "balance must be round-robin, sticky, failover, weighted, locality, or fastest",
                ),
            ),
            ("chekc = \"tcp\"\n", Some("unknown key")),
            ("check = \"redis\"\n", Some("unknown checker `redis`")),
            (
                "balance = \"least-conn\"\n",
                Some(
                    "balance must be round-robin, sticky, failover, weighted, locality, or fastest",
                ),
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

    // spectra-scenario: service-config / Multiple errors reported together / validation outcomes
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

    // spectra-scenario: service-config / Mixed backend forms
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

    /// LMDB refuses a map size that is not a multiple of the page size, so
    /// the config rounds it up to a multiple of 64KiB, which every common
    /// page size divides.
    #[test]
    fn map_size_rounded_up_to_64kib() {
        for (given, want) in [
            ("\"1MiB\"", 1 << 20),
            ("\"1048577\"", (1 << 20) + (64 << 10)),
            ("\"1500KiB\"", 1536 << 10),
        ] {
            let c = parse_ok(&format!("[store]\nmap_size = {given}\n{SVC}"));
            assert_eq!(c.store.map_size, want, "{given}");
            let dir = tempfile::tempdir().unwrap();
            let store = StoreConfig {
                path: dir.path().join("state.mdb"),
                ..c.store
            };
            crate::store::Store::open(&store).unwrap_or_else(|e| panic!("{given}: {e}"));
        }
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
        assert!(!c.store.sync, "sync defaults to false");
        let c = parse_ok(&format!("[store]\nsync = true\n{SVC}"));
        assert!(c.store.sync);
        let e = parse_err(&format!("[store]\nsync = \"yes\"\n{SVC}"));
        assert_eq!(e[0].path, "store.sync");
        assert_eq!(e[0].message, "must be true or false");
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

    // spectra-scenario: service-config / Multiple errors reported together / validation outcomes
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
        assert_eq!(d.rate_limit, DEFAULT_DNS_RATE_LIMIT, "rate_limit default");
        assert_eq!(DEFAULT_DNS_RATE_LIMIT, 100);
        for (text, want) in [("rate_limit = 0", 0), ("rate_limit = 5000", 5000)] {
            let c = parse_ok(&format!("[dns]\n{text}\n{SVC}"));
            assert_eq!(c.dns.unwrap().rate_limit, want, "{text}");
        }
        let rows: &[(&str, &str, &str)] = &[
            ("ttl = \"2h\"", "dns.ttl", "ttl above 3600s"),
            (
                "rate_limit = -1",
                "dns.rate_limit",
                "rate_limit must be 0 to 1000000",
            ),
            (
                "rate_limit = 1000001",
                "dns.rate_limit",
                "rate_limit must be 0 to 1000000",
            ),
            (
                "rate_limit = \"100\"",
                "dns.rate_limit",
                "must be an integer",
            ),
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
            (
                &format!("zone = \"{}\"", long_zone(MAX_ZONE_LEN + 1)),
                "dns.zone",
                "zone name longer than 242 characters",
            ),
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

    /// A valid zone name of exactly `len` characters: 60-character labels.
    pub(crate) fn long_zone(len: usize) -> String {
        let mut z = String::new();
        while z.len() < len {
            if !z.is_empty() {
                z.push('.');
            }
            let n = (len - z.len()).min(60);
            z.push_str(&"a".repeat(n));
        }
        z
    }

    /// A 230-character service name in the default zone: an IPv4 target
    /// name fits (8 + 1 + 230), an IPv6 one (39 + 1 + 230) does not, so the
    /// DNS server could never answer SRV for it.
    // spectra-scenario: service-config / Multiple errors reported together / validation outcomes
    #[test]
    fn srv_target_names_must_fit() {
        let name = format!("{}.svc.emfor", long_zone(230 - ".svc.emfor".len()));
        assert_eq!(name.len(), 230);
        let text = |dns: &str| {
            format!(
                "{dns}[services.\"{name}\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\", \"[2001:db8::1]:80\"]\n"
            )
        };
        let e = parse_err(&text("[dns]\n"));
        assert_eq!(e.len(), 1, "{e:?}");
        assert_eq!(e[0].path, format!("services.\"{name}\".backends[1]"));
        assert!(e[0].message.contains("SRV target name"), "{}", e[0]);
        // Not served by DNS: no [dns], or a name outside the zone.
        parse_ok(&text(""));
        parse_ok(&text("[dns]\nzone = \"other.zone\"\n"));
        // An IPv4 backend alone fits.
        parse_ok(&format!(
            "[dns]\n[services.\"{name}\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n"
        ));
    }

    #[test]
    fn longest_zone_is_accepted() {
        let zone = long_zone(MAX_ZONE_LEN);
        assert_eq!(zone.len(), 242);
        let d = parse_ok(&format!("[dns]\nzone = \"{zone}\"\n{SVC}"))
            .dns
            .unwrap();
        assert_eq!(d.zone, zone);
    }

    /// Spec service-config "Config validation rules": `[metrics]` rows.
    // spectra-scenario: service-config / Multiple errors reported together / validation outcomes
    #[test]
    fn metrics_table() {
        assert_eq!(parse_ok(SVC).metrics, None);
        assert_eq!(
            parse_ok(&format!("[metrics]\n{SVC}")).metrics,
            Some(MetricsConfig {
                listen: vec!["127.0.0.1:5301".parse().unwrap()]
            })
        );
        let c = parse_ok(&format!("[metrics]\nlisten = [\"[::1]:9000\"]\n{SVC}"));
        assert_eq!(c.metrics.unwrap().listen, ["[::1]:9000".parse().unwrap()]);
        let rows: &[(&str, &str, &str)] = &[
            ("listen = []", "metrics.listen", "listen must not be empty"),
            (
                "listen = [\"localhost:5301\"]",
                "metrics.listen[0]",
                "listen address must be an IP literal with port",
            ),
            (
                "listen = [\"127.0.0.1:5301\", \"127.0.0.1:5301\"]",
                "metrics.listen[1]",
                "duplicate listen address",
            ),
            ("path = \"/m\"", "metrics.path", "unknown key"),
        ];
        for (line, path, frag) in rows {
            let e = parse_err(&format!("[metrics]\n{line}\n{SVC}"));
            assert!(
                e.iter()
                    .any(|e| e.path == *path && e.message.contains(frag)),
                "{line}: {e:?}"
            );
        }
        assert_eq!(parse_metrics_listen("[services.a]\nrise = 0\n"), Some(None));
        assert_eq!(
            parse_metrics_listen("[metrics]\nlisten = [\"127.0.0.1:9\"]\n[services.a]\nrise = 0\n"),
            Some(Some(vec!["127.0.0.1:9".parse().unwrap()]))
        );
        assert_eq!(parse_metrics_listen("[metrics]\nlisten = []\n"), None);
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
    fn plugin_env_pass_is_read() {
        let (specs, errors) = parse_plugins(
            "[plugins.\"redis\"]\ncommand = [\"/bin/sh\"]\nenv_pass = [\"http_proxy\"]\n",
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(specs[0].env_pass, vec!["http_proxy".to_string()]);
    }

    // spectra-scenario: checker-plugins / Service uses a plugin / plugin declaration validation
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
            (
                "[plugins.\"redis\"]\ncommand = [\"/bin/sh\"]\nenv_pass = [\"A B\"]\n",
                Some(("plugins.redis.env_pass", "invalid variable name \"A B\"")),
            ),
            (
                "[plugins.\"redis\"]\ncommand = [\"/bin/sh\"]\nenv_pass = [\"HOME\", \"HOME\"]\n",
                Some(("plugins.redis.env_pass", "duplicate variable name \"HOME\"")),
            ),
            (
                "[plugins.\"redis\"]\ncommand = [\"/bin/sh\"]\nenv_pass = \"HOME\"\n",
                Some(("plugins.redis.env_pass", "must be an array of strings")),
            ),
            // An argument with NUL cannot be passed to the plugin.
            (
                "[plugins.\"redis\"]\ncommand = [\"/bin/sh\", \"-c\\u0000x\"]\n",
                Some(("plugins.redis.command", "must not contain NUL")),
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
                            env_pass: vec![],
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
            crate::checker::tcp::TcpFactory.validate(&toml::Table::new())
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

    /// The `EMFOR_PREFIX` rule behind `DEFAULT_CONFIG_PATH`.
    // spectra-scenario: service-config / Built with a prefix
    // spectra-scenario: service-config / Built with a prefix / prefix to default path
    #[test]
    fn prefix_to_default_config_path() {
        for (prefix, want) in [
            (None, "/etc/emfor/emfor.toml"),
            (Some(""), "/etc/emfor/emfor.toml"),
            (Some("/"), "/etc/emfor/emfor.toml"),
            (Some("/usr"), "/etc/emfor/emfor.toml"),
            (Some("/usr/"), "/etc/emfor/emfor.toml"),
            (Some("/usr/local"), "/usr/local/etc/emfor/emfor.toml"),
            (Some("/usr/local/"), "/usr/local/etc/emfor/emfor.toml"),
            (Some("/opt/emfor"), "/opt/emfor/etc/emfor/emfor.toml"),
        ] {
            let dir = prefix_dir(prefix).unwrap();
            assert_eq!(format!("{dir}{CONFIG_SUFFIX}"), want, "{prefix:?}");
        }
        let err = prefix_dir(Some("usr/local")).unwrap_err();
        assert!(err.contains("EMFOR_PREFIX"), "{err}");
    }
}

/// Config split over `include_dir`: the spec's "Config directory" requirements.
#[cfg(test)]
mod layers_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const MAIN: &str = r#"
include_dir = "conf.d"
site = "dc1"

[services."api.svc.emfor"]
check = "tcp"
backends = ["10.0.0.1:80", "10.0.0.2:80"]
interval = "2s"
"#;

    /// A main file named `emfor.toml` with `main`, and a 0755 `conf.d`
    /// holding `fragments` (mode 0644).
    fn setup(main: &str, fragments: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let conf_d = dir.path().join("conf.d");
        std::fs::create_dir(&conf_d).expect("conf.d");
        std::fs::set_permissions(&conf_d, std::fs::Permissions::from_mode(0o755)).expect("mode");
        for (name, text) in fragments {
            let p = conf_d.join(name);
            std::fs::write(&p, text).expect("fragment");
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).expect("mode");
        }
        let main_path = dir.path().join("emfor.toml");
        std::fs::write(&main_path, main).expect("main");
        // Not the mode the umask gives: the main file is checked as well.
        std::fs::set_permissions(&main_path, std::fs::Permissions::from_mode(0o644)).expect("mode");
        (dir, main_path)
    }

    fn names(layers: &Layers) -> Vec<String> {
        layers
            .files
            .iter()
            .map(|f| f.file_name().expect("name").to_string_lossy().into_owned())
            .collect()
    }

    fn errs(r: Result<Layers, Vec<ConfigError>>) -> String {
        r.expect_err("expected errors")
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn merged(base: &str, over: &str) -> toml::Table {
        let mut t: toml::Table = base.parse().expect("base");
        merge_table(
            &mut t,
            over.parse().expect("over"),
            "",
            "f",
            &mut HashMap::new(),
        );
        t
    }

    // spectra-scenario: service-config / Table merges key by key
    #[test]
    fn tables_merge_key_by_key() {
        let t = merged(
            "[s.a]\ncheck = \"tcp\"\ninterval = \"2s\"\n",
            "[s.a]\ninterval = \"500ms\"\n",
        );
        assert_eq!(t["s"]["a"]["check"].as_str(), Some("tcp"));
        assert_eq!(t["s"]["a"]["interval"].as_str(), Some("500ms"));
    }

    // spectra-scenario: service-config / List is replaced
    #[test]
    fn lists_and_mixed_types_are_replaced() {
        let t = merged(
            "b = [\"1\", \"2\"]\nx = 1\n[t]\nk = 1\n",
            "b = [\"3\"]\nx = [1]\nt = 5\n",
        );
        assert_eq!(t["b"].as_array().expect("list").len(), 1);
        assert!(t["x"].is_array());
        assert_eq!(t["t"].as_integer(), Some(5));
    }

    #[test]
    fn inline_table_merges_like_a_section() {
        let t = merged("[s.a]\nx = 1\ny = 2\n", "s.a = { y = 3 }\n");
        assert_eq!(t["s"]["a"]["x"].as_integer(), Some(1));
        assert_eq!(t["s"]["a"]["y"].as_integer(), Some(3));
    }

    // spectra-scenario: service-config / Files are read in filename order
    #[test]
    fn files_are_read_in_byte_order() {
        let (_d, main) = setup(
            MAIN,
            &[("10-b.toml", ""), ("01-a.toml", ""), ("9-c.toml", "")],
        );
        let l = read_layers(&main).expect("layers");
        assert_eq!(
            names(&l),
            ["emfor.toml", "01-a.toml", "10-b.toml", "9-c.toml"]
        );
    }

    // spectra-scenario: service-config / Unselected entries are ignored
    #[test]
    fn unselected_entries_are_ignored() {
        let (d, main) = setup(
            MAIN,
            &[
                ("a.toml", ""),
                (".hidden.toml", ""),
                ("b.toml~", ""),
                ("c.toml.disabled", ""),
            ],
        );
        std::fs::create_dir(d.path().join("conf.d/d.toml")).expect("subdir");
        let l = read_layers(&main).expect("layers");
        assert_eq!(names(&l), ["emfor.toml", "a.toml"]);
    }

    /// macOS file systems refuse a name that is not UTF-8, so only Linux
    /// and the BSDs can create one.
    // spectra-scenario: service-config / Non-UTF-8 file name
    #[cfg(target_os = "linux")]
    #[test]
    fn non_utf8_file_name_is_skipped_with_a_warning() {
        use std::os::unix::ffi::OsStrExt;
        let (d, main) = setup(MAIN, &[("a.toml", "")]);
        let bad = std::ffi::OsStr::from_bytes(b"b\xff.toml");
        std::fs::write(d.path().join("conf.d").join(bad), "").expect("bad name");
        let l = read_layers(&main).expect("layers");
        assert_eq!(names(&l), ["emfor.toml", "a.toml"]);
        assert_eq!(l.warnings.len(), 1, "{:?}", l.warnings);
        assert!(l.warnings[0].contains("not valid UTF-8"));
    }

    /// A failed load still hands back the warnings (Linux only: the name).
    // spectra-scenario: service-config / Skipped name reported by check-config and by a failed load
    #[cfg(target_os = "linux")]
    #[test]
    fn warnings_survive_a_failed_load() {
        use std::os::unix::ffi::OsStrExt;
        let (d, main) = setup(MAIN, &[("a.toml", "this is not toml")]);
        let bad = std::ffi::OsStr::from_bytes(b"b\xff.toml");
        std::fs::write(d.path().join("conf.d").join(bad), "").expect("bad name");
        let mut warnings = Vec::new();
        assert!(read_layers_with(&main, &mut warnings).is_err());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    // spectra-scenario: service-config / Symbolic link to a regular file is followed
    #[test]
    fn symlink_to_a_regular_file_is_followed() {
        let (d, main) = setup(MAIN, &[]);
        let target = d.path().join("elsewhere.txt");
        std::fs::write(&target, "site = \"dc9\"\n").expect("target");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).expect("mode");
        std::os::unix::fs::symlink(&target, d.path().join("conf.d/10-svc.toml")).expect("link");
        let l = read_layers(&main).expect("layers");
        assert_eq!(names(&l), ["emfor.toml", "10-svc.toml"]);
        assert_eq!(l.table["site"].as_str(), Some("dc9"));
    }

    // spectra-scenario: service-config / Relative include_dir
    #[test]
    fn relative_include_dir_is_relative_to_the_main_file() {
        let (_d, main) = setup(MAIN, &[("a.toml", "")]);
        let l = read_layers(&main).expect("layers");
        assert_eq!(
            l.files[1],
            main.parent().expect("dir").join("conf.d/a.toml")
        );
    }

    #[test]
    fn no_include_dir_reads_the_main_file_only() {
        let (_d, main) = setup("site = \"dc1\"\n", &[("a.toml", "site = \"x\"\n")]);
        let l = read_layers(&main).expect("layers");
        assert!(!l.included && l.origins.is_empty());
        assert_eq!(l.files.len(), 1);
        assert_eq!(l.table["site"].as_str(), Some("dc1"));
    }

    // spectra-scenario: service-config / Table merges key by key
    #[test]
    fn later_fragment_overrides_the_main_file() {
        let (_d, main) = setup(
            MAIN,
            &[(
                "99-local.toml",
                "[services.\"api.svc.emfor\"]\ninterval = \"5s\"\n",
            )],
        );
        let l = read_layers(&main).expect("layers");
        let c = parse_layers(&l, &Registry::builtin()).expect("config");
        let s = &c.services[0];
        assert_eq!(s.settings.interval, Duration::from_secs(5));
        assert_eq!(s.backends.len(), 2);
    }

    // spectra-scenario: service-config / List is replaced
    #[test]
    fn backends_list_is_replaced() {
        let (_d, main) = setup(
            MAIN,
            &[(
                "99.toml",
                "[services.\"api.svc.emfor\"]\nbackends = [\"10.0.0.3:80\"]\n",
            )],
        );
        let c = parse_layers(&read_layers(&main).expect("layers"), &Registry::builtin())
            .expect("config");
        assert_eq!(c.services[0].backends.len(), 1);
        assert_eq!(c.services[0].backends[0].id, "10.0.0.3:80");
    }

    // spectra-scenario: service-config / Store in a fragment
    // spectra-scenario: service-config / Nested include
    #[test]
    fn store_and_include_dir_are_main_only() {
        let (_d, main) = setup(MAIN, &[("10-store.toml", "[store]\npath = \"/x\"\n")]);
        assert!(errs(read_layers(&main)).contains("10-store.toml: store is only allowed"));
        let (_d, main) = setup(MAIN, &[("20-inc.toml", "include_dir = \"x\"\n")]);
        assert!(errs(read_layers(&main)).contains("20-inc.toml: include_dir is only allowed"));
    }

    // spectra-scenario: service-config / status and lookup need only the main file
    #[test]
    fn load_store_reads_only_the_main_file() {
        let (d, main) = setup(MAIN, &[("10.toml", "this is not toml")]);
        std::fs::set_permissions(
            d.path().join("conf.d/10.toml"),
            std::fs::Permissions::from_mode(0o000),
        )
        .expect("mode");
        assert!(load_store(&main).is_ok());
    }

    // spectra-scenario: service-config / Group-writable fragment
    #[test]
    fn group_writable_fragment_is_refused() {
        let (d, main) = setup(MAIN, &[("10-a.toml", "")]);
        let f = d.path().join("conf.d/10-a.toml");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o664)).expect("mode");
        let e = errs(read_layers(&main));
        assert!(e.contains("10-a.toml") && e.contains("0664"), "{e}");
    }

    /// A link is judged by the file it points to, not by the link.
    // spectra-scenario: service-config / Link to a file that is not trusted
    #[test]
    fn symlink_to_a_group_writable_file_is_refused() {
        let (d, main) = setup(MAIN, &[]);
        let target = d.path().join("elsewhere.txt");
        std::fs::write(&target, "site = \"dc9\"\n").expect("target");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o664)).expect("mode");
        std::os::unix::fs::symlink(&target, d.path().join("conf.d/10-svc.toml")).expect("link");
        let e = errs(read_layers(&main));
        assert!(e.contains("10-svc.toml") && e.contains("0664"), "{e}");
    }

    /// A fragment owned by someone else is refused, whatever its mode. Only
    /// root can give a file away, so this runs in the Debian container.
    // spectra-scenario: service-config / Fragment owned by another user
    // spectra-scenario: service-config / Link to a file that is not trusted
    // spectra-scenario: service-config / Directory owned by another user
    #[test]
    fn foreign_owner_is_refused() {
        if nix::unistd::geteuid().as_raw() != 0 {
            return;
        }
        let (d, main) = setup(MAIN, &[("10-a.toml", "")]);
        let frag = d.path().join("conf.d/10-a.toml");
        std::os::unix::fs::chown(&frag, Some(12345), None).expect("chown");
        let e = errs(read_layers(&main));
        assert!(
            e.contains("10-a.toml") && e.contains("owned by uid 12345"),
            "{e}"
        );
        // A link to such a file is refused too.
        std::fs::remove_file(&frag).expect("remove");
        let target = d.path().join("elsewhere.txt");
        std::fs::write(&target, "").expect("target");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).expect("mode");
        std::os::unix::fs::chown(&target, Some(12345), None).expect("chown");
        std::os::unix::fs::symlink(&target, &frag).expect("link");
        assert!(errs(read_layers(&main)).contains("owned by uid 12345"));
        // And a directory owned by someone else.
        std::fs::remove_file(&frag).expect("remove");
        std::os::unix::fs::chown(d.path().join("conf.d"), Some(12345), None).expect("chown");
        assert!(errs(read_layers(&main)).contains("conf.d"));
    }

    /// Spec "Config directory permissions": the owner must be root or the
    /// user running emfor, whatever the mode.
    // spectra-scenario: service-config / Fragment owned by another user
    #[test]
    fn owner_must_be_root_or_the_daemon_user() {
        assert_eq!(untrusted(0o100644, 0, 1000), None);
        assert_eq!(untrusted(0o100644, 1000, 1000), None);
        assert_eq!(untrusted(0o100640, 0, 0), None);
        let why = untrusted(0o100644, 1001, 1000).expect("foreign owner");
        assert!(
            why.contains("owned by uid 1001") && why.contains("uid 1000"),
            "{why}"
        );
        let why = untrusted(0o100664, 0, 0).expect("group writable");
        assert!(why.contains("0664"), "{why}");
        assert!(untrusted(0o040777, 0, 0).is_some());
    }

    // spectra-scenario: service-config / World-writable directory
    #[test]
    fn world_writable_directory_is_refused() {
        let (d, main) = setup(MAIN, &[]);
        std::fs::set_permissions(
            d.path().join("conf.d"),
            std::fs::Permissions::from_mode(0o777),
        )
        .expect("mode");
        let e = errs(read_layers(&main));
        assert!(e.contains("conf.d") && e.contains("0777"), "{e}");
    }

    /// Spec "Config directory permissions", "Main file modes": the main file
    /// is refused when group or others can write it, whatever else it holds.
    // spectra-scenario: service-config / Main file that others can write
    // spectra-scenario: service-config / Main file that others can write / Main file modes
    #[test]
    fn main_file_modes() {
        let (_d, main) = setup("site = \"dc1\"\n", &[]);
        for (mode, ok) in [
            (0o600, true),
            (0o640, true),
            (0o644, true),
            (0o604, true),
            (0o620, false),
            (0o602, false),
            (0o664, false),
            (0o666, false),
        ] {
            std::fs::set_permissions(&main, std::fs::Permissions::from_mode(mode)).expect("mode");
            let r = read_layers(&main);
            if ok {
                assert!(r.is_ok(), "mode {mode:04o} must load");
            } else {
                let e = errs(r);
                assert!(
                    e.contains(&main.display().to_string())
                        && e.contains(&format!("mode {mode:04o}"))
                        && e.contains(&format!("chmod g-w,o-w {}", main.display())),
                    "mode {mode:04o}: {e}"
                );
            }
        }
    }

    /// A refused main file is not parsed: a syntax error in it does not
    /// show before the mode does.
    #[test]
    fn main_file_mode_is_checked_before_the_content() {
        let (_d, main) = setup("this is not toml\n", &[]);
        std::fs::set_permissions(&main, std::fs::Permissions::from_mode(0o664)).expect("mode");
        let e = errs(read_layers(&main));
        assert!(e.contains("0664") && !e.contains("invalid TOML"), "{e}");
    }

    /// A link is judged by the file it points to.
    // spectra-scenario: service-config / Link to a main file that is not trusted
    #[test]
    fn main_file_link_to_a_file_others_can_write_is_refused() {
        let (d, main) = setup("site = \"dc1\"\n", &[]);
        std::fs::set_permissions(&main, std::fs::Permissions::from_mode(0o664)).expect("mode");
        let link = d.path().join("link.toml");
        std::os::unix::fs::symlink(&main, &link).expect("link");
        let e = errs(read_layers(&link));
        assert!(
            e.contains(&link.display().to_string()) && e.contains("0664"),
            "{e}"
        );
    }

    /// The main file owned by someone else is refused, whatever its mode,
    /// with the command that fixes it. Only root can give a file away, so
    /// this runs in the Debian container.
    // spectra-scenario: service-config / Main file owned by another user
    #[test]
    fn main_file_foreign_owner_is_refused() {
        if nix::unistd::geteuid().as_raw() != 0 {
            return;
        }
        let (_d, main) = setup("site = \"dc1\"\n", &[]);
        std::os::unix::fs::chown(&main, Some(12345), None).expect("chown");
        let e = errs(read_layers(&main));
        assert!(
            e.contains(&main.display().to_string())
                && e.contains("owned by uid 12345")
                && e.contains(&format!("chown root {}", main.display())),
            "{e}"
        );
    }

    /// `status` and `lookup` read only `[store]` and do not check the main
    /// file (spec "Config directory permissions").
    // spectra-scenario: service-config / status and lookup do not check the main file
    #[test]
    fn load_store_does_not_check_the_main_file() {
        let (_d, main) = setup("[store]\npath = \"/tmp/x.mdb\"\n", &[]);
        std::fs::set_permissions(&main, std::fs::Permissions::from_mode(0o664)).expect("mode");
        assert!(load_store(&main).is_ok());
    }

    #[test]
    fn missing_main_file_cannot_be_read() {
        let (d, _main) = setup("", &[]);
        let e = errs(read_layers(&d.path().join("absent.toml")));
        assert!(
            e.contains("cannot read") && e.contains("absent.toml"),
            "{e}"
        );
    }

    // spectra-scenario: service-config / Missing directory at start
    #[test]
    fn missing_directory_is_an_error() {
        let (d, main) = setup(MAIN, &[]);
        std::fs::remove_dir(d.path().join("conf.d")).expect("remove");
        let e = errs(read_layers(&main));
        assert!(e.starts_with("include_dir: "), "{e}");
    }

    #[test]
    fn include_dir_that_is_a_file_is_an_error() {
        let (d, main) = setup(MAIN, &[]);
        std::fs::remove_dir(d.path().join("conf.d")).expect("remove");
        std::fs::write(d.path().join("conf.d"), "").expect("file");
        assert!(errs(read_layers(&main)).contains("not a directory"));
    }

    // spectra-scenario: service-config / Empty include_dir
    #[test]
    fn empty_include_dir_is_an_error() {
        let (_d, main) = setup("include_dir = \"\"\n", &[]);
        assert_eq!(errs(read_layers(&main)), "include_dir: must not be empty");
    }

    /// A link that points at itself is neither missing nor a file: the error
    /// must not be swallowed like a dangling link (EACCES cannot be tested
    /// as root).
    // spectra-scenario: service-config / Dangling link is skipped, unreadable link is an error
    #[test]
    fn unreadable_fragment_link_is_an_error() {
        let (d, main) = setup(MAIN, &[]);
        let link = d.path().join("conf.d/loop.toml");
        std::os::unix::fs::symlink(&link, &link).expect("link");
        assert!(errs(read_layers(&main)).contains("loop.toml"));
        let dangling = d.path().join("conf.d/loop.toml");
        std::fs::remove_file(&dangling).expect("remove");
        std::os::unix::fs::symlink(d.path().join("nowhere"), &dangling).expect("link");
        assert!(read_layers(&main).is_ok());
    }

    #[test]
    fn include_dir_must_be_a_string() {
        let (_d, main) = setup("include_dir = 3\n", &[]);
        assert_eq!(errs(read_layers(&main)), "include_dir: must be a string");
    }

    // spectra-scenario: service-config / Syntax error in a fragment
    #[test]
    fn syntax_error_names_file_line_and_column() {
        let (_d, main) = setup(MAIN, &[("20-x.toml", "a = 1\nb = 2\nc = = 3\n")]);
        let e = errs(read_layers(&main));
        assert!(e.contains("20-x.toml:3:5: invalid TOML"), "{e}");
    }

    // spectra-scenario: service-config / Invalid value in a fragment
    #[test]
    fn invalid_value_names_key_path_and_file() {
        let (_d, main) = setup(
            MAIN,
            &[(
                "30-y.toml",
                "[services.\"api.svc.emfor\"]\ninterval = \"fast\"\n",
            )],
        );
        let l = read_layers(&main).expect("layers");
        let e = parse_layers(&l, &Registry::builtin()).expect_err("invalid");
        let text = e[0].to_string();
        assert!(
            text.starts_with("services.\"api.svc.emfor\".interval: ") && text.contains("30-y.toml"),
            "{text}"
        );
    }

    #[test]
    fn error_in_the_main_file_names_the_main_file() {
        let (_d, main) = setup(
            &MAIN.replace("\"tcp\"", "\"nope\""),
            &[("30-y.toml", "site = \"dc2\"\n")],
        );
        let l = read_layers(&main).expect("layers");
        let e = parse_layers(&l, &Registry::builtin()).expect_err("invalid");
        assert!(e[0].to_string().contains("emfor.toml"), "{}", e[0]);
    }

    #[test]
    fn single_file_errors_carry_no_file() {
        let e = parse_err_text(
            "[services.\"a.svc.emfor\"]\ncheck = \"nope\"\nbackends = [\"1.2.3.4:80\"]\n",
        );
        assert!(e.iter().all(|e| e.file.is_none()));
    }

    fn parse_err_text(text: &str) -> Vec<ConfigError> {
        parse(text, &Registry::builtin()).expect_err("invalid")
    }

    // spectra-scenario: service-config / Plugin declared in a fragment
    #[test]
    fn plugin_in_a_fragment_is_declared() {
        let (_d, main) = setup(
            MAIN,
            &[("10-p.toml", "[plugins.probe]\ncommand = [\"/bin/sh\"]\n")],
        );
        let l = read_layers(&main).expect("layers");
        let (specs, errors) = plugins_in(&l.table);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "probe");
    }

    // spectra-scenario: service-config / Fragment changes a listen address
    #[test]
    fn listen_change_in_a_fragment_is_seen() {
        let (_d, main) = setup(
            MAIN,
            &[("10-dns.toml", "[dns]\nlisten = [\"127.0.0.1:5353\"]\n")],
        );
        let l = read_layers(&main).expect("layers");
        let listen = dns_listen_in(&l.table).expect("valid").expect("dns table");
        assert_eq!(listen, vec!["127.0.0.1:5353".parse().expect("addr")]);
    }

    /// Spec peer-group "Plugin env_pass changes the digest".
    // spectra-scenario: peer-group / Plugin env_pass changes the digest
    // spectra-scenario: peer-group / Plugin env_pass changes the digest / what changes the digest
    #[test]
    fn env_pass_is_part_of_the_check_digest() {
        let t = Duration::from_secs(1);
        let options = |pass: Option<&str>| {
            let mut o = toml::Table::new();
            o.insert("command".into(), toml::Value::Array(vec!["/bin/sh".into()]));
            if let Some(name) = pass {
                o.insert("env_pass".into(), toml::Value::Array(vec![name.into()]));
            }
            o
        };
        // `exec`: `env_pass` is a check option.
        assert_ne!(
            check_digest("exec", &options(None), t),
            check_digest("exec", &options(Some("HOME")), t)
        );
        assert_eq!(
            check_digest("exec", &options(Some("HOME")), t),
            check_digest("exec", &options(Some("HOME")), t)
        );
        // A plugin: `env_pass` is part of the checker identity.
        let command = ["/usr/local/libexec/emfor-check".to_string()];
        let id = |pass: &[String]| crate::plugin::plugin_checker_id("redis", &command, pass);
        let none = toml::Table::new();
        let proxy = ["http_proxy".to_string()];
        assert_ne!(
            check_digest(&id(&[]), &none, t),
            check_digest(&id(&proxy), &none, t)
        );
        assert_eq!(
            check_digest(&id(&proxy), &none, t),
            check_digest(&id(&proxy), &none, t)
        );
    }

    // spectra-scenario: service-config / Peer digest does not depend on file splitting
    #[test]
    fn digest_does_not_depend_on_splitting() {
        let one = "site = \"dc1\"\ninclude_dir = \"conf.d\"\n[services.\"api.svc.emfor\"]\ncheck = \"http\"\nbackends = [\"10.0.0.1:80\"]\ntimeout = \"500ms\"\n[services.\"api.svc.emfor\".check_options]\npath = \"/h\"\n";
        let (_d1, m1) = setup(one, &[]);
        let (_d2, m2) = setup(
            "site = \"dc1\"\ninclude_dir = \"conf.d\"\n[services.\"api.svc.emfor\"]\ncheck = \"http\"\nbackends = [\"10.0.0.1:80\"]\n",
            &[
                (
                    "10.toml",
                    "[services.\"api.svc.emfor\"]\ntimeout = \"500ms\"\n",
                ),
                (
                    "20.toml",
                    "[services.\"api.svc.emfor\".check_options]\npath = \"/h\"\n",
                ),
            ],
        );
        let r = Registry::builtin();
        let a = parse_layers(&read_layers(&m1).expect("one"), &r).expect("one");
        let b = parse_layers(&read_layers(&m2).expect("split"), &r).expect("split");
        assert_eq!(a.services[0].digest, b.services[0].digest);
    }

    fn shown(main: &str, fragments: &[(&str, &str)]) -> (String, Layers, tempfile::TempDir) {
        let (d, path) = setup(main, fragments);
        let l = read_layers(&path).expect("layers");
        (show_layers(&l), l, d)
    }

    /// Spec "Showing the merged config", scenario "Round trip": the text
    /// parses to the merged table without `include_dir`, whatever the keys and
    /// values look like.
    // spectra-scenario: service-config / Round trip
    #[test]
    fn show_round_trips_the_merged_table() {
        let main = "include_dir = \"conf.d\"\nsite = \"dc#1\"\n\n[dns]\n\n[services.\"api.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\", { addr = \"10.0.0.2:80\", weight = 3 }]\n[services.\"api.svc.emfor\".check_options]\npath = \"/a # b\"\nnote = \"line one\\nline two\"\n\"a\\\"b\" = 1\n\"c\\\\d\" = 2\n[services.\"api.svc.emfor\".check_options.\"q\\\"r\"]\nx = 1\n";
        let (text, l, _d) = shown(
            main,
            &[(
                "10.toml",
                "[services.\"api.svc.emfor\"]\ninterval = \"5s\"\n",
            )],
        );
        let parsed: toml::Table = text.parse().unwrap_or_else(|e| panic!("{e}\n{text}"));
        let mut merged = l.table.clone();
        merged.remove("include_dir");
        assert_eq!(parsed, merged, "{text}");
        assert!(!parsed.contains_key("include_dir"));
        // A value with a line break is a multi-line string; its comment follows
        // the closing quotes.
        assert!(text.contains("line two\"\"\" # "), "{text}");
        assert!(text.contains("# include_dir = \"conf.d\" (its files are merged below)"));
        let (text, l, _d) = shown(
            "site = \"dc1\"\n[services.a]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n",
            &[],
        );
        assert_eq!(
            text.parse::<toml::Table>().expect("valid"),
            l.table,
            "{text}"
        );
    }

    // spectra-scenario: service-config / Valid config with fragments
    #[test]
    fn show_names_the_file_that_set_each_value() {
        let (text, _l, d) = shown(
            MAIN,
            &[(
                "99-local.toml",
                "[services.\"api.svc.emfor\"]\ninterval = \"5s\"\nbackends = [\"10.0.0.3:80\"]\n",
            )],
        );
        let main = d.path().join("emfor.toml");
        let frag = d.path().join("conf.d/99-local.toml");
        let line = |key: &str| {
            text.lines()
                .find(|l| l.starts_with(key))
                .unwrap_or_else(|| panic!("{key}\n{text}"))
                .to_string()
        };
        assert_eq!(
            line("interval"),
            format!("interval = \"5s\" # {}", frag.display())
        );
        assert_eq!(
            line("backends"),
            format!("backends = [\"10.0.0.3:80\"] # {}", frag.display())
        );
        assert_eq!(
            line("check"),
            format!("check = \"tcp\" # {}", main.display())
        );
        assert!(text.contains("[services.\"api.svc.emfor\"]"), "{text}");
    }

    // spectra-scenario: service-config / Without include_dir
    #[test]
    fn show_without_include_dir_has_one_comment() {
        let (text, _l, d) = shown(
            "site = \"dc1\"\n[services.a]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n",
            &[],
        );
        let comments: Vec<&str> = text.lines().filter(|l| l.contains('#')).collect();
        assert_eq!(
            comments,
            [format!(
                "# merged from {}",
                d.path().join("emfor.toml").display()
            )]
        );
        assert!(text.starts_with("# merged from "));
    }

    /// A file name with a line break must not turn the rest of the name into
    /// TOML (Linux and the BSDs allow it in a name).
    #[test]
    fn show_escapes_control_characters_in_comments() {
        assert_eq!(
            comment_safe("a\nsite = \"x\"\n#"),
            "a\\u000asite = \"x\"\\u000a#"
        );
        assert_eq!(
            comment_safe("/etc/emfor/emfor.toml"),
            "/etc/emfor/emfor.toml"
        );
    }

    // spectra-scenario: service-config / Defaults are not shown
    #[test]
    fn show_has_no_defaults() {
        let (text, _l, _d) = shown(MAIN, &[]);
        assert!(
            !text
                .lines()
                .any(|l| l.starts_with("timeout") || l.starts_with("rise")),
            "{text}"
        );
    }

    #[test]
    fn parent_path_walks_up_keys_and_indexes() {
        assert_eq!(
            parent_path("services.\"a.b\".backends[0]"),
            Some("services.\"a.b\".backends")
        );
        assert_eq!(
            parent_path("services.\"a.b\".backends"),
            Some("services.\"a.b\"")
        );
        assert_eq!(parent_path("services.\"a.b\""), Some("services"));
        assert_eq!(parent_path("services"), None);
    }
}
