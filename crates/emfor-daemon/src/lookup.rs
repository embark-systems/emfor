//! `emfor lookup`: the backend an application would get for a service, and
//! with `--explain` why (spec lookup-command).
//!
//! The answer comes from `emfor_client`, so it is what the client library
//! gives on the same store. `--explain` shows the candidate tiers and the
//! reason of every backend; it never advances a rotation counter of the
//! process that a later lookup could see.

use std::net::IpAddr;
use std::path::Path;
use std::process::ExitCode;

use emfor_client::{Client, Error, ErrorCode, Explanation, explain_record};
use emfor_format::{ServiceRecord, rules::Tier};
use serde_json::{Value, json};

use crate::config;
use crate::schedule::unix_ms;
use crate::status::{Reader, age};

/// An address was printed.
const EXIT_ANSWER: u8 = 0;
/// No backend can be chosen (`ALL_DOWN`, or none of the family with `--dns`).
const EXIT_NO_BACKEND: u8 = 1;
/// The service does not exist, the command line or the config is invalid.
const EXIT_USAGE: u8 = 2;
/// The store cannot be used or has a format this command does not read.
const EXIT_STORE: u8 = 3;

/// The address family of a DNS query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DnsFamily {
    #[value(name = "A", alias = "a")]
    A,
    #[value(name = "AAAA", alias = "aaaa")]
    Aaaa,
}

impl DnsFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            DnsFamily::A => "A",
            DnsFamily::Aaaa => "AAAA",
        }
    }
}

#[derive(Debug, clap::Args)]
pub struct LookupArgs {
    /// The service to look up, such as api.svc.emfor.
    pub service: String,
    /// The affinity key of `sticky` services; the same key gets the same
    /// backend while it is healthy.
    #[arg(long, value_name = "KEY")]
    pub key: Option<String>,
    /// Say why: the tier of the candidates and the reason of every backend.
    #[arg(long)]
    pub explain: bool,
    /// Print one JSON object instead of text.
    #[arg(long)]
    pub json: bool,
    /// Answer as the DNS server of the daemon would (A or AAAA).
    #[arg(long, value_name = "FAMILY", value_enum)]
    pub dns: Option<DnsFamily>,
}

/// The exit status of a failed lookup.
fn exit_for(code: ErrorCode) -> u8 {
    match code {
        ErrorCode::AllDown => EXIT_NO_BACKEND,
        ErrorCode::NotFound => EXIT_USAGE,
        ErrorCode::Unavailable | ErrorCode::Incompatible => EXIT_STORE,
    }
}

