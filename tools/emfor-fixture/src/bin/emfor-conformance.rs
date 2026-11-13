//! `emfor-conformance [--cases <cases.json>] [--vectors <sticky-v1.json>]
//! [--timeout-ms <ms>] -- <program...>`
//!
//! Runs the conformance cases and pick vectors (default: tests/conformance/v1/)
//! against a client's lookup program, one process per lookup:
//!
//! ```text
//! <program...> <name> <affinity-key, "" for none> <count>
//! ```
//!
//! with `EMFOR_STORE` and `EMFOR_MAX_STALENESS` set for the case. The program
//! does `count` lookups (the length of `expect_all_of`, the sum of
//! `expect_counts`, or 1) in one process and prints one line per lookup:
//! `OK <address>` or `ERR <CODE> <message>`. A run that takes longer than the
//! timeout (default 10 s) is killed and fails. Exits 1 if any lookup fails its
//! expectation, 2 on a usage or setup error.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/conformance/v1");

struct Run<'a> {
    program: &'a [String],
    timeout: Duration,
    lookups: usize,
    failures: Vec<String>,
}

/// One answer line from the lookup program. The message is shown in
/// failure reports (through `Debug`).
#[derive(Debug)]
enum Answer {
    Ok(String),
    Err(String, #[allow(dead_code)] String),
}

impl Run<'_> {
    /// Run the program for one lookup entry and check its answers.
    fn check(&mut self, what: &str, store: &Path, staleness: &str, lookup: &Value) {
        self.lookups += 1;
        let name = lookup["name"].as_str().unwrap_or_default();
        let key = lookup
            .get("affinity_key")
            .and_then(Value::as_str)
            .unwrap_or("");
        let all_of = lookup.get("expect_all_of").and_then(Value::as_array);
        let counts = lookup.get("expect_counts").and_then(Value::as_object);
        let count = match (all_of, counts) {
            (Some(a), _) => a.len(),
            (None, Some(c)) => c.values().filter_map(Value::as_u64).sum::<u64>() as usize,
            (None, None) => 1,
        };
        let what = format!("{what}, lookup {lookup}");
        let answers = match self.answers(store, staleness, name, key, count) {
            Ok(a) => a,
            Err(e) => return self.failures.push(format!("{what}: {e}")),
        };
        let fail = if let Some(want) = lookup.get("expect").and_then(Value::as_str) {
            match &answers[..] {
                [Answer::Ok(a)] if a == want => None,
                other => Some(format!("want {want}, got {other:?}")),
            }
        } else if let Some(want) = lookup.get("expect_error").and_then(Value::as_str) {
            match &answers[..] {
                [Answer::Err(code, _)] if code == want => None,
                other => Some(format!("want error {want}, got {other:?}")),
            }
        } else if let Some(want) = all_of {
            let want: BTreeSet<&str> = want.iter().filter_map(Value::as_str).collect();
            let got: Option<BTreeSet<&str>> = answers
                .iter()
                .map(|a| match a {
                    Answer::Ok(s) => Some(s.as_str()),
                    Answer::Err(..) => None,
                })
                .collect();
            match got {
                Some(g) if g == want => None,
                _ => Some(format!("want all of {want:?}, got {answers:?}")),
            }
        } else if let Some(want) = counts {
            let want: BTreeMap<&str, u64> = want
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_u64().unwrap_or(0)))
                .collect();
            let mut got: BTreeMap<&str, u64> = BTreeMap::new();
            let mut errors = false;
            for a in &answers {
                match a {
                    Answer::Ok(s) => *got.entry(s.as_str()).or_default() += 1,
                    Answer::Err(..) => errors = true,
                }
            }
            (errors || got != want).then(|| format!("want counts {want:?}, got {answers:?}"))
        } else {
            Some("lookup has no expectation".into())
        };
        if let Some(f) = fail {
            self.failures.push(format!("{what}: {f}"));
        }
    }

    fn answers(
        &self,
        store: &Path,
        staleness: &str,
        name: &str,
        key: &str,
        count: usize,
    ) -> Result<Vec<Answer>, String> {
        let mut child = Command::new(&self.program[0])
            .args(&self.program[1..])
            .args([name, key, &count.to_string()])
            .env("EMFOR_STORE", store)
            .env("EMFOR_MAX_STALENESS", staleness)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", self.program[0]))?;
        // Read both pipes while waiting, so a chatty program cannot block.
        let reader = |mut r: Box<dyn Read + Send>| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = r.read_to_end(&mut buf);
                buf
            })
        };
        let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else {
            return Err(format!("cannot read the output of {}", self.program[0]));
        };
        let stdout = reader(Box::new(out));
        let stderr = reader(Box::new(err));
        let start = Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
                break s;
            }
            if start.elapsed() > self.timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("program timed out after {:?}", self.timeout));
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let stdout = stdout.join().unwrap_or_default();
        let stderr = stderr.join().unwrap_or_default();
        if !status.success() {
            return Err(format!(
                "program exited with {status}: {}",
                String::from_utf8_lossy(&stderr).trim()
            ));
        }
        let text = String::from_utf8_lossy(&stdout);
        let answers: Vec<Answer> = text
            .lines()
            .map(|l| {
                if let Some(a) = l.strip_prefix("OK ") {
                    Ok(Answer::Ok(a.to_string()))
                } else if let Some(e) = l.strip_prefix("ERR ") {
                    let (code, msg) = e.split_once(' ').unwrap_or((e, ""));
                    Ok(Answer::Err(code.to_string(), msg.to_string()))
                } else {
                    Err(format!("unexpected output line {l:?}"))
                }
            })
            .collect::<Result<_, _>>()?;
        if answers.len() != count {
            return Err(format!(
                "want {count} lines, got {}: {text:?}",
                answers.len()
            ));
        }
        Ok(answers)
    }
}

