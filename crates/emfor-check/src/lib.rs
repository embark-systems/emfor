//! The plugins behind the `emfor-check` executable: the database plugins
//! redis, mysql, pgsql and ldap, and the network plugins dns, radius and
//! sip.

mod ber;
mod dns;
mod ldap;
mod mysql;
pub mod net;
pub mod options;
mod pgsql;
mod radius;
mod redis;
mod resp;
mod sip;

use emfor_plugin::{CheckOutcome, CheckRequest, OptionError, Options, Plugin};

/// Which service a plugin checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Redis,
    Mysql,
    Pgsql,
    Ldap,
    Dns,
    Radius,
    Sip,
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
            "radius" => Some(Kind::Radius),
            "sip" => Some(Kind::Sip),
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
            Kind::Radius => "radius",
            Kind::Sip => "sip",
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
            Kind::Radius => radius::parse(options, true).map(|_| ()),
            Kind::Sip => sip::parse(options, true).map(|_| ()),
            kind => options::parse(kind, options).map(|_| ()),
        };
        result.err().unwrap_or_default()
    }

    async fn check(&self, req: CheckRequest) -> CheckOutcome {
        // The daemon only sends options that passed `validate`. Files are
        // not read here: each check reads them and reports a missing one as
        // `password_file: ...`, `secret_file: ...` or `ca_file: ...`.
        match self.kind {
            Kind::Dns => match dns::parse(&req.options) {
                Ok(settings) => dns::check(&req, &settings).await,
                Err(errors) => options_error(errors),
            },
            Kind::Radius => match radius::parse(&req.options, false) {
                Ok(settings) => radius::check(&req, &settings).await,
                Err(errors) => options_error(errors),
            },
            Kind::Sip => match sip::parse(&req.options, false) {
                Ok(settings) => sip::check(&req, &settings).await,
                Err(errors) => options_error(errors),
            },
            kind => check_database(kind, &req).await,
        }
    }
}

/// A check by one of the database plugins, which share `options`.
async fn check_database(kind: Kind, req: &CheckRequest) -> CheckOutcome {
    let settings = match options::settings(kind, &req.options) {
        Ok(s) => s,
        Err(errors) => return options_error(errors),
    };
    match kind {
        Kind::Redis => redis::check(req, &settings).await,
        Kind::Mysql => mysql::check(req, &settings).await,
        Kind::Pgsql => pgsql::check(req, &settings).await,
        Kind::Ldap => ldap::check(req, &settings).await,
        // `Service::check` sends the network plugins elsewhere; answering
        // `error` keeps a mistake there visible without a panic.
        Kind::Dns | Kind::Radius | Kind::Sip => CheckOutcome::Error(format!(
            "internal: {} check sent to the database plugins",
            kind.name()
        )),
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
