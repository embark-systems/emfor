use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::*;
use crate::checker::Registry;
use crate::config::{self, PeersConfig};

fn conf(extra: &str, backends: &str) -> Config {
    let text =
        format!("[services.\"a.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [{backends}]\n{extra}\n");
    config::parse(&text, &Registry::builtin()).unwrap_or_else(|e| panic!("{e:?}"))
}

fn one(old: &Config, new: &Config) -> Change {
    let c = changes(old, new);
    assert_eq!(c.len(), 1, "{c:?}");
    c.into_iter().next().unwrap()
}

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Sink {
    type Writer = Sink;
    fn make_writer(&'a self) -> Sink {
        self.clone()
    }
}

/// What `log_changes` writes for `changes`.
fn logged(changes: Vec<Change>) -> String {
    let sink = Sink::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(sink.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, || log_changes(changes));
    String::from_utf8(sink.0.lock().unwrap().clone()).unwrap()
}

const B1: &str = "\"10.0.0.11:8080\"";
const B2: &str = "\"10.0.0.11:8080\", \"10.0.0.12:8080\"";

// spectra-scenario: service-config / More than fifty differences / change values
#[test]
fn no_difference_is_no_change() {
    assert!(changes(&conf("", B1), &conf("", B1)).is_empty());
}

// spectra-scenario: service-config / More than fifty differences / change values
#[test]
fn services_and_backends_come_and_go() {
    let none = Config {
        services: Vec::new(),
        ..conf("", B1)
    };
    assert_eq!(
        one(&none, &conf("", B2)),
        Change::ServiceAdded {
            service: "a.svc.emfor".into(),
            backends: 2
        }
    );
    assert_eq!(
        one(&conf("", B1), &none),
        Change::ServiceRemoved {
            service: "a.svc.emfor".into(),
            backends: 1
        }
    );
    assert_eq!(
        one(&conf("", B1), &conf("", B2)),
        Change::BackendAdded {
            service: "a.svc.emfor".into(),
            backend: "10.0.0.12:8080".into()
        }
    );
    assert_eq!(
        one(&conf("", B2), &conf("", B1)),
        Change::BackendRemoved {
            service: "a.svc.emfor".into(),
            backend: "10.0.0.12:8080".into()
        }
    );
}

// spectra-scenario: service-config / Backend added and another drained
// spectra-scenario: service-config / More than fifty differences / change values
#[test]
fn backend_fields_change() {
    let with = |attrs: &str| {
        conf(
            "",
            &format!("{{ addr = \"10.0.0.11:8080\"{attrs} }}, \"10.0.0.13:8080\""),
        )
    };
    let base = with("");
    for (attrs, field, from, to) in [
        (", weight = 3", "weight", "1", "3"),
        (", site = \"dc1\"", "site", "-", "dc1"),
        (", backup = true", "backup", "false", "true"),
        (", drain = true", "drain", "false", "true"),
    ] {
        assert_eq!(
            one(&base, &with(attrs)),
            Change::BackendChanged {
                service: "a.svc.emfor".into(),
                backend: "10.0.0.11:8080".into(),
                field,
                from: from.into(),
                to: to.into()
            },
            "{field}"
        );
    }
    // Added and drained in one reload, the added one first by id order.
    let both = changes(
        &conf("", "\"10.0.0.11:8080\""),
        &conf(
            "",
            "{ addr = \"10.0.0.11:8080\", drain = true }, \"10.0.0.13:8080\"",
        ),
    );
    let out = logged(both);
    let added = out
        .find("change=\"backend_added\"")
        .or(out.find("change=backend_added"));
    assert!(added.is_some(), "{out}");
    assert!(
        out.contains("field=drain") && out.contains("from=false") && out.contains("to=true"),
        "{out}"
    );
}

