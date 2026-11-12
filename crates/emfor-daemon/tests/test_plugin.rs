//! The `emfor-test-plugin` binary speaks protocol 1.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

#[test]
fn handshake_validate_and_check() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_emfor-test-plugin"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let requests = [
        json!({"type":"hello","protocol":1}),
        json!({"type":"validate","id":1,"options":{"mode":"down"}}),
        json!({"type":"validate","id":2,"options":{"mode":"nope","port":1}}),
        json!({"type":"check","id":3,"service":"a","target":{"ip":"10.0.0.1","port":80},"options":{"mode":"down"},"timeout_ms":900}),
    ];
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut answers = Vec::new();
    for r in &requests {
        writeln!(stdin, "{r}").unwrap();
        stdin.flush().unwrap();
        let line = lines.next().unwrap().unwrap();
        answers.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "{status}");
    assert_eq!(answers[0]["type"], "hello");
    assert_eq!(answers[0]["protocol"], 1);
    assert_eq!(answers[1], json!({"type":"validate","id":1,"errors":[]}));
    assert_eq!(answers[2]["id"], 2);
    let keys: Vec<&str> = answers[2]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys.len(), 2);
    assert!(keys.contains(&"mode") && keys.contains(&"port"), "{keys:?}");
    assert_eq!(
        answers[3],
        json!({"type":"check","id":3,"result":"down","reason":"test plugin says down"})
    );
}
