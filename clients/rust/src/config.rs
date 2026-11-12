//! Configuration from the environment.

use std::path::PathBuf;

pub const DEFAULT_STORE: &str = "/var/lib/emfor/state.mdb";
pub const DEFAULT_MAX_STALENESS_MS: u64 = 3000;
pub const ENV_STORE: &str = "EMFOR_STORE";
pub const ENV_MAX_STALENESS: &str = "EMFOR_MAX_STALENESS";

/// Store path and maximum staleness from the environment. An invalid
/// staleness is kept as an error so every lookup reports it.
pub fn from_env() -> (PathBuf, Result<u64, String>) {
    let path = std::env::var_os(ENV_STORE)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_STORE));
    let max = match std::env::var_os(ENV_MAX_STALENESS) {
        None => Ok(DEFAULT_MAX_STALENESS_MS),
        Some(v) => match v.to_str() {
            Some(s) => parse_max_staleness(s),
            None => Err(invalid(&v.to_string_lossy())),
        },
    };
    (path, max)
}

fn invalid(value: &str) -> String {
    format!(
        "{ENV_MAX_STALENESS}={value:?} is invalid: use a positive whole number followed by ms or s, such as 3s or 1500ms"
    )
}

/// Parse a staleness such as `3s` or `1500ms` into milliseconds.
pub fn parse_max_staleness(s: &str) -> Result<u64, String> {
    let (digits, factor) = if let Some(d) = s.strip_suffix("ms") {
        (d, 1)
    } else if let Some(d) = s.strip_suffix('s') {
        (d, 1000)
    } else {
        return Err(invalid(s));
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid(s));
    }
    match digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(factor))
    {
        Some(ms) if ms > 0 => Ok(ms),
        _ => Err(invalid(s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staleness_values() {
        for (v, want) in [("3s", Some(3000)), ("1500ms", Some(1500)), ("1ms", Some(1))] {
            assert_eq!(parse_max_staleness(v).ok(), want, "{v}");
        }
        for v in [
            "0s",
            "0ms",
            "1.5s",
            "3",
            "fast",
            "",
            "s",
            "ms",
            "-3s",
            "+3s",
            " 3s",
            "3 s",
            "99999999999999999999s",
        ] {
            let e = parse_max_staleness(v).unwrap_err();
            assert!(e.contains("EMFOR_MAX_STALENESS"), "{v}: {e}");
        }
    }
}
