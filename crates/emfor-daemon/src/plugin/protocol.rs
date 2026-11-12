//! Plugin protocol version 1: one JSON object per line, daemon to plugin on
//! the plugin's stdin, plugin to daemon on its stdout. See
//! docs/plugins/protocol-v1.md.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The protocol version this daemon speaks.
pub const PROTOCOL: u32 = 1;

/// A message from the daemon to a plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Request {
    Hello {
        protocol: u32,
    },
    Validate {
        id: u64,
        options: serde_json::Map<String, serde_json::Value>,
    },
    Check {
        id: u64,
        service: String,
        target: Target,
        options: serde_json::Map<String, serde_json::Value>,
        timeout_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub ip: String,
    pub port: u16,
}

/// A message from a plugin to the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Response {
    Hello {
        protocol: u32,
        name: String,
        version: String,
    },
    Validate {
        id: u64,
        errors: Vec<ValidateError>,
    },
    Check {
        id: u64,
        /// `up`, `down`, or `error`. Kept as raw JSON so that any other
        /// value, or none, still reaches its request as an invalid response
        /// instead of failing to parse.
        #[serde(default)]
        result: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl Response {
    /// Request id this answer belongs to; `None` for the handshake.
    pub fn id(&self) -> Option<u64> {
        match self {
            Response::Hello { .. } => None,
            Response::Validate { id, .. } | Response::Check { id, .. } => Some(*id),
        }
    }
}

/// One problem in `check_options`; `key` is relative to `check_options`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidateError {
    pub key: String,
    pub message: String,
}

/// `timeout_ms` sent with a check: the service timeout minus a margin of
/// `min(100ms, timeout / 10)`, so the plugin can answer `down` in time.
pub fn timeout_ms(timeout: Duration) -> u64 {
    let margin = (timeout / 10).min(Duration::from_millis(100));
    (timeout - margin).as_millis() as u64
}

/// Convert `check_options` to JSON. TOML datetimes become RFC 3339 strings.
pub fn options_to_json(options: &toml::Table) -> serde_json::Map<String, serde_json::Value> {
    options
        .iter()
        .map(|(k, v)| (k.clone(), value_to_json(v)))
        .collect()
}

fn value_to_json(v: &toml::Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        toml::Value::String(s) => J::String(s.clone()),
        toml::Value::Integer(i) => J::from(*i),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or(J::Null),
        toml::Value::Boolean(b) => J::Bool(*b),
        toml::Value::Datetime(d) => J::String(d.to_string()),
        toml::Value::Array(a) => J::Array(a.iter().map(value_to_json).collect()),
        toml::Value::Table(t) => J::Object(options_to_json(t)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLES: &str = include_str!("../../../../docs/plugins/protocol-v1-examples.jsonl");

    #[test]
    fn examples_round_trip() {
        let mut requests = 0;
        let mut responses = 0;
        for line in EXAMPLES.lines().filter(|l| !l.trim().is_empty()) {
            let original: serde_json::Value = serde_json::from_str(line).unwrap();
            if let Ok(r) = serde_json::from_str::<Request>(line)
                && serde_json::to_value(&r).unwrap() == original
            {
                requests += 1;
                continue;
            }
            let r: Response = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("line is neither request nor response: {line}: {e}"));
            assert_eq!(serde_json::to_value(&r).unwrap(), original, "line: {line}");
            responses += 1;
        }
        // hello, validate, check in each direction.
        assert!(requests >= 3, "{requests} requests");
        assert!(responses >= 3, "{responses} responses");
    }

    #[test]
    fn timeout_ms_examples() {
        assert_eq!(timeout_ms(Duration::from_secs(1)), 900);
        assert_eq!(timeout_ms(Duration::from_secs(5)), 4900);
        assert_eq!(timeout_ms(Duration::from_millis(200)), 180);
    }

    #[test]
    fn options_convert_to_json() {
        let t: toml::Table = r#"
            s = "x"
            n = 3
            f = 1.5
            b = true
            d = 2026-11-12T10:00:00Z
            a = [1, "two"]
            [sub]
            k = "v"
        "#
        .parse()
        .unwrap();
        let j = serde_json::Value::Object(options_to_json(&t));
        assert_eq!(
            j,
            serde_json::json!({
                "s": "x", "n": 3, "f": 1.5, "b": true,
                "d": "2026-11-12T10:00:00Z",
                "a": [1, "two"], "sub": {"k": "v"}
            })
        );
    }

    #[test]
    fn check_answer_without_reason() {
        let r: Response = serde_json::from_str(r#"{"type":"check","id":4,"result":"up"}"#).unwrap();
        assert_eq!(
            r,
            Response::Check {
                id: 4,
                result: "up".into(),
                reason: None
            }
        );
        assert_eq!(r.id(), Some(4));
    }

    #[test]
    fn check_answer_with_odd_result_still_parses() {
        for line in [
            r#"{"type":"check","id":7,"result":1}"#,
            r#"{"type":"check","id":7}"#,
        ] {
            let r: Response = serde_json::from_str(line).unwrap();
            assert_eq!(r.id(), Some(7), "{line}");
        }
    }
}
