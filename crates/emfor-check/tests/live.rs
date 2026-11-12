//! `emfor-check` against real redis, MariaDB, PostgreSQL and OpenLDAP servers.
//!
//! Ignored by default: these tests need the servers that
//! tests/plugins/inside.sh starts in the `emfor-plugins-test` container
//! (run tests/plugins/run.sh). The layout they expect:
//!
//! | Port | Server |
//! | --- | --- |
//! | 6379 | redis primary, plaintext, user `emfor` |
//! | 6380 | redis replica of 6379, plaintext |
//! | 6390 | redis primary, TLS |
//! | 3306 | MariaDB, TLS available, user `emfor` |
//! | 3307 | MariaDB with `read_only = 1`, no TLS |
//! | 5432 | PostgreSQL primary, TLS available, user `emfor`, database `app` |
//! | 5433 | PostgreSQL standby of 5432, no TLS |
//! | 389 | OpenLDAP, StartTLS available, `cn=monitor,dc=example,dc=org` |
//! | 636 | the same OpenLDAP, LDAPS |
//! | 3389 | OpenLDAP without TLS |
//!
//! Server certificates are for `db.test`, signed by `$EMFOR_LIVE_DIR/ca.pem`;
//! `$EMFOR_LIVE_DIR/good.pass` holds the password, `bad.pass` a wrong one.
//! Nothing listens on 6399, 3399, 5499 and 3899. The LDAP administrator is
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
