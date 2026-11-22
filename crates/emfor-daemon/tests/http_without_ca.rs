//! `http` checks of a daemon that has no system CA certificates, end to end:
//! `SSL_CERT_FILE` and `SSL_CERT_DIR` name nothing, which is what a host
//! without the `ca-certificates` package looks like to the daemon. Plain
//! `http` must work there; `https` with verification must say what is
//! missing.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;

use common::{Daemon, Reader, WAIT, listener, wait_within};
use emfor_format::HealthState;

/// A server that answers every request with `status` and an empty body.
fn server(status: &'static str) -> String {
    let (l, addr): (TcpListener, String) = listener();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(mut s) = conn else { continue };
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                );
            });
        }
    });
    addr
}

fn service(name: &str, backend: &str, options: &str) -> String {
    format!(
        "[services.\"{name}\"]\ncheck = \"http\"\nbackends = [\"{backend}\"]\ninterval = \"1s\"\ntimeout = \"1s\"\nrise = 1\nfall = 1\njitter = \"0%\"\n\n[services.\"{name}\".check_options]\npath = \"/\"\n{options}\n"
    )
}

// spectra-scenario: health-checking / Plain http without CA certificates
// spectra-scenario: health-checking / https without CA certificates
#[test]
fn http_works_and_verified_https_names_the_missing_certificates() {
    let ok = server("200 OK");
    let unavailable = server("503 Service Unavailable");
    let tls = server("200 OK");
    let none = Path::new("/nonexistent-ca-certificates");
    let d = Daemon::spawn_with(
        |dir| {
            format!(
                "{}{}{}{}",
                common::store_header(dir),
                service("ok.svc.emfor", &ok, "scheme = \"http\"\n"),
                service("busy.svc.emfor", &unavailable, "scheme = \"http\"\n"),
                service("tls.svc.emfor", &tls, "scheme = \"https\"\n"),
            )
        },
        &[("SSL_CERT_FILE", none), ("SSL_CERT_DIR", none)],
    );
    d.wait_log("emfor started");
    let r = Reader::open(&d.store());
    wait_within("every backend has a result", WAIT, || {
        ["ok", "busy", "tls"].iter().all(|n| {
            r.backend(&format!("{n}.svc.emfor"))
                .is_some_and(|b| b.checked_at.is_some())
        })
    });
    let ok = r.backend("ok.svc.emfor").unwrap();
    assert_eq!(ok.state, HealthState::Healthy, "{:?}", ok.reason);
    let busy = r.backend("busy.svc.emfor").unwrap();
    assert_eq!(busy.state, HealthState::Unhealthy);
    assert!(
        busy.reason.as_deref().is_some_and(|m| m.contains("503")),
        "{:?}",
        busy.reason
    );
    let tls = r.backend("tls.svc.emfor").unwrap();
    let reason = tls.reason.unwrap_or_default();
    assert!(reason.contains("system CA certificates"), "{reason}");
    assert!(reason.contains("tls_verify = false"), "{reason}");
    let (code, log) = d.stop();
    assert_eq!(code, 0, "{log}");
}