// spectra-scenario: service-config / More than fifty differences / change values
#[test]
fn service_settings_change_once_per_service() {
    let base = conf("interval = \"2s\"\ntimeout = \"1s\"", B1);
    let c = one(&base, &conf("interval = \"5s\"\ntimeout = \"1s\"", B1));
    assert_eq!(
        c,
        Change::ServiceChanged {
            service: "a.svc.emfor".into(),
            field: "interval",
            from: "2s".into(),
            to: "5s".into()
        }
    );
}

// spectra-scenario: service-config / Timeout change keeps the state
#[test]
fn a_timeout_change_keeps_the_state() {
    let old = conf("interval = \"2s\"\ntimeout = \"1s\"", B1);
    let new = conf("interval = \"2s\"\ntimeout = \"500ms\"", B1);
    let c = changes(&old, &new);
    assert_eq!(c.len(), 2, "{c:?}");
    assert!(
        matches!(
            &c[0],
            Change::ServiceChanged {
                field: "timeout",
                ..
            }
        ),
        "{c:?}"
    );
    match &c[1] {
        Change::CheckDefinitionChanged {
            resets_state,
            digest_from,
            digest_to,
            ..
        } => {
            assert!(!resets_state);
            assert_ne!(digest_from, digest_to);
        }
        other => panic!("{other:?}"),
    }
}

// spectra-scenario: service-config / Check options changed without exposing them
// spectra-scenario: service-config / More than fifty differences / change values
#[test]
fn changed_check_options_are_not_shown() {
    let old = config::parse(
        "[services.\"a.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.11:8080\"]\n[services.\"a.svc.emfor\".check_options]\nsend = \"hello-secret-one\"\nexpect = \"ready-one\"\n",
        &Registry::builtin(),
    )
    .unwrap();
    let new = config::parse(
        "[services.\"a.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.11:8080\"]\n[services.\"a.svc.emfor\".check_options]\nsend = \"hello-secret-two\"\nexpect = \"ready-two\"\n",
        &Registry::builtin(),
    )
    .unwrap();
    let c = changes(&old, &new);
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(matches!(
        c[0],
        Change::CheckDefinitionChanged {
            resets_state: true,
            ..
        }
    ));
    let out = logged(c);
    assert!(out.contains("resets_state=true"), "{out}");
    for secret in ["hello-secret", "ready-one", "ready-two"] {
        assert!(!out.contains(secret), "{out}");
    }
    // Another check name is both a service change and a definition change.
    let http = conf("", B1);
    let other = config::parse(
        "[services.\"a.svc.emfor\"]\ncheck = \"http\"\nbackends = [\"10.0.0.11:8080\"]\n",
        &Registry::builtin(),
    )
    .unwrap();
    let c = changes(&http, &other);
    assert!(
        matches!(&c[0], Change::ServiceChanged { field: "check", .. }),
        "{c:?}"
    );
    assert!(
        matches!(
            c[1],
            Change::CheckDefinitionChanged {
                resets_state: true,
                ..
            }
        ),
        "{c:?}"
    );
}

fn peers(members: &[&str], encrypt: bool, previous: Option<&str>) -> PeersConfig {
    PeersConfig {
        listen: "10.0.0.5:5302".parse().unwrap(),
        members: members.iter().map(|m| m.parse().unwrap()).collect(),
        secret_file: PathBuf::from("/etc/emfor/peers.key"),
        previous_secret_file: previous.map(PathBuf::from),
        encrypt,
        accept_plaintext: true,
        cipher: crate::config::Cipher::default(),
    }
}

// spectra-scenario: service-config / Peer group changes
// spectra-scenario: service-config / More than fifty differences / change values
#[test]
fn peer_group_changes_without_secret_content() {
    let mut old = conf("", B1);
    let mut new = conf("", B1);
    old.peers = Some(peers(&["10.0.0.5:5302", "10.0.0.6:5302"], false, None));
    new.peers = Some(peers(
        &["10.0.0.5:5302", "10.0.0.7:5302"],
        true,
        Some("/etc/emfor/old.key"),
    ));
    let c = changes(&old, &new);
    assert_eq!(
        c,
        vec![
            Change::PeerMemberRemoved {
                peer: "10.0.0.6:5302".into()
            },
            Change::PeerMemberAdded {
                peer: "10.0.0.7:5302".into()
            },
            Change::PeersChanged {
                field: "encrypt",
                from: "false".into(),
                to: "true".into(),
                applies: "now"
            },
            Change::PeersChanged {
                field: "previous_secret_file",
                from: "-".into(),
                to: "/etc/emfor/old.key".into(),
                applies: "next_start"
            },
        ]
    );
    let out = logged(c);
    assert!(out.contains("applies=next_start"), "{out}");
    // Paths of secret files, never their content or a hash of it.
    assert!(!out.contains("sha256"), "{out}");
}

