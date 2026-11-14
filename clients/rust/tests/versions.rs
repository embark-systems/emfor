//! Every client, and the FreeBSD port example, carries the emfor release's
//! version: libemfor and the wrappers must come from the same release as
//! the daemon. A release bumps the workspace version in Cargo.toml and each
//! place below (RELEASING.md).

/// (file under the repository root, text that must contain the version).
const PLACES: [(&str, &str); 6] = [
    ("clients/python/pyproject.toml", "version = \"{}\""),
    ("clients/python/emfor/__init__.py", "__version__ = \"{}\""),
    ("clients/perl/lib/Emfor.pm", "our $VERSION = '{}';"),
    ("clients/php/emfor.c", "#define PHP_EMFOR_VERSION \"{}\""),
    ("examples/freebsd/Makefile", "DISTVERSION=\t{}\n"),
    (
        "clients/go/README.md",
        "go get github.com/embark-systems/emfor/clients/go@v{}",
    ),
];

#[test]
fn clients_carry_the_release_version() {
    let version = env!("CARGO_PKG_VERSION");
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let wrong: Vec<String> = PLACES
        .iter()
        .filter_map(|(file, pattern)| {
            let text = std::fs::read_to_string(format!("{root}/{file}")).unwrap();
            let want = pattern.replace("{}", version);
            (!text.contains(&want)).then(|| format!("{file}: no {want:?}"))
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "not at version {version}:\n{}",
        wrong.join("\n")
    );
}

/// The FreeBSD port's `Makefile.crates` lists every crates.io package in
/// `Cargo.lock` as `name-version`, in the lock file's order.
#[test]
fn freebsd_port_lists_the_locked_crates() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let lock = std::fs::read_to_string(format!("{root}/Cargo.lock")).unwrap();
    let mut want = Vec::new();
    for package in lock.split("[[package]]").skip(1) {
        let field = |key: &str| {
            package
                .lines()
                .find_map(|l| l.strip_prefix(key)?.strip_prefix(" = \""))
                .map(|v| v.trim_end_matches('"'))
        };
        if field("source").is_some_and(|s| s.starts_with("registry+")) {
            want.push(format!(
                "{}-{}",
                field("name").unwrap(),
                field("version").unwrap()
            ));
        }
    }
    let crates =
        std::fs::read_to_string(format!("{root}/examples/freebsd/Makefile.crates")).unwrap();
    let got: Vec<&str> = crates
        .trim_start_matches("CARGO_CRATES=")
        .split(|c: char| c.is_whitespace() || c == '\\')
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(
        got, want,
        "examples/freebsd/Makefile.crates does not match Cargo.lock; regenerate it (examples/freebsd/README.md)"
    );
}
