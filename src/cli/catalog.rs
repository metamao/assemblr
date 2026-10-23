//! Catalog maintenance, for the catalog's maintainer: `assemblr registry
//! create` and `assemblr catalog refresh`. Neither command commits: the
//! maintainer reviews and commits `registry.jsonc` and `registry-index.json`
//! with Git.

use std::path::Path;
use std::process::ExitCode;

use assemblr::adapters::{
    CATALOG_INDEX, CATALOG_MANIFEST, CatalogInitFailure, RefreshFailure, Registry, init_catalog,
    refresh_catalog,
};
use assemblr::domain::CatalogIdentity;
use clap::Subcommand;

use super::diagnostic::{Diagnosis, count, fail, fail_with};
use super::registry::warn_excluded;

#[derive(Debug, Subcommand)]
pub(super) enum CatalogCommand {
    /// Regenerate the catalog's index from its units' tags
    ///
    /// Run it in the catalog's directory after changing `registry.jsonc` or
    /// after a unit publishes a version, then commit and push
    /// `registry-index.json`. Remote units are fetched first.
    Refresh,
}

/// A catalog identity, with an error that names a catalog, not a unit.
pub(super) fn parse_catalog(input: &str) -> Result<CatalogIdentity, String> {
    CatalogIdentity::parse(input)
        .map_err(|error| format!("invalid catalog {input:?}: {}", error.reason()))
}

pub(super) fn run(command: CatalogCommand) -> ExitCode {
    match command {
        CatalogCommand::Refresh => refresh(),
    }
}

/// `registry create`, which the contract lists among the registry commands.
pub(super) fn create(identity: &CatalogIdentity, directory: Option<&Path>) -> ExitCode {
    match init_catalog(directory, identity) {
        Ok(directory) => {
            println!("Created the catalog {identity} in {}.", directory.display());
            println!(
                "Next: `cd {}`, list units in {CATALOG_MANIFEST}, and run `assemblr catalog \
                 refresh`.",
                directory.display()
            );
            ExitCode::SUCCESS
        }
        Err(failure) => {
            let diagnosis = match &failure {
                CatalogInitFailure::NotEmpty(_) => Diagnosis::note("nothing was changed")
                    .advise("give a new or empty directory as the second argument"),
                CatalogInitFailure::Git(_) => {
                    Diagnosis::hint("Assemblr needs Git installed and on the PATH")
                }
                CatalogInitFailure::Io { .. } => Diagnosis::default(),
            };
            fail_with(&failure.to_string(), &diagnosis)
        }
    }
}

fn refresh() -> ExitCode {
    let registry = match Registry::locate() {
        Ok(registry) => registry,
        Err(error) => return fail(&error.to_string(), None),
    };
    match refresh_catalog(Path::new("."), &registry) {
        Ok(refreshed) => {
            warn_excluded(refreshed.excluded());
            let versions: usize = refreshed.versions().values().map(Vec::len).sum();
            let summary = format!(
                "{} and {}",
                count(refreshed.versions().len(), "unit", "units"),
                count(versions, "version", "versions")
            );
            if refreshed.changed() {
                println!("Refreshed the index of {}: {summary}.", refreshed.catalog());
                for (unit, versions) in refreshed.versions() {
                    let list: Vec<String> = versions.iter().map(ToString::to_string).collect();
                    if list.is_empty() {
                        println!("  {unit}: no published version");
                    } else {
                        println!("  {unit}: {}", list.join(", "));
                    }
                }
                println!("Next: commit and push {CATALOG_INDEX}.");
            } else {
                println!(
                    "The index of {} is up to date: {summary}.",
                    refreshed.catalog()
                );
            }
            ExitCode::SUCCESS
        }
        Err(failure) => {
            let diagnosis = match &failure {
                RefreshFailure::Read { .. } => {
                    Diagnosis::note("nothing was changed").advise(format!(
                        "run the command in the catalog's directory, next to {CATALOG_MANIFEST}"
                    ))
                }
                RefreshFailure::Manifest { .. } | RefreshFailure::Rejected(_) => {
                    Diagnosis::note(format!("{CATALOG_INDEX} was not changed"))
                }
                RefreshFailure::Write { .. } => Diagnosis::default(),
            };
            fail_with(&failure.to_string(), &diagnosis)
        }
    }
}
