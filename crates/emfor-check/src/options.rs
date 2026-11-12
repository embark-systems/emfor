//! `check_options` shared by the redis, mysql, pgsql and ldap plugins.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use emfor_plugin::{OptionError, Options};
use serde_json::Value;

use crate::Kind;

/// Options every plugin accepts (`role` is not one of ldap's).
const COMMON: &[&str] = &[
    "user",
    "password_file",
    "role",
    "tls",
    "tls_verify",
    "server_name",
    "ca_file",
];

/// Role a backend has to have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// redis `master`.
    Master,
    /// redis `replica` (`ROLE` says `slave`).
    Replica,
    /// mysql or pgsql primary (writable, not in recovery).
    Primary,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Master => "master",
            Role::Replica => "replica",
            Role::Primary => "primary",
        }
    }
}

/// TLS settings; present only with `tls = true` (or, for ldap,
/// `starttls = true`).
pub use emfor_tls::TlsSettings;

/// Validated `check_options`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    pub user: Option<String>,
    pub password_file: Option<PathBuf>,
    pub role: Option<Role>,
    pub tls: Option<TlsSettings>,
    /// ldap: upgrade a plaintext connection with StartTLS (`tls` then holds
    /// the TLS settings).
    pub starttls: bool,
    pub database: Option<String>,
}

/// Parse and validate a service's `check_options` for one plugin, as the
/// daemon's `validate` request asks.
///
/// Reads `password_file` and `ca_file` to check that they are usable.
pub fn parse(kind: Kind, options: &Options) -> Result<Settings, Vec<OptionError>> {
    parse_with(kind, options, true)
}

/// Turn options that already passed [`parse`] into settings for one check,
/// without reading any file: the check reads `password_file` and `ca_file`
/// itself and reports an unreadable file with its own reason.
pub fn settings(kind: Kind, options: &Options) -> Result<Settings, Vec<OptionError>> {
    parse_with(kind, options, false)
}

fn parse_with(
    kind: Kind,
    options: &Options,
    read_files: bool,
) -> Result<Settings, Vec<OptionError>> {
    let mut errors = Vec::new();
    for key in options.keys() {
        let known = (COMMON.contains(&key.as_str()) && !(kind == Kind::Ldap && key == "role"))
            || (kind == Kind::Pgsql && key == "database")
            || (kind == Kind::Ldap && key == "starttls");
        if !known {
            errors.push(OptionError::new(key, "unknown key"));
        }
    }

    let user = non_empty_string(options, "user", &mut errors);
    let database = if kind == Kind::Pgsql {
        non_empty_string(options, "database", &mut errors)
    } else {
        None
    };
    let required: &[(&str, &Option<String>)] = match kind {
        Kind::Redis | Kind::Ldap => &[],
        Kind::Mysql => &[("user", &user)],
        Kind::Pgsql => &[("user", &user), ("database", &database)],
    };
    for (key, value) in required {
        if value.is_none() && !options.contains_key(*key) {
            errors.push(OptionError::new(*key, "missing required key"));
        }
    }

    let password_file = match options.get("password_file") {
        None => None,
        Some(Value::String(p)) => match check_file(Path::new(p), read_files, check_password_file) {
            Ok(()) => Some(PathBuf::from(p)),
            Err(message) => {
                errors.push(OptionError::new("password_file", message));
                None
            }
        },
        Some(_) => {
            errors.push(OptionError::new("password_file", "must be a string"));
            None
        }
    };
    if kind == Kind::Redis && user.is_some() && !options.contains_key("password_file") {
        errors.push(OptionError::new("user", "requires password_file"));
    }
    // An ldap bind with a DN and no password is an "unauthenticated bind",
    // which servers often accept like an anonymous one.
    if kind == Kind::Ldap {
        if options.contains_key("user") && !options.contains_key("password_file") {
            errors.push(OptionError::new("user", "requires password_file"));
        }
        if options.contains_key("password_file") && !options.contains_key("user") {
            errors.push(OptionError::new("password_file", "requires user"));
        }
    }

    let role = match options.get("role") {
        // ldap has no role; the key was reported as unknown.
        None => None,
        Some(_) if kind == Kind::Ldap => None,
        Some(v) => {
            let role = match (kind, v.as_str()) {
                (Kind::Redis, Some("master")) => Some(Role::Master),
                (Kind::Redis, Some("replica")) => Some(Role::Replica),
                (Kind::Mysql | Kind::Pgsql, Some("primary")) => Some(Role::Primary),
                _ => None,
            };
            if role.is_none() {
                let message = match kind {
                    Kind::Redis => "must be one of master, replica",
                    Kind::Mysql | Kind::Pgsql => "must be primary",
                    Kind::Ldap => unreachable!("ldap role is skipped above"),
                };
                errors.push(OptionError::new("role", message));
            }
            role
        }
    };

    let tls_on = boolean(options, "tls", false, &mut errors);
    let starttls = kind == Kind::Ldap && boolean(options, "starttls", false, &mut errors);
    if tls_on && starttls {
        errors.push(OptionError::new("starttls", "conflicts with tls"));
    }
    let verify = boolean(options, "tls_verify", true, &mut errors);
    let server_name = non_empty_string(options, "server_name", &mut errors);
    let server_name = match server_name {
        Some(name) if let Err(message) = emfor_tls::check_server_name(&name) => {
            errors.push(OptionError::new("server_name", message));
            None
        }
        other => other,
    };
    let ca_file = match options.get("ca_file") {
        None => None,
        Some(Value::String(p)) => match check_file(Path::new(p), read_files, check_ca_file) {
            Ok(()) => Some(PathBuf::from(p)),
            Err(message) => {
                errors.push(OptionError::new("ca_file", message));
                None
            }
        },
        Some(_) => {
            errors.push(OptionError::new("ca_file", "must be a string"));
            None
        }
    };
    if !tls_on && !starttls {
        let message = if kind == Kind::Ldap {
            "requires tls = true or starttls = true"
        } else {
            "requires tls = true"
        };
        for key in ["tls_verify", "server_name", "ca_file"] {
            if options.contains_key(key) {
                errors.push(OptionError::new(key, message));
            }
        }
    }

    if errors.is_empty() {
        Ok(Settings {
            user,
            password_file,
            role,
            starttls,
            tls: (tls_on || starttls).then_some(TlsSettings {
                verify,
                server_name,
                ca_file,
            }),
            database,
        })
    } else {
        Err(errors)
    }
}

