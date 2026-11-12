//! Every client carries the emfor release's version: libemfor and the
//! wrappers must come from the same release as the daemon. A release bumps
//! the workspace version in Cargo.toml and each place below (RELEASING.md).

/// (file under the repository root, text that must contain the version).
const PLACES: [(&str, &str); 4] = [
    ("clients/python/pyproject.toml", "version = \"{}\""),
    ("clients/perl/lib/Emfor.pm", "our $VERSION = '{}';"),
    ("clients/php/emfor.c", "#define PHP_EMFOR_VERSION \"{}\""),
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
