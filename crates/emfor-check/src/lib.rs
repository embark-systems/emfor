//! The plugins behind the `emfor-check` executable: the database plugins
//! redis, mysql, pgsql and ldap, and the network plugin dns.

mod ber;
mod dns;
mod ldap;
mod mysql;
pub mod net;
pub mod options;
mod pgsql;
mod redis;
mod resp;

use emfor_plugin::{CheckOutcome, CheckRequest, OptionError, Options, Plugin};

/// Which service a plugin checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Redis,
    Mysql,
    Pgsql,
    Ldap,
    Dns,
}

impl Kind {
    /// The plugin for a command-line argument.
    pub fn from_name(name: &str) -> Option<Kind> {
        match name {
            "redis" => Some(Kind::Redis),
            "mysql" => Some(Kind::Mysql),
            "pgsql" => Some(Kind::Pgsql),
            "ldap" => Some(Kind::Ldap),
            "dns" => Some(Kind::Dns),
            _ => None,
        }
    }

    /// The plugin's name, as given on the command line and in the handshake.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Redis => "redis",
            Kind::Mysql => "mysql",
            Kind::Pgsql => "pgsql",
            Kind::Ldap => "ldap",
            Kind::Dns => "dns",
        }
    }
}

/// One of the `emfor-check` plugins.
pub struct Service {
    kind: Kind,
}

impl Service {
    pub fn new(kind: Kind) -> Service {
        Service { kind }
    }
}

impl Plugin for Service {
    fn name(&self) -> &str {
        self.kind.name()
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn validate(&self, options: &Options) -> Vec<OptionError> {
        // The network plugins validate their own options; the database
        // plugins share `options`.
        let result = match self.kind {
            Kind::Dns => dns::parse(options).map(|_| ()),
            kind => options::parse(kind, options).map(|_| ()),
        };
        result.err().unwrap_or_default()
    }

    async fn check(&self, req: CheckRequest) -> CheckOutcome {
        // The daemon only sends options that passed `validate`. Files are
        // not read here: each check reads them and reports a missing one as
        // `password_file: ...` or `ca_file: ...`.
        if self.kind == Kind::Dns {
            return match dns::parse(&req.options) {
                Ok(settings) => dns::check(&req, &settings).await,
                Err(errors) => options_error(errors),
            };
        }
        let settings = match options::settings(self.kind, &req.options) {
            Ok(s) => s,
            Err(errors) => return options_error(errors),
        };
        match self.kind {
            Kind::Redis => redis::check(&req, &settings).await,
            Kind::Mysql => mysql::check(&req, &settings).await,
            Kind::Pgsql => pgsql::check(&req, &settings).await,
            Kind::Ldap => ldap::check(&req, &settings).await,
            // Handled above.
            Kind::Dns => dns_is_not_a_database(),
        }
    }
}

/// The `error` for options that no longer pass validation at check time.
fn options_error(errors: Vec<OptionError>) -> CheckOutcome {
    let list: Vec<String> = errors
        .iter()
        .map(|e| format!("{}: {}", e.key, e.message))
        .collect();
    CheckOutcome::Error(format!("check_options: {}", list.join("; ")))
}

/// `Kind::Dns` never reaches the database branch of [`Service::check`];
/// answering `error` keeps a mistake there visible without a panic.
fn dns_is_not_a_database() -> CheckOutcome {
    CheckOutcome::Error("internal: dns check sent to the database plugins".into())
}