// spectra-scenario: service-config / More than fifty differences
#[test]
fn more_than_fifty_changes_are_cut_with_a_count() {
    let many: Vec<Change> = (0..80)
        .map(|i| Change::ServiceAdded {
            service: format!("s{i:02}.svc.emfor"),
            backends: 1,
        })
        .collect();
    let out = logged(many);
    assert_eq!(out.matches("service_added").count(), 50, "{out}");
    assert_eq!(out.matches("omitted=30").count(), 1, "{out}");
    assert_eq!(out.lines().count(), 51, "{out}");
    assert_eq!(capped(vec![Change::More { omitted: 1 }; 3]).len(), 3);
}

// spectra-scenario: service-config / More than fifty differences / change values
#[test]
fn a_changed_default_changes_every_service_that_uses_it() {
    let text = |interval: &str| {
        format!(
            "[defaults]\ninterval = \"{interval}\"\n{}",
            ["a", "b", "c"]
                .iter()
                .map(|n| format!(
                    "[services.\"{n}.svc.emfor\"]\ncheck = \"tcp\"\nbackends = [\"10.0.0.1:80\"]\n"
                ))
                .collect::<String>()
        )
    };
    let parse = |t: String| config::parse(&t, &Registry::builtin()).unwrap();
    let c = changes(&parse(text("2s")), &parse(text("5s")));
    assert_eq!(c.len(), 3, "{c:?}");
    assert!(
        c.iter().all(|c| matches!(
            c,
            Change::ServiceChanged {
                field: "interval",
                ..
            }
        )),
        "{c:?}"
    );
}

// spectra-scenario: service-config / Cipher changed
#[test]
fn cipher_change_is_one_peers_changed_line() {
    let mut old = conf("", B1);
    let mut new = conf("", B1);
    let members = ["10.0.0.5:5302", "10.0.0.6:5302"];
    old.peers = Some(peers(&members, true, None));
    let mut gcm = peers(&members, true, None);
    gcm.cipher = crate::config::Cipher::Aes256Gcm;
    new.peers = Some(gcm);
    let c = changes(&old, &new);
    assert_eq!(
        c,
        vec![Change::PeersChanged {
            field: "cipher",
            from: "chacha20-poly1305".into(),
            to: "aes-256-gcm".into(),
            applies: "now"
        }]
    );
    assert!(changes(&new, &new).is_empty(), "same cipher, no line");
}

// spectra-scenario: service-config / Peer group changes
#[test]
fn secret_file_path_change_is_a_peers_changed_line_without_content() {
    let mut old = conf("", B1);
    let mut new = conf("", B1);
    let members = ["10.0.0.5:5302", "10.0.0.6:5302"];
    old.peers = Some(peers(&members, true, None));
    let mut moved = peers(&members, true, None);
    moved.secret_file = PathBuf::from("/etc/emfor/peers-new.key");
    new.peers = Some(moved);
    let c = changes(&old, &new);
    assert_eq!(
        c,
        vec![Change::PeersChanged {
            field: "secret_file",
            from: "/etc/emfor/peers.key".into(),
            to: "/etc/emfor/peers-new.key".into(),
            applies: "now"
        }]
    );
    let out = logged(c);
    assert!(out.contains("field=secret_file"), "{out}");
    assert!(out.contains("applies=now"), "{out}");
    assert!(!out.contains("sha256"), "{out}");
}
