//! Embeds the version shown by `assemblr --version`: the package version and,
//! when it can be determined, the source commit. Release builds set
//! `ASSEMBLR_COMMIT`; other builds ask Git, and show only the version when
//! Git or the repository is missing (for example a source archive).

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=ASSEMBLR_COMMIT");
    // `logs/HEAD` changes on every commit, `HEAD` on every checkout.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/logs/HEAD");

    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let full = match commit() {
        Some(commit) => format!("{version} ({commit})"),
        None => version,
    };
    println!("cargo:rustc-env=ASSEMBLR_VERSION={full}");
}

fn commit() -> Option<String> {
    let given = std::env::var("ASSEMBLR_COMMIT")
        .ok()
        .filter(|c| !c.trim().is_empty());
    let commit = given.or_else(|| {
        let out = Command::new("git")
            .args(["rev-parse", "--short=12", "HEAD"])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    })?;
    let commit = commit.trim();
    let valid = !commit.is_empty() && commit.chars().all(|c| c.is_ascii_alphanumeric());
    valid.then(|| commit.chars().take(12).collect())
}