/// Run `emfor lookup`.
pub fn main(config_path: &Path, args: &LookupArgs) -> ExitCode {
    let store = match config::load_store(config_path) {
        Ok(s) => s,
        Err(errors) => {
            for e in &errors {
                eprintln!("{e}");
            }
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let explain = args.explain || args.dns.is_some();
    if let Some(family) = args.dns {
        return dns_mode(&store.path, &store, args, family);
    }
    let client = Client::for_store(store.path.clone());
    let key = args.key.as_deref();
    if !explain && !args.json {
        return match client.lookup(&args.service, key) {
            Ok(address) => {
                println!("{address}");
                ExitCode::from(EXIT_ANSWER)
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(exit_for(e.code))
            }
        };
    }
    // `--explain` and `--json`: the answer and its explanation come from one
    // reading of the store.
    let (answer, explanation) = match client.lookup_explained(&args.service, key) {
        Ok((answer, explanation)) => (answer, Some(explanation)),
        Err(e) => (Err(e), None),
    };
    print_result(
        args,
        explain,
        &store.path,
        "client",
        None,
        &answer.map(Some),
        explanation.as_ref(),
    )
}

/// `--dns A|AAAA`: explain what the DNS server of the daemon would answer,
/// from the store (spec lookup-command, "DNS mode explains the answer of the
/// DNS server").
fn dns_mode(
    store_path: &Path,
    store: &config::StoreConfig,
    args: &LookupArgs,
    family: DnsFamily,
) -> ExitCode {
    let snap = match Reader::open(store).and_then(|r| r.read()) {
        Ok(s) => s,
        Err(failure) => {
            let e = Error {
                code: ErrorCode::Unavailable,
                message: failure.0,
            };
            return print_result(args, true, store_path, "dns", Some(family), &Err(e), None);
        }
    };
    let wanted = config::normalize_service_name(&args.service);
    let Some(record) = snap.services.iter().find(|r| r.name == wanted) else {
        let e = Error {
            code: ErrorCode::NotFound,
            message: format!("no service named {wanted:?}"),
        };
        return print_result(args, true, store_path, "dns", Some(family), &Err(e), None);
    };
    let now = unix_ms();
    let mut e = dns_explanation(record, family, snap.site.as_deref(), now);
    e.store_path = store_path.display().to_string();
    e.heartbeat_age_ms = now.saturating_sub(snap.heartbeat_ms);
    e.max_staleness_ms = DNS_HEARTBEAT_LIMIT_MS;
    e.stale = snap.heartbeat_ms == 0 || e.heartbeat_age_ms > DNS_HEARTBEAT_LIMIT_MS;
    let answer: Result<Option<String>, Error> = match (&e.error, &e.selection.chosen) {
        (Some(err), _) => Err(err.clone()),
        (None, chosen) => Ok(chosen.clone()),
    };
    print_result(
        args,
        true,
        store_path,
        "dns",
        Some(family),
        &answer,
        Some(&e),
    )
}

/// How old the heartbeat may be before the daemon does not look like it is
/// running (the client's default limit); the DNS server answers whatever it is.
const DNS_HEARTBEAT_LIMIT_MS: u64 = 3000;

/// The explanation of an A or AAAA answer: the backends of the family go
/// through the same tiers as a client's (the store counts as fresh, there is
/// no key), and the weighted rule is the daemon's smooth weighted
/// round-robin.
fn dns_explanation(
    record: &ServiceRecord,
    family: DnsFamily,
    site: Option<&str>,
    now: u64,
) -> Explanation {
    let mut rec = record.clone();
    rec.backends.retain(|b| {
        b.ip.parse::<IpAddr>().is_ok_and(|ip| match family {
            DnsFamily::A => ip.is_ipv4(),
            DnsFamily::Aaaa => ip.is_ipv6(),
        })
    });
    let mut e = explain_record(&rec, false, None, site, now, true);
    if rec.backends.is_empty() {
        e.tier = Tier::None;
        e.selection.rule = format!("the service has no {} backend", family.as_str());
        e.error = Some(Error {
            code: ErrorCode::AllDown,
            message: format!("service {} has no {} backend", record.name, family.as_str()),
        });
    }
    e
}

/// Print the outcome of a lookup (text or JSON) and return its exit status.
/// `explanation` is `None` when the store could not be read.
pub fn print_result(
    args: &LookupArgs,
    explain: bool,
    store_path: &Path,
    via: &str,
    family: Option<DnsFamily>,
    answer: &Result<Option<String>, Error>,
    explanation: Option<&Explanation>,
) -> ExitCode {
    if args.json {
        println!(
            "{:#}",
            json_result(args, explain, store_path, via, family, answer, explanation)
        );
    } else {
        for line in text_result(explain, store_path, via, family, answer, explanation) {
            println!("{line}");
        }
        if let Err(e) = answer {
            eprintln!("{e}");
        }
    }
    ExitCode::from(match answer {
        Ok(_) => EXIT_ANSWER,
        Err(e) => exit_for(e.code),
    })
}

/// The heartbeat of a daemon that stopped is 0, which is a very old time.
fn heartbeat_text(age_ms: u64) -> String {
    const TEN_YEARS_MS: u64 = 10 * 365 * 24 * 3600 * 1000;
    if age_ms > TEN_YEARS_MS {
        "no heartbeat (the daemon has stopped)".to_string()
    } else {
        format!("heartbeat {} ago", age(age_ms))
    }
}

/// The text output. Without `--explain` it is the address alone.
fn text_result(
    explain: bool,
    store_path: &Path,
    via: &str,
    family: Option<DnsFamily>,
    answer: &Result<Option<String>, Error>,
    explanation: Option<&Explanation>,
) -> Vec<String> {
    let mut out = Vec::new();
    let Some(e) = explanation.filter(|_| explain) else {
        if let Ok(Some(a)) = answer {
            out.push(a.clone());
        }
        return out;
    };
    out.push(format!("service: {}", e.service));
    let how = match (via, family) {
        ("dns", Some(f)) => format!("as the DNS server answers an {} query", f.as_str()),
        _ => "as a client library answers".to_string(),
    };
    out.push(format!("answering: {how}"));
    let state = if e.stale {
        format!(
            "stale: the heartbeat is older than {}",
            age(e.max_staleness_ms)
        )
    } else {
        "fresh".to_string()
    };
    out.push(format!(
        "store: {} ({}, {state})",
        store_path.display(),
        heartbeat_text(e.heartbeat_age_ms)
    ));
    out.push(format!(
        "balance: {}  on_all_down: {}",
        e.balance.as_str(),
        e.on_all_down.as_str()
    ));
    out.push(format!("tier: {}", e.tier.describe()));
    out.push("candidates:".to_string());
    if e.candidates.is_empty() {
        out.push("  (none)".to_string());
    }
    out.extend(e.candidates.iter().map(|id| format!("  {id}")));
    if !e.excluded.is_empty() {
        out.push("excluded:".to_string());
        out.extend(e.excluded.iter().map(|(id, why)| format!("  {id}  {why}")));
    }
    if !e.slow_start.is_empty() {
        out.push("slow start:".to_string());
        out.extend(
            e.slow_start
                .iter()
                .map(|(id, step, w)| format!("  {id}  step {step} of 10, weight {w}")),
        );
    }
    out.push(format!("selection: {}", e.selection.rule));
    if via == "client" && !e.selection.deterministic && e.error.is_none() {
        out.push(
            "  the actual choice depends on the rotation start of the process, which is random in a new process"
                .to_string(),
        );
    }
    if via == "dns" && e.stale {
        out.push(
            "note: the daemon does not look like it is running, so no DNS server is answering from this store"
                .to_string(),
        );
    }
    if via == "dns" {
        out.push(
            "note: this reads the store, which the daemon updates up to about one second after it decides a reason, so it can differ from the live DNS answer for that second; the rotation counters of the daemon are in its memory and are not shown"
                .to_string(),
        );
    }
    match answer {
        Ok(Some(a)) => out.push(format!("answer: {a}")),
        Ok(None) => {}
        Err(err) => out.push(format!("no answer: {err}")),
    }
    out
}

/// The `--json` object (spec lookup-command, "JSON output is stable and
/// only grows").
fn json_result(
    args: &LookupArgs,
    explain: bool,
    store_path: &Path,
    via: &str,
    family: Option<DnsFamily>,
    answer: &Result<Option<String>, Error>,
    explanation: Option<&Explanation>,
) -> Value {
    let result = match answer {
        Ok(a) => json!({"status": "ok", "address": a}),
        Err(e) => json!({"status": "error",
            "error": {"code": e.code.as_str(), "message": e.message}}),
    };
    let mut v = json!({
        "service": explanation.map_or(args.service.as_str(), |e| e.service.as_str()),
        "via": via,
        "family": family.map(DnsFamily::as_str),
        "store": {
            "path": store_path.display().to_string(),
            "heartbeat_age_ms": explanation.map(|e| e.heartbeat_age_ms),
            "stale": explanation.map(|e| e.stale),
        },
        "balance": explanation.map(|e| e.balance.as_str()),
        "on_all_down": explanation.map(|e| e.on_all_down.as_str()),
        "result": result,
    });
    if let (true, Some(e)) = (explain, explanation) {
        v["explain"] = json!({
            "tier": e.tier.as_str(),
            "candidates": e.candidates.iter().map(|id| json!({"id": id})).collect::<Vec<_>>(),
            "excluded": e.excluded.iter()
                .map(|(id, why)| json!({"id": id, "reason": why})).collect::<Vec<_>>(),
            "slow_start": e.slow_start.iter()
                .map(|(id, step, w)| json!({"id": id, "step": step, "weight": w}))
                .collect::<Vec<_>>(),
            "selection": {
                "rule": e.selection.rule,
                "deterministic": e.selection.deterministic,
                "chosen": e.selection.chosen,
            },
        });
    }
    v
}
