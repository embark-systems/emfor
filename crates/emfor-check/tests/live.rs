//! `emfor-check` against real redis, MariaDB, PostgreSQL, OpenLDAP,
//! unbound, FreeRADIUS, Kamailio, freeDiameter, Postfix, RabbitMQ, MongoDB
//! and gRPC (Python grpcio) servers.
//!
//! Ignored by default: these tests need the servers that
//! tests/plugins/inside.sh starts in the `emfor-plugins-test` container
//! (run tests/plugins/run.sh). The layout they expect:
//!
//! | Port | Server |
//! | --- | --- |
//! | 6379 | redis primary, plaintext, user `emfor` |
//! | 6380 | redis replica of 6379, plaintext |
//! | 6381 | redis replica of 6399 (nothing there), plaintext |
//! | 6390 | redis primary, TLS |
//! | 3306 | MariaDB, TLS available, user `emfor` |
//! | 3307 | MariaDB replica of 3306 with `read_only = 1`, no TLS |
//! | 3308 | MariaDB replica of 3399 (nothing there), no TLS |
//! | 5432 | PostgreSQL primary, TLS available, user `emfor`, database `app` |
//! | 5433 | PostgreSQL standby of 5432, no TLS |
//! | 5434 | PostgreSQL standby of 5499 (nothing there), no TLS |
//! | 389 | OpenLDAP, StartTLS available, `cn=monitor,dc=example,dc=org` |
//! | 636 | the same OpenLDAP, LDAPS |
//! | 3389 | OpenLDAP without TLS |
//! | 5353 | unbound, UDP and TCP, local zone `check.test` with only `a.check.test A 192.0.2.1` |
//! | 1812, 1813 | FreeRADIUS authentication and accounting, Status-Server on, secret `testing123`, user `probe` with password `probe-pw` |
//! | 5060, 5061 | Kamailio, UDP and TCP on 5060, TLS on 5061; OPTIONS is 200, or 404 for user `nobody` |
//! | 2525 | Postfix, plaintext with STARTTLS |
//! | 4465 | the same Postfix, TLS from the start |
//! | 2526 | the same Postfix without STARTTLS |
//! | 5672 | RabbitMQ, plaintext, user `emfor` with vhost `app` (no rights on vhost `locked`) |
//! | 5671 | the same RabbitMQ, TLS |
//! | 27017 | MongoDB primary of replica set `rs0`, no login |
//! | 27018 | MongoDB secondary of `rs0` |
//! | 27020 | MongoDB standalone with `--auth`, user `emfor` in `admin`, plaintext or TLS |
//! | 50051 | gRPC health service: `""` SERVING, `down` NOT_SERVING, others NOT_FOUND |
//! | 50052 | the same over TLS |
//! | 50053 | gRPC without a health service |
//!
//! Server certificates are for `db.test`, signed by `$EMFOR_LIVE_DIR/ca.pem`;
//! `$EMFOR_LIVE_DIR/good.pass` holds the password, `bad.pass` a wrong one.
//! Nothing listens on 6399, 3399, 5499, 3899, 5399, 1899, 5099, 2599, 5699, 27099 and 50099 (TCP and
//! UDP).
//! `radius.secret`, `wrong.secret` and `probe.pass` hold the RADIUS secret,
//! another secret and the password of `probe`. The LDAP administrator is
//! `cn=admin,dc=example,dc=org` with password `admin-pw`.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

fn dir() -> PathBuf {
    PathBuf::from(std::env::var("EMFOR_LIVE_DIR").expect("EMFOR_LIVE_DIR is set by inside.sh"))
}

fn file(name: &str) -> String {
    dir().join(name).to_str().unwrap().to_owned()
}