/// `expect_counts`, when present, must map addresses to positive integers;
/// anything else is a setup error, not a lookup failure.
fn check_counts(what: &str, lookup: &Value) -> Result<(), String> {
    let Some(c) = lookup.get("expect_counts") else {
        return Ok(());
    };
    let ok = c
        .as_object()
        .is_some_and(|m| !m.is_empty() && m.values().all(|v| v.as_u64().is_some_and(|n| n > 0)));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{what}: expect_counts must map addresses to positive integers, got {c}"
        ))
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn backend(id: &str) -> Value {
    let (ip, port) = id.rsplit_once(':').unwrap_or((id, "0"));
    let ip = ip.trim_start_matches('[').trim_end_matches(']');
    json!({"id": id, "ip": ip, "port": port.parse::<u16>().unwrap_or(0),
           "state": "healthy", "checked_at": 1, "reason": null})
}

fn run(args: &[String]) -> Result<bool, String> {
    let mut cases = PathBuf::from(DATA).join("cases.json");
    let mut vectors = PathBuf::from(DATA).join("sticky-v1.json");
    let mut timeout = Duration::from_secs(10);
    let mut i = 0;
    while i < args.len() && args[i] != "--" {
        match (args[i].as_str(), args.get(i + 1)) {
            ("--cases", Some(p)) => cases = p.into(),
            ("--vectors", Some(p)) => vectors = p.into(),
            ("--timeout-ms", Some(ms)) => {
                let ms = ms.parse().map_err(|_| format!("bad --timeout-ms {ms:?}"))?;
                timeout = Duration::from_millis(ms);
            }
            _ => return Err(format!("unknown argument {:?}", args[i])),
        }
        i += 2;
    }
    let program = args
        .get(i + 1..)
        .filter(|p| !p.is_empty())
        .ok_or("missing -- <program...>")?;
    let mut r = Run {
        program,
        timeout,
        lookups: 0,
        failures: Vec::new(),
    };
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;

    let cases_json = read_json(&cases)?;
    let list = cases_json.as_array().ok_or("cases.json is not a list")?;
    for (n, case) in list.iter().enumerate() {
        let what = format!("case {:?}", case["case"].as_str().unwrap_or("?"));
        let store = dir.path().join(format!("case-{n}.mdb"));
        let max_ms = case["max_staleness_ms"].as_u64().unwrap_or(3000);
        if !case["store"].is_null() {
            emfor_fixture::write_store(&case["store"], max_ms, emfor_fixture::now_ms(), &store)
                .map_err(|e| format!("{what}: {e}"))?;
        }
        let staleness = match case.get("max_staleness").and_then(Value::as_str) {
            Some(raw) => raw.to_string(),
            None => format!("{max_ms}ms"),
        };
        for lookup in case["lookups"].as_array().into_iter().flatten() {
            check_counts(&what, lookup)?;
            r.check(&what, &store, &staleness, lookup);
        }
    }

    let v = read_json(&vectors)?;
    let picks = v["picks"]
        .as_array()
        .ok_or("sticky vectors have no picks")?;
    let services: Vec<Value> = picks
        .iter()
        .enumerate()
        .map(|(n, p)| {
            let backends: Vec<Value> = p["ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(backend)
                .collect();
            json!({"name": format!("pick-{n}.svc.emfor"), "on_all_down": "closed",
                   "balance": "sticky", "all_down": false, "backends": backends})
        })
        .collect();
    let store = dir.path().join("picks.mdb");
    let picks_store =
        json!({"schema_version": "1", "heartbeat": "fresh", "raw": {}, "services": services});
    emfor_fixture::write_store(&picks_store, 60_000, emfor_fixture::now_ms(), &store)?;
    for (n, p) in picks.iter().enumerate() {
        let lookup = json!({"name": format!("pick-{n}.svc.emfor"), "affinity_key": p["key"],
                            "expect": p["expect"]});
        r.check("sticky pick", &store, "60s", &lookup);
    }

    for f in &r.failures {
        println!("FAIL {f}");
    }
    println!(
        "conformance: {} cases, {} picks, {} lookups, {} failures",
        list.len(),
        picks.len(),
        r.lookups,
        r.failures.len()
    );
    Ok(r.failures.is_empty())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("emfor-conformance: {e}");
            std::process::exit(2);
        }
    }
}
