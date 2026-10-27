//! The sample units and projects that scenarios start from.
//!
//! The units' files are in `fixture/units/`, later versions in
//! `fixture/releases/<unit>/<version>/`, and other repositories in
//! `fixture/repos/`. They are small but real: they build, publish, compose,
//! and update with the binary under test.

use std::fs;
use std::path::{Path, PathBuf};

use crate::machine::{Machine, arg, copy_tree};

/// The units, in an order where each comes after its dependencies.
const UNITS: [&str; 9] = [
    "base",
    "typescript",
    "cloudflare-worker",
    "node-api",
    "eslint",
    "auth-jwt",
    "docker-bun",
    "angular-ssr",
    "cloudflare-pages",
];

fn data(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/agent_bench/fixture")
        .join(path)
}

/// Authors, publishes, and registers every unit at 1.0.0 in `units/`.
pub(crate) fn units(machine: &Machine<'_>) -> Result<(), String> {
    let units = machine.root.path.join("units");
    for name in UNITS {
        machine.assemblr(&units, &["unit", "init", &format!("@acme/{name}")])?;
        let dir = units.join(name);
        copy_tree(&data(&format!("units/{name}")), &dir)?;
        publish(machine, &dir, "Release 1.0.0")?;
        machine.assemblr(&units, &["registry", "add", arg(&dir)?])?;
    }
    Ok(())
}

/// Publishes the next version of `unit` from `fixture/releases/`.
pub(crate) fn release(machine: &Machine<'_>, unit: &str, version: &str) -> Result<(), String> {
    let dir = machine.root.path.join("units").join(unit);
    copy_tree(&data(&format!("releases/{unit}/{version}")), &dir)?;
    publish(machine, &dir, &format!("Release {version}"))
}

fn publish(machine: &Machine<'_>, dir: &Path, message: &str) -> Result<(), String> {
    machine.assemblr(dir, &["unit", "build"])?;
    machine.git(dir, &["add", "-A"])?;
    machine.git(dir, &["commit", "-qm", message])?;
    machine.assemblr(dir, &["unit", "publish"])?;
    Ok(())
}

/// A committed project in `dir` that selects `units`, with each
/// `(placeholder, value)` of `values` replaced in the composed files, as
/// its consumer would.
pub(crate) fn project(
    machine: &Machine<'_>,
    dir: &Path,
    units: &[&str],
    values: &[(&str, &str)],
    message: &str,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    machine.git(dir, &["init", "-q"])?;
    machine.assemblr(dir, &["init"])?;
    for unit in units {
        machine.assemblr(dir, &["add", unit])?;
    }
    for (placeholder, value) in values {
        replace(dir, placeholder, value)?;
    }
    commit(machine, dir, message)
}

/// Replaces `old` with `new` in every file under `dir`, outside Git's and
/// Assemblr's own state.
pub(crate) fn replace(dir: &Path, old: &str, new: &str) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|error| format!("cannot read {}: {error}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if !path.ends_with(".git") && !path.ends_with(".assemblr") {
                replace(&path, old, new)?;
            }
        } else if let Ok(text) = fs::read_to_string(&path) {
            if text.contains(old) {
                fs::write(&path, text.replace(old, new))
                    .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
            }
        }
    }
    Ok(())
}

pub(crate) fn commit(machine: &Machine<'_>, dir: &Path, message: &str) -> Result<(), String> {
    machine.git(dir, &["add", "-A"])?;
    machine.git(dir, &["commit", "-qm", message])?;
    Ok(())
}

/// A committed repository in `dir` with the files of `fixture/repos/<name>`.
pub(crate) fn repository(machine: &Machine<'_>, name: &str, dir: &Path) -> Result<(), String> {
    copy_tree(&data(&format!("repos/{name}")), dir)?;
    machine.git(dir, &["init", "-q"])?;
    commit(machine, dir, "Initial commit")
}
