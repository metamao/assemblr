//! The sample units, projects, and catalog that scenarios start from.
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

/// Gives each of `units`, already published, a remote: a bare clone in
/// `remotes/<unit>.git`, set as its `origin` by a `file://` URL, as a unit
/// pushed to a Git host has.
pub(crate) fn remotes(machine: &Machine<'_>, units: &[&str]) -> Result<(), String> {
    let root = &machine.root.path;
    for name in units {
        let dir = root.join("units").join(name);
        let remote = root.join("remotes").join(format!("{name}.git"));
        machine.git(root, &["clone", "-q", "--bare", arg(&dir)?, arg(&remote)?])?;
        machine.git(
            &dir,
            &[
                "remote",
                "add",
                "origin",
                &format!("file://{}", arg(&remote)?),
            ],
        )?;
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

/// A committed project in `dir` that selects `units`, each with its options
/// as `name=value`.
pub(crate) fn project(
    machine: &Machine<'_>,
    dir: &Path,
    units: &[(&str, &[&str])],
    message: &str,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    machine.git(dir, &["init", "-q"])?;
    machine.assemblr(dir, &["init"])?;
    for (unit, options) in units {
        let mut args = vec!["add", unit];
        for option in *options {
            args.extend(["--option", option]);
        }
        machine.assemblr(dir, &args)?;
    }
    commit(machine, dir, message)
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

/// The catalog `@acme/catalog` in `catalog/`, listing every unit by a
/// `file://` URL, with its index refreshed and committed, and the README.md
/// and AGENTS.md that `catalog init` writes committed as they are.
pub(crate) fn catalog(machine: &Machine<'_>) -> Result<PathBuf, String> {
    let root = &machine.root.path;
    machine.assemblr(root, &["catalog", "init", "@acme/catalog", "catalog"])?;
    let dir = root.join("catalog");
    machine.git(&dir, &["add", "README.md", "AGENTS.md"])?;
    machine.git(&dir, &["commit", "-qm", "Describe the catalog"])?;
    for name in UNITS {
        let source = root.join("units").join(name);
        machine.assemblr(
            &dir,
            &["catalog", "add", &format!("file://{}", arg(&source)?)],
        )?;
    }
    Ok(dir)
}