fn non_empty_string(options: &Options, key: &str, errors: &mut Vec<OptionError>) -> Option<String> {
    match options.get(key) {
        None => None,
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(_) => {
            errors.push(OptionError::new(key, "must be a non-empty string"));
            None
        }
    }
}

fn boolean(options: &Options, key: &str, default: bool, errors: &mut Vec<OptionError>) -> bool {
    match options.get(key) {
        None => default,
        Some(Value::Bool(b)) => *b,
        Some(_) => {
            errors.push(OptionError::new(key, "must be a boolean"));
            default
        }
    }
}

/// Check a file option: always that the path is absolute, and with
/// `read_files` also its content.
fn check_file(
    path: &Path,
    read_files: bool,
    check_content: fn(&Path) -> Result<(), String>,
) -> Result<(), String> {
    if !read_files {
        return if path.is_absolute() {
            Ok(())
        } else {
            Err("not an absolute path".into())
        };
    }
    check_content(path)
}

/// Read a file named by an option, with the option's error messages.
fn read_option_file(path: &Path) -> Result<Vec<u8>, String> {
    if !path.is_absolute() {
        return Err("not an absolute path".into());
    }
    std::fs::read(path).map_err(|e| match e.kind() {
        ErrorKind::NotFound => "file not found".into(),
        kind => format!("cannot read: {kind}"),
    })
}

fn check_password_file(path: &Path) -> Result<(), String> {
    let bytes = read_option_file(path)?;
    let password = trim_newline(&bytes);
    if password.is_empty() {
        return Err("file is empty".into());
    }
    if std::str::from_utf8(password).is_err() {
        return Err("not valid UTF-8".into());
    }
    Ok(())
}

fn check_ca_file(path: &Path) -> Result<(), String> {
    emfor_tls::check_ca_file(path)
}

/// Remove one trailing `\n` or `\r\n`.
fn trim_newline(bytes: &[u8]) -> &[u8] {
    bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(bytes)
}