/// A running `emfor-check <plugin>` after the handshake.
struct Plugin {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Plugin {
    fn start(name: &str) -> Plugin {
        let mut child = Command::new(env!("CARGO_BIN_EXE_emfor-check"))
            .arg(name)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut p = Plugin {
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        let hello = p.ask(json!({"type":"hello","protocol":1}));
        assert_eq!(hello["name"], name);
        p
    }

    fn ask(&mut self, req: Value) -> Value {
        writeln!(self.stdin, "{req}").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"))
    }

    /// Validate, then check `127.0.0.1:<port>`; returns (result, reason).
    fn check(&mut self, port: u16, options: Value) -> (String, String) {
        let id = self.next_id;
        self.next_id += 2;
        let v = self.ask(json!({"type":"validate","id":id,"options":options}));
        assert_eq!(v["errors"], json!([]), "options {options} are invalid");
        let a = self.ask(json!({
            "type":"check","id":id + 1,"service":"live",
            "target":{"ip":"127.0.0.1","port":port},
            "options":options,"timeout_ms":3000,
        }));
        assert_eq!(a["id"], id + 1);
        (
            a["result"].as_str().unwrap().to_owned(),
            a["reason"].as_str().unwrap_or("").to_owned(),
        )
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[track_caller]
fn up(got: (String, String)) {
    assert_eq!(got.0, "up", "reason: {}", got.1);
}

#[track_caller]
fn down(got: (String, String), reason_start: &str) {
    assert_eq!(got.0, "down", "reason: {}", got.1);
    assert!(
        got.1.starts_with(reason_start),
        "reason {:?} does not start with {reason_start:?}",
        got.1
    );
    assert!(
        !got.1.contains("s3cret") && !got.1.contains("wrong-pw"),
        "reason shows a password: {}",
        got.1
    );
}

/// Options with the user, a password file and anything in `extra`.
fn opts(user: &str, pass: &str, extra: Value) -> Value {
    let mut o = json!({"user": user, "password_file": file(pass)});
    for (k, v) in extra.as_object().unwrap() {
        o[k] = v.clone();
    }
    o
}

fn tls(server_name: &str, verify: bool) -> Value {
    if verify {
        json!({"tls": true, "server_name": server_name, "ca_file": file("ca.pem")})
    } else {
        json!({"tls": true, "server_name": server_name, "tls_verify": false})
    }
}

/// Run the same TLS cases against a server: right name, wrong name,
/// verification off, and TLS against a plaintext port.
fn tls_cases(p: &mut Plugin, base: impl Fn(Value) -> Value, tls_port: u16, plain_port: u16) {
    up(p.check(tls_port, base(tls("db.test", true))));
    down(p.check(tls_port, base(tls("nope.test", true))), "tls: ");
    up(p.check(tls_port, base(tls("nope.test", false))));
    down(p.check(plain_port, base(tls("db.test", false))), "tls: ");
}

#[test]
#[ignore = "needs the emfor-plugins-test container"]
fn redis_live() {
    let mut p = Plugin::start("redis");
    up(p.check(6379, opts("emfor", "good.pass", json!({}))));
    down(
        p.check(6379, opts("emfor", "bad.pass", json!({}))),
        "auth: ",
    );
    down(p.check(6399, json!({})), "connect: ");
    up(p.check(6379, opts("emfor", "good.pass", json!({"role": "master"}))));
    down(
        p.check(6380, opts("emfor", "good.pass", json!({"role": "master"}))),
        "role is replica, want master",
    );
    up(p.check(6380, opts("emfor", "good.pass", json!({"role": "replica"}))));
    up(p.check(
        6380,
        opts(
            "emfor",
            "good.pass",
            json!({"role": "replica", "max_lag": 30}),
        ),
    ));
    down(
        p.check(6379, opts("emfor", "good.pass", json!({"role": "replica"}))),
        "role is master, want replica",
    );
    down(
        p.check(6381, opts("emfor", "good.pass", json!({"role": "replica"}))),
        "replication: link to master is ",
    );
    tls_cases(&mut p, |t| opts("emfor", "good.pass", t), 6390, 6379);

    // Rotate the password on the server and in the file; no reload.
    let rotate = Command::new("redis-cli")
        .args([
            "-p",
            "6379",
            "--user",
            "emfor",
            "--pass",
            "s3cret",
            "--no-auth-warning",
        ])
        .args(["ACL", "SETUSER", "emfor", "resetpass", ">rotated-pw"])
        .output()
        .unwrap();
    assert!(rotate.status.success(), "{rotate:?}");
    let rotated = dir().join("rotated.pass");
    std::fs::write(&rotated, "s3cret\n").unwrap();
    let o = json!({"user": "emfor", "password_file": rotated.to_str().unwrap()});
    down(p.check(6379, o.clone()), "auth: ");
    std::fs::write(&rotated, "rotated-pw\n").unwrap();
    up(p.check(6379, o));
}

#[test]
#[ignore = "needs the emfor-plugins-test container"]
fn mysql_live() {
    let mut p = Plugin::start("mysql");
    up(p.check(3306, opts("emfor", "good.pass", json!({}))));
    down(
        p.check(3306, opts("emfor", "bad.pass", json!({}))),
        "auth: ",
    );
    down(p.check(3399, json!({"user": "emfor"})), "connect: ");
    up(p.check(3306, opts("emfor", "good.pass", json!({"role": "primary"}))));
    down(
        p.check(3307, opts("emfor", "good.pass", json!({"role": "primary"}))),
        "role is read-only, want primary",
    );
    up(p.check(3307, opts("emfor", "good.pass", json!({}))));
    up(p.check(3307, opts("emfor", "good.pass", json!({"role": "replica"}))));
    up(p.check(
        3307,
        opts(
            "emfor",
            "good.pass",
            json!({"role": "replica", "max_lag": 30}),
        ),
    ));
    down(
        p.check(3306, opts("emfor", "good.pass", json!({"role": "replica"}))),
        "role is not a replica, want replica",
    );
    down(
        p.check(3308, opts("emfor", "good.pass", json!({"role": "replica"}))),
        "replication: IO thread is ",
    );
    tls_cases(&mut p, |t| opts("emfor", "good.pass", t), 3306, 3307);
}

#[test]
#[ignore = "needs the emfor-plugins-test container"]
fn pgsql_live() {
    let mut p = Plugin::start("pgsql");
    let db = |extra: Value| {
        let mut o = opts("emfor", "good.pass", extra);
        o["database"] = json!("app");
        o
    };
    up(p.check(5432, db(json!({}))));
    let mut wrong = db(json!({}));
    wrong["password_file"] = json!(file("bad.pass"));
    down(p.check(5432, wrong), "auth: ");
    down(
        p.check(5499, json!({"user": "emfor", "database": "app"})),
        "connect: ",
    );
    let mut missing_db = db(json!({}));
    missing_db["database"] = json!("nope");
    down(p.check(5432, missing_db), "auth: ");
    up(p.check(5432, db(json!({"role": "primary"}))));
    down(
        p.check(5433, db(json!({"role": "primary"}))),
        "role is standby, want primary",
    );
    up(p.check(5433, db(json!({}))));
    up(p.check(5433, db(json!({"role": "replica"}))));
    up(p.check(5433, db(json!({"role": "replica", "max_lag": 30}))));
    down(
        p.check(5432, db(json!({"role": "replica"}))),
        "role is primary, want replica",
    );
    // The lost standby retries its primary; between attempts it has no WAL
    // receiver, so allow a few seconds for a check to see that.
    let mut lost = p.check(5434, db(json!({"role": "replica"})));
    for _ in 0..20 {
        if lost.0 == "down" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
        lost = p.check(5434, db(json!({"role": "replica"})));
    }
    down(lost, "replication: no WAL receiver");
    tls_cases(&mut p, db, 5432, 5433);
}

#[test]
#[ignore = "needs the emfor-plugins-test container"]
fn ldap_live() {
    const MONITOR: &str = "cn=monitor,dc=example,dc=org";
    let mut p = Plugin::start("ldap");
    up(p.check(389, json!({})));
    up(p.check(389, opts(MONITOR, "good.pass", json!({}))));
    assert_eq!(
        p.check(389, opts(MONITOR, "bad.pass", json!({}))),
        ("down".into(), "auth: 49 invalidCredentials".into())
    );
    down(p.check(3899, json!({})), "connect: ");

    // LDAPS on 636: right name, wrong name, verification off, and TLS
    // against the plaintext port 389.
    tls_cases(&mut p, |t| opts(MONITOR, "good.pass", t), 636, 389);

    // StartTLS on 389.
    let starttls = |name: &str| {
        opts(
            MONITOR,
            "good.pass",
            json!({"starttls": true, "server_name": name, "ca_file": file("ca.pem")}),
        )
    };
    up(p.check(389, starttls("db.test")));
    down(p.check(389, starttls("nope.test")), "tls: ");
    down(
        p.check(3389, starttls("db.test")),
        "tls: StartTLS refused: ",
    );

    // Rotate the password on the server and in the file; no reload.
    let rotate = Command::new("ldappasswd")
        .args(["-x", "-H", "ldap://127.0.0.1:389"])
        .args(["-D", "cn=admin,dc=example,dc=org", "-w", "admin-pw"])
        .args(["-s", "rotated-pw", MONITOR])
        .output()
        .unwrap();
    assert!(rotate.status.success(), "{rotate:?}");
    let rotated = dir().join("ldap-rotated.pass");
    std::fs::write(&rotated, "s3cret\n").unwrap();
    let o = json!({"user": MONITOR, "password_file": rotated.to_str().unwrap()});
    down(p.check(389, o.clone()), "auth: 49 invalidCredentials");
    std::fs::write(&rotated, "rotated-pw\n").unwrap();
    up(p.check(389, o));
}

#[test]
#[ignore = "needs the emfor-plugins-test container"]
fn dns_live() {
    let mut p = Plugin::start("dns");
    let name = |n: &str, extra: Value| {
        let mut o = json!({ "name": n });
        for (k, v) in extra.as_object().unwrap() {
            o[k] = v.clone();
        }
        o
    };
    up(p.check(5353, name("a.check.test", json!({ "expect_answer": true }))));
    assert_eq!(
        p.check(5353, name("nope.check.test", json!({}))),
        ("down".into(), "rcode: NXDOMAIN".into())
    );
    up(p.check(
        5353,
        name(
            "nope.check.test",
            json!({ "expect_rcode": ["NOERROR", "NXDOMAIN"] }),
        ),
    ));
    up(p.check(
        5353,
        name(
            "A.Check.Test.",
            json!({ "transport": "tcp", "expect_answer": true }),
        ),
    ));
    assert_eq!(
        p.check(
            5353,
            name(
                "a.check.test",
                json!({ "type": "AAAA", "expect_answer": true })
            )
        ),
        ("down".into(), "answer: no AAAA records".into())
    );
    let closed = p.check(5399, name("a.check.test", json!({})));
    assert_eq!(closed.0, "down", "reason: {}", closed.1);
    assert!(
        closed.1.starts_with("connect: ") || closed.1 == "timeout",
        "reason {:?}",
        closed.1
    );
    down(
        p.check(5399, name("a.check.test", json!({ "transport": "tcp" }))),
        "connect: ",
    );
}

#[test]
#[ignore = "needs the emfor-plugins-test container"]
fn radius_live() {
    let mut p = Plugin::start("radius");
    let with = |extra: Value| {
        let mut o = json!({ "secret_file": file("radius.secret") });
        for (k, v) in extra.as_object().unwrap() {
            o[k] = v.clone();
        }
        o
    };
    let access = |pass: &str, extra: Value| {
        let mut o = with(json!({
            "request": "access-request",
            "user": "probe",
            "password_file": file(pass),
        }));
        for (k, v) in extra.as_object().unwrap() {
            o[k] = v.clone();
        }
        o
    };
    let no_secrets = |got: &(String, String)| {
        assert!(
            !got.1.contains("testing123") && !got.1.contains("probe-pw"),
            "reason shows a secret: {}",
            got.1
        );
    };

    up(p.check(1812, with(json!({}))));
    up(p.check(1813, with(json!({ "application": "acct" }))));
    // FreeRADIUS checks the request's Message-Authenticator and drops a
    // request made with another secret without answering.
    let wrong = p.check(1812, json!({ "secret_file": file("wrong.secret") }));
    no_secrets(&wrong);
    assert_eq!(wrong, ("down".into(), "timeout".into()));
    up(p.check(1812, access("probe.pass", json!({}))));
    let reject = p.check(1812, access("bad.pass", json!({})));
    no_secrets(&reject);
    down(reject, "auth: Access-Reject");
    up(p.check(1812, access("bad.pass", json!({ "reject_is_up": true }))));
    // FreeRADIUS 3.2.1 does not sign its responses with Message-Authenticator.
    assert_eq!(
        p.check(1812, with(json!({ "require_message_authenticator": true }))),
        (
            "down".into(),
            "auth: response has no Message-Authenticator".into()
        )
    );
    let closed = p.check(1899, with(json!({})));
    assert_eq!(closed.0, "down", "reason: {}", closed.1);
    assert!(
        closed.1.starts_with("connect: ") || closed.1 == "timeout",
        "reason {:?}",
        closed.1
    );
}

#[test]
#[ignore = "needs the emfor-plugins-test container"]
fn sip_live() {
    let mut p = Plugin::start("sip");
    up(p.check(5060, json!({})));
    up(p.check(5060, json!({ "transport": "tcp" })));
    let sips = |name: &str| {
        json!({
            "transport": "tcp",
            "tls": true,
            "server_name": name,
            "ca_file": file("ca.pem"),
        })
    };
    up(p.check(5061, sips("db.test")));
    down(p.check(5061, sips("nope.test")), "tls: ");
    let nobody = |extra: Value| {
        let mut o = json!({ "uri": "sip:nobody@127.0.0.1:5060" });
        for (k, v) in extra.as_object().unwrap() {
            o[k] = v.clone();
        }
        o
    };
    up(p.check(5060, nobody(json!({}))));
    assert_eq!(
        p.check(5060, nobody(json!({ "expect_status": ["200"] }))),
        ("down".into(), "status: 404 Not Found".into())
    );
    let closed = p.check(5099, json!({}));
    assert_eq!(closed.0, "down", "reason: {}", closed.1);
    assert!(
        closed.1.starts_with("connect: ") || closed.1 == "timeout",
        "reason {:?}",
        closed.1
    );
}

#[test]
#[ignore = "needs the emfor-plugins-test container"]
fn diameter_live() {
    let mut p = Plugin::start("diameter");
    // A 3GPP-style vendor-specific application; the relay on 3868 has every
    // application in common, the node on 3869 none.
    let peer = |host: &str, extra: Value| {
        let mut o = json!({
            "origin_host": host,
            "origin_realm": "test",
            "vendor_specific_application_ids": [{ "vendor_id": 999999, "auth_application_id": 4294967040u64 }],
        });
        for (k, v) in extra.as_object().unwrap() {
            o[k] = v.clone();
        }
        o
    };
    up(p.check(3868, peer("emfor.test", json!({}))));
    let tls = |client: bool| {
        let mut o = json!({ "tls": true, "server_name": "db.test", "ca_file": file("ca.pem") });
        if client {
            o["tls_cert_file"] = json!(file("emfor.crt"));
            o["tls_key_file"] = json!(file("emfor.key"));
        }
        peer("emfor.test", o)
    };
    up(p.check(5658, tls(true)));
    // freeDiameter requires a client certificate: without one the TLS
    // handshake completes on our side, then it closes the connection.
    down(p.check(5658, tls(false)), "cea: connection closed by peer");
    let stranger = p.check(3868, peer("stranger.test", json!({})));
    assert_eq!(stranger.0, "down", "reason: {}", stranger.1);
    assert_eq!(stranger.1, "cea: 3010 DIAMETER_UNKNOWN_PEER");
    assert_eq!(
        p.check(
            3869,
            json!({ "origin_host": "emfor.test", "origin_realm": "test", "auth_application_ids": [4] })
        ),
        ("down".into(), "cea: 5010 DIAMETER_NO_COMMON_APPLICATION".into())
    );
    down(p.check(3999, peer("emfor.test", json!({}))), "connect: ");
}

#[test]
#[ignore = "needs the emfor-plugins-test container"]
fn smtp_live() {
    let mut p = Plugin::start("smtp");
    up(p.check(2525, json!({})));
    up(p.check(2525, json!({"helo": "app1.test"})));
    let verified = json!({"server_name": "db.test", "ca_file": file("ca.pem")});
    let mut starttls = verified.clone();
    starttls["starttls"] = json!(true);
    up(p.check(2525, starttls.clone()));
    let mut wrong_name = starttls.clone();
    wrong_name["server_name"] = json!("nope.test");
    down(p.check(2525, wrong_name), "tls: ");
    let mut implicit = verified;
    implicit["tls"] = json!(true);
    up(p.check(4465, implicit.clone()));
    down(p.check(2525, implicit), "tls: ");
    down(p.check(2526, starttls), "tls: STARTTLS not offered");
    down(p.check(2599, json!({})), "connect: ");
}

#[test]
#[ignore = "needs the emfor-plugins-test container"]
fn amqp_live() {
    let mut p = Plugin::start("amqp");
    let login = |pass: &str, extra: Value| {
        let mut o = opts("emfor", pass, extra);
        if o.get("vhost").is_none() {
            o["vhost"] = json!("app");
        }
        o
    };
    up(p.check(5672, login("good.pass", json!({}))));
    down(p.check(5672, login("bad.pass", json!({}))), "auth: 403 ");
    down(
        p.check(5672, login("good.pass", json!({"vhost": "locked"}))),
        "vhost: 530 ",
    );
    down(
        p.check(5672, login("good.pass", json!({"vhost": "nope"}))),
        "vhost: 530 ",
    );
    up(p.check(5671, login("good.pass", tls("db.test", true))));
    down(
        p.check(5672, login("good.pass", tls("db.test", true))),
        "tls: ",
    );
    down(p.check(5699, login("good.pass", json!({}))), "connect: ");
}

#[test]
#[ignore = "needs the emfor-plugins-test container"]
fn mongodb_live() {
    let mut p = Plugin::start("mongodb");
    up(p.check(27017, json!({})));
    up(p.check(27017, json!({"role": "primary"})));
    down(
        p.check(27017, json!({"role": "secondary"})),
        "role is primary, want secondary",
    );
    down(
        p.check(27018, json!({"role": "primary"})),
        "role is secondary, want primary",
    );
    up(p.check(27018, json!({"role": "secondary"})));
    up(p.check(
        27020,
        opts("emfor", "good.pass", json!({"role": "primary"})),
    ));
    down(
        p.check(27020, opts("emfor", "bad.pass", json!({}))),
        "auth: AuthenticationFailed: ",
    );
    up(p.check(27020, opts("emfor", "good.pass", tls("db.test", true))));
    down(
        p.check(27020, opts("emfor", "good.pass", tls("nope.test", true))),
        "tls: ",
    );
    down(p.check(27099, json!({})), "connect: ");
}

#[test]
#[ignore = "needs the emfor-plugins-test container"]
fn grpc_live() {
    let mut p = Plugin::start("grpc");
    up(p.check(50051, json!({})));
    down(
        p.check(50051, json!({"service": "down"})),
        "status: NOT_SERVING",
    );
    down(
        p.check(50051, json!({"service": "nope"})),
        "grpc: 5 NOT_FOUND: unknown service",
    );
    down(p.check(50053, json!({})), "grpc: 12 UNIMPLEMENTED");
    up(p.check(50052, tls("db.test", true)));
    down(p.check(50052, tls("nope.test", true)), "tls: ");
    down(p.check(50051, tls("db.test", true)), "tls: ");
    down(p.check(50052, json!({})), "query: HTTP/2: ");
    down(p.check(50099, json!({})), "connect: ");
}
