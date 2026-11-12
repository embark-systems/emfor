// The default config path for an install prefix. Shared by build.rs
// (`include!`) and the unit tests in src/config.rs (`#[path]`), so it
// must stay free of crate imports.

/// The default config path for `EMFOR_PREFIX` (`None` when unset): unset,
/// empty or `/usr` gives `/etc/emfor/emfor.toml`, any other absolute path
/// `<prefix>/etc/emfor/emfor.toml`. A relative prefix is an error.
pub fn default_config_path(prefix: Option<&str>) -> Result<String, String> {
    let prefix = match prefix {
        None | Some("") => return Ok("/etc/emfor/emfor.toml".to_owned()),
        Some(p) => p,
    };
    if !prefix.starts_with('/') {
        return Err(format!(
            "EMFOR_PREFIX must be an absolute path, got {prefix:?}"
        ));
    }
    let prefix = prefix.strip_suffix('/').unwrap_or(prefix);
    match prefix {
        "" | "/usr" => Ok("/etc/emfor/emfor.toml".to_owned()),
        p => Ok(format!("{p}/etc/emfor/emfor.toml")),
    }
}
