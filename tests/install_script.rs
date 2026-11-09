//! The install scripts install, by default, the release they belong to
//! (ADR 0043).

fn sets(script: &str, expected: &str) -> bool {
    script.lines().any(|line| line.trim() == expected)
}

#[test]
fn install_sh_defaults_to_the_package_version() {
    let expected = format!("default_version={}", env!("CARGO_PKG_VERSION"));
    assert!(
        sets(include_str!("../install.sh"), &expected),
        "install.sh must set `{expected}`, the version in Cargo.toml"
    );
}

#[test]
fn install_ps1_defaults_to_the_package_version() {
    let expected = format!("$defaultVersion = \"{}\"", env!("CARGO_PKG_VERSION"));
    assert!(
        sets(include_str!("../install.ps1"), &expected),
        "install.ps1 must set `{expected}`, the version in Cargo.toml"
    );
}
