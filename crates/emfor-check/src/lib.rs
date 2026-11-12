//! The plugins behind the `emfor-check` executable: redis, mysql, pgsql
//! and ldap.

mod ber;
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
}

impl Kind {
    /// The plugin for a command-line argument.
    pub fn from_name(name: &str) -> Option<Kind> {
        match name {
            "redis" => Some(Kind::Redis),
            "mysql" => Some(Kind::Mysql),
            "pgsql" => Some(Kind::Pgsql),
            "ldap" => Some(Kind::Ldap),
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
        options::parse(self.kind, options).err().unwrap_or_default()
    }

    async fn check(&self, req: CheckRequest) -> CheckOutcome {
        // The daemon only sends options that passed `validate`. Files are
        // not read here: each check reads them and reports a missing one as
        // `password_file: ...` or `ca_file: ...`.
        let settings = match options::settings(self.kind, &req.options) {
            Ok(s) => s,
            Err(errors) => {
                let list: Vec<String> = errors
                    .iter()
                    .map(|e| format!("{}: {}", e.key, e.message))
                    .collect();
                return CheckOutcome::Error(format!("check_options: {}", list.join("; ")));
            }
        };
        match self.kind {
            Kind::Redis => redis::check(&req, &settings).await,
            Kind::Mysql => mysql::check(&req, &settings).await,
            Kind::Pgsql => pgsql::check(&req, &settings).await,
            Kind::Ldap => ldap::check(&req, &settings).await,
        }
    }
}