/// Read the password for one check. The error is the check's `error`
/// reason; it names the I/O error kind and never the file's content.
pub fn read_password(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("password_file: {}", e.kind()))?;
    let password = trim_newline(&bytes);
    if password.is_empty() {
        return Err("password_file: file is empty".into());
    }
    String::from_utf8(password.to_vec()).map_err(|_| "password_file: not valid UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Files {
        dir: tempfile::TempDir,
    }

    impl Files {
        fn new() -> Files {
            Files {
                dir: tempfile::tempdir().unwrap(),
            }
        }
        fn write(&self, name: &str, content: &[u8]) -> String {
            let path = self.dir.path().join(name);
            std::fs::write(&path, content).unwrap();
            path.to_str().unwrap().to_owned()
        }
        fn missing(&self) -> String {
            self.dir.path().join("missing").to_str().unwrap().to_owned()
        }
    }

    fn errors(kind: Kind, options: Value) -> Vec<(String, String)> {
        let Value::Object(map) = options else {
            panic!("options must be an object")
        };
        match parse(kind, &map) {
            Ok(_) => Vec::new(),
            Err(errs) => errs.into_iter().map(|e| (e.key, e.message)).collect(),
        }
    }

    type Errors = Vec<(String, String)>;

    fn one(key: &str, message: &str) -> Errors {
        vec![(key.to_owned(), message.to_owned())]
    }

    /// A self-signed certificate in PEM form (any valid certificate works).
    const CERT_PEM: &str = include_str!("../tests/data/test-ca.pem");

    #[test]
    fn option_validation_table() {
        let f = Files::new();
        let empty = f.write("empty", b"\n");
        let missing = f.missing();
        let not_pem = f.write("not-pem.txt", b"hello\n");
        let pass = f.write("ldap.pass", b"s3cret\n");
        let rows: Vec<(Kind, Value, Errors)> = vec![
            (Kind::Mysql, json!({}), one("user", "missing required key")),
            (
                Kind::Pgsql,
                json!({"user": "emfor"}),
                one("database", "missing required key"),
            ),
            (
                Kind::Redis,
                json!({"database": "0"}),
                one("database", "unknown key"),
            ),
            (
                Kind::Redis,
                json!({"role": "primary"}),
                one("role", "must be one of master, replica"),
            ),
            (
                Kind::Mysql,
                json!({"user": "emfor", "role": "master"}),
                one("role", "must be primary"),
            ),
            (
                Kind::Redis,
                json!({"user": "emfor"}),
                one("user", "requires password_file"),
            ),
            (
                Kind::Redis,
                json!({"password_file": "redis.pass"}),
                one("password_file", "not an absolute path"),
            ),
            (
                Kind::Redis,
                json!({"password_file": missing}),
                one("password_file", "file not found"),
            ),
            (
                Kind::Redis,
                json!({"password_file": empty}),
                one("password_file", "file is empty"),
            ),
            (
                Kind::Redis,
                json!({"server_name": "redis.internal"}),
                one("server_name", "requires tls = true"),
            ),
            (
                Kind::Redis,
                json!({"tls": "yes"}),
                one("tls", "must be a boolean"),
            ),
            (
                Kind::Redis,
                json!({"tls": true, "ca_file": not_pem}),
                one("ca_file", "no certificate found"),
            ),
            (
                Kind::Redis,
                json!({"timeout": 1}),
                one("timeout", "unknown key"),
            ),
            (
                Kind::Redis,
                json!({"starttls": true}),
                one("starttls", "unknown key"),
            ),
            (Kind::Ldap, json!({}), Vec::new()),
            (
                Kind::Ldap,
                json!({"user": "cn=monitor,dc=example,dc=org"}),
                one("user", "requires password_file"),
            ),
            (
                Kind::Ldap,
                json!({"password_file": pass}),
                one("password_file", "requires user"),
            ),
            (
                Kind::Ldap,
                json!({"tls": true, "starttls": true}),
                one("starttls", "conflicts with tls"),
            ),
            (
                Kind::Ldap,
                json!({"server_name": "ldap.internal"}),
                one("server_name", "requires tls = true or starttls = true"),
            ),
            (
                Kind::Ldap,
                json!({"starttls": "yes"}),
                one("starttls", "must be a boolean"),
            ),
            (
                Kind::Ldap,
                json!({"role": "primary"}),
                one("role", "unknown key"),
            ),
            (
                Kind::Ldap,
                json!({"database": "dc=example"}),
                one("database", "unknown key"),
            ),
        ];
        for (kind, options, want) in rows {
            assert_eq!(errors(kind, options.clone()), want, "{kind:?} {options}");
        }
    }

    #[test]
    fn valid_options() {
        let f = Files::new();
        let pass = f.write("pass", b"s3cret\n");
        let ca = f.write("ca.pem", CERT_PEM.as_bytes());
        let Value::Object(map) = json!({
            "user": "emfor", "database": "app", "role": "primary",
            "password_file": pass, "tls": true, "tls_verify": false,
            "server_name": "db.internal", "ca_file": ca,
        }) else {
            unreachable!()
        };
        let s = parse(Kind::Pgsql, &map).unwrap();
        assert_eq!(s.user.as_deref(), Some("emfor"));
        assert_eq!(s.database.as_deref(), Some("app"));
        assert_eq!(s.role, Some(Role::Primary));
        let tls = s.tls.unwrap();
        assert!(!tls.verify);
        assert_eq!(tls.server_name.as_deref(), Some("db.internal"));
        assert!(tls.ca_file.is_some());
        assert_eq!(errors(Kind::Redis, json!({})), vec![]);
        assert_eq!(parse(Kind::Redis, &Options::new()).unwrap().tls, None);
    }

    #[test]
    fn password_key_is_unknown() {
        assert_eq!(
            errors(Kind::Redis, json!({"password": "s3cret"})),
            one("password", "unknown key")
        );
    }

    #[test]
    fn other_type_errors() {
        assert_eq!(
            errors(Kind::Mysql, json!({"user": ""})),
            one("user", "must be a non-empty string")
        );
        assert_eq!(
            errors(Kind::Redis, json!({"password_file": 1})),
            one("password_file", "must be a string")
        );
        assert_eq!(
            errors(Kind::Redis, json!({"tls": true, "ca_file": "ca.pem"})),
            one("ca_file", "not an absolute path")
        );
        assert_eq!(
            errors(
                Kind::Redis,
                json!({"tls": true, "server_name": "bad name!"})
            ),
            one("server_name", "not a valid host name or IP address")
        );
        assert_eq!(
            errors(Kind::Redis, json!({"tls": false, "tls_verify": true})),
            one("tls_verify", "requires tls = true")
        );
    }

    #[test]
    fn password_file_must_be_utf8() {
        let f = Files::new();
        let latin1 = f.write("latin1", b"caf\xe9\n");
        assert_eq!(
            errors(Kind::Redis, json!({"password_file": latin1})),
            one("password_file", "not valid UTF-8")
        );
    }

    #[test]
    fn settings_for_a_check_do_not_read_files() {
        let Value::Object(map) = json!({
            "user": "emfor", "password_file": "/nonexistent/pass",
            "tls": true, "ca_file": "/nonexistent/ca.pem",
        }) else {
            unreachable!()
        };
        assert!(parse(Kind::Mysql, &map).is_err());
        let s = settings(Kind::Mysql, &map).unwrap();
        assert_eq!(s.password_file, Some(PathBuf::from("/nonexistent/pass")));
        assert_eq!(
            s.tls.unwrap().ca_file,
            Some(PathBuf::from("/nonexistent/ca.pem"))
        );
        let Value::Object(relative) = json!({"user": "emfor", "password_file": "pass"}) else {
            unreachable!()
        };
        assert_eq!(
            settings(Kind::Mysql, &relative).unwrap_err()[0].message,
            "not an absolute path"
        );
    }

    #[test]
    fn read_password_trims_one_newline() {
        let f = Files::new();
        let crlf = f.write("crlf", b"pa ss\r\n");
        let lf = f.write("lf", b"pass\n\n");
        let bare = f.write("bare", b"pass");
        assert_eq!(read_password(Path::new(&crlf)).unwrap(), "pa ss");
        assert_eq!(read_password(Path::new(&lf)).unwrap(), "pass\n");
        assert_eq!(read_password(Path::new(&bare)).unwrap(), "pass");
    }

    #[test]
    fn read_password_errors_hide_the_content() {
        let f = Files::new();
        let path = f.write("pass", b"s3cret\n");
        std::fs::remove_file(&path).unwrap();
        let err = read_password(Path::new(&path)).unwrap_err();
        assert!(err.starts_with("password_file: "), "{err}");
        assert!(!err.contains("s3cret"));
        let empty = f.write("empty", b"\r\n");
        assert_eq!(
            read_password(Path::new(&empty)).unwrap_err(),
            "password_file: file is empty"
        );
    }
}
