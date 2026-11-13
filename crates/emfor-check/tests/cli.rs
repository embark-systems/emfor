//! The `emfor-check` executable: plugin selection and the handshake.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn exe() -> Command {
    Command::new(env!("CARGO_BIN_EXE_emfor-check"))
}

#[test]
fn each_plugin_names_itself_in_the_handshake() {
    for name in ["redis", "mysql", "pgsql", "ldap", "dns", "radius"] {
        let mut child = exe()
            .arg(name)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, r#"{{"type":"hello","protocol":1}}"#).unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        drop(stdin);
        let status = child.wait().unwrap();
        assert!(status.success(), "{name}: {status}");
        let hello: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            hello,
            json!({"type":"hello","protocol":1,"name":name,"version":env!("CARGO_PKG_VERSION")})
        );
    }
}

#[test]
fn bad_arguments_exit_2_without_answering() {
    for args in [&[][..], &["memcached"][..], &["redis", "mysql"][..]] {
        let mut child = exe()
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // The handshake may hit a closed pipe; that is the point.
        let _ = writeln!(
            child.stdin.take().unwrap(),
            r#"{{"type":"hello","protocol":1}}"#
        );
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            out.stdout.is_empty(),
            "{args:?}: answered {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("usage: emfor-check redis|mysql|pgsql")
        );
    }
}

#[test]
fn password_file_removed_after_validation_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let pass = dir.path().join("redis.pass");
    std::fs::write(&pass, "s3cret\n").unwrap();
    let options = json!({"password_file": pass.to_str().unwrap()});

    let mut child = exe()
        .arg("redis")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut ask = |req: Value| -> Value {
        writeln!(stdin, "{req}").unwrap();
        stdin.flush().unwrap();
        serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap()
    };
    let valid = ask(json!({"type":"validate","id":1,"options":options}));
    assert_eq!(valid["errors"], json!([]));
    std::fs::remove_file(&pass).unwrap();
    let answer = ask(json!({
        "type":"check","id":2,"service":"cache",
        "target":{"ip":"127.0.0.1","port":1},"options":options,"timeout_ms":900,
    }));
    assert_eq!(answer["result"], "error", "{answer}");
    let reason = answer["reason"].as_str().unwrap();
    assert!(reason.starts_with("password_file: "), "{reason}");
    assert!(!reason.contains("s3cret"), "{reason}");
    drop(ask);
    child.kill().unwrap();
    child.wait().unwrap();
}
