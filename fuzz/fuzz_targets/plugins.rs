//! A plugin check against a backend that answers with the fuzzer's bytes.
//!
//! The first byte picks the plugin, the second its options (one of a few
//! sets per plugin, without TLS), the rest is what the backend answers:
//! the bytes a TCP connection reads, or for UDP length-prefixed datagrams
//! (see `emfor_check::net::fuzz`). `seeds/plugins.py` writes a well-formed
//! answer for each plugin but grpc to `seeds/plugins/`. `mysql` and `pgsql`
//! are left out: their protocols are parsed by `mysql_async` and
//! `tokio-postgres`, which connect on their own.
#![no_main]

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use emfor_check::{Kind, Service};
use emfor_plugin::{CheckRequest, Options, Plugin};
use serde_json::{Value, json};

const KINDS: [Kind; 10] = [
    Kind::Redis,
    Kind::Ldap,
    Kind::Dns,
    Kind::Radius,
    Kind::Sip,
    Kind::Diameter,
    Kind::Smtp,
    Kind::Amqp,
    Kind::Mongodb,
    Kind::Grpc,
];

/// A password file and a RADIUS secret file, written once.
fn files() -> &'static (String, String) {
    static FILES: OnceLock<(String, String)> = OnceLock::new();
    FILES.get_or_init(|| {
        let dir: PathBuf = std::env::temp_dir().join(format!("emfor-fuzz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the temp dir is writable");
        let password = dir.join("password");
        let secret = dir.join("secret");
        std::fs::write(&password, "secret\n").expect("the temp dir is writable");
        std::fs::write(&secret, "testing123\n").expect("the temp dir is writable");
        (password.display().to_string(), secret.display().to_string())
    })
}

fn presets(kind: Kind) -> Vec<Value> {
    let (password, secret) = files();
    match kind {
        Kind::Redis => vec![
            json!({}),
            json!({"role": "master"}),
            json!({"role": "replica", "max_lag": 10}),
            json!({"password_file": password}),
            json!({"user": "u", "password_file": password, "role": "replica"}),
        ],
        Kind::Ldap => vec![
            json!({}),
            json!({"user": "cn=admin,dc=example,dc=org", "password_file": password}),
        ],
        Kind::Dns => vec![
            json!({"name": "example.org"}),
            json!({"name": "example.org", "type": "MX", "expect_answer": true}),
            json!({"name": "example.org", "type": "TXT", "transport": "tcp", "expect_answer": true}),
            json!({"name": ".", "type": "SOA", "expect_rcode": ["NOERROR", "NXDOMAIN"]}),
            json!({"name": "_sip._udp.example.org", "type": "SRV", "expect_answer": true}),
        ],
        Kind::Radius => vec![
            json!({"secret_file": secret}),
            json!({"secret_file": secret, "require_message_authenticator": true}),
            json!({"secret_file": secret, "request": "access-request", "user": "u", "password_file": password, "reject_is_up": true}),
        ],
        Kind::Sip => vec![
            json!({}),
            json!({"transport": "tcp"}),
            json!({"transport": "tcp", "expect_status": ["200"]}),
        ],
        Kind::Diameter => vec![
            json!({"origin_host": "emfor.example.org", "origin_realm": "example.org", "auth_application_ids": [16777251]}),
            json!({"origin_host": "emfor.example.org", "origin_realm": "example.org",
                   "vendor_specific_application_ids": [{"vendor_id": 10415, "auth_application_id": 16777238}]}),
        ],
        Kind::Smtp => vec![json!({}), json!({"helo": "emfor.example.org"})],
        Kind::Amqp => vec![
            json!({"user": "guest", "password_file": password}),
            json!({"user": "guest", "password_file": password, "vhost": "prod"}),
        ],
        Kind::Mongodb => vec![
            json!({}),
            json!({"role": "primary"}),
            json!({"role": "secondary"}),
            json!({"user": "u", "password_file": password}),
        ],
        Kind::Grpc => vec![json!({}), json!({"service": "emfor.Test"})],
        Kind::Mysql | Kind::Pgsql => vec![json!({})],
    }
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .expect("a current-thread runtime can be built")
    })
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let [k, p, reply @ ..] = data else {
        return;
    };
    let kind = KINDS[usize::from(*k) % KINDS.len()];
    let presets = presets(kind);
    let Value::Object(options) = presets[usize::from(*p) % presets.len()].clone() else {
        return;
    };
    let options: Options = options;
    emfor_check::net::fuzz::set_reply(reply);
    let req = CheckRequest {
        service: "fuzz.svc.emfor".into(),
        ip: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        port: 4242,
        options,
        timeout: Duration::from_secs(2),
    };
    let service = Service::new(kind);
    let outcome = runtime().block_on(service.check(req));
    // EMFOR_FUZZ_PRINT=1 shows each outcome, to check the seeds by hand.
    if std::env::var_os("EMFOR_FUZZ_PRINT").is_some() {
        eprintln!(
            "{} {}: {outcome:?}",
            kind.name(),
            usize::from(*p) % presets.len()
        );
    }
});
