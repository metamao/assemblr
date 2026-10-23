//! Catalog maintenance, for the catalog's maintainer: `assemblr catalog
//! init`, `catalog add`, `catalog remove`, and `catalog refresh`. The last three commit `registry.jsonc` and
//! `registry-index.json`, and nothing else, unless `--no-commit` is given;
//! none of them pushes (see `docs/adr/0033-catalog-entry-commands.md`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use assemblr::adapters::{
    CATALOG_INDEX, CATALOG_MANIFEST, CatalogAdded, CatalogChangeFailure, CatalogInitFailure,
    GitError, GitRepository, RefreshFailure, Refreshed, Registry, add_to_catalog, init_catalog,
    refresh_catalog, remove_from_catalog,
};
use assemblr::domain::{CatalogIdentity, UnitIdentity};
use clap::Subcommand;

use super::diagnostic::{Diagnosis, count, fail, fail_with};
use super::registry::warn_excluded;

#[derive(Debug, Subcommand)]
pub(super) enum CatalogCommand {
    /// Create a catalog of unit repositories
    ///
    /// Creates a Git repository with a `registry.jsonc` that lists no units
    /// yet. Add units with `assemblr catalog add`.
    #[command(after_long_help = "Examples:\n  \
        assemblr catalog init @acme/web\n  \
        assemblr catalog init @acme/web catalogs/web")]
    Init {
        /// The catalog, like `@acme/web`
        #[arg(value_name = "CATALOG", value_parser = parse_catalog)]
        identity: CatalogIdentity,
        /// The directory to create it in [default: the catalog's name]
        directory: Option<PathBuf>,
    },
    /// Add a unit repository to the catalog
    ///
    /// Run it in the catalog's directory. The unit's name is read from its
    /// repository, the index is refreshed, and both files are committed.
    /// Remote repositories are fetched first.
    #[command(after_long_help = "Examples:\n  \
        assemblr catalog add https://github.com/acme/web.git\n  \
        assemblr catalog add git@github.com:acme/web.git")]
    Add {
        /// The unit's repository: an `https://`, `ssh://`, or `file://` URL, or `git@host:path`
        #[arg(value_name = "REPOSITORY")]
        reference: String,
        /// Write the files without committing them
        #[arg(long)]
        no_commit: bool,
    },
    /// Remove a unit from the catalog
    ///
    /// Run it in the catalog's directory. The index is refreshed and both
    /// files are committed. The unit's repository is not changed.
    #[command(after_long_help = "Example:\n  assemblr catalog remove @acme/web")]
    Remove {
        /// The unit, like `@acme/web`
        #[arg(value_name = "UNIT", value_parser = UnitIdentity::parse)]
        unit: UnitIdentity,
        /// Write the files without committing them
        #[arg(long)]
        no_commit: bool,
    },
    /// Regenerate the catalog's index from its units' tags
    ///
    /// Run it in the catalog's directory after a unit publishes a version,
    /// or after editing `registry.jsonc`, then push. Remote units are
    /// fetched first.
    Refresh {
        /// Write the index without committing it
        #[arg(long)]
        no_commit: bool,
    },
}

/// A catalog identity, with an error that names a catalog, not a unit.
fn parse_catalog(input: &str) -> Result<CatalogIdentity, String> {
    CatalogIdentity::parse(input)
        .map_err(|error| format!("invalid catalog {input:?}: {}", error.reason()))
}

pub(super) fn run(command: CatalogCommand) -> ExitCode {
    let root = Path::new(".");
    match command {
        CatalogCommand::Init {
            identity,
            directory,
        } => init(&identity, directory.as_deref()),
        CatalogCommand::Add {
            reference,
            no_commit,
        } => with_registry(|registry| add(root, registry, &reference, !no_commit)),
        CatalogCommand::Remove { unit, no_commit } => {
            with_registry(|registry| remove(root, registry, &unit, !no_commit))
        }
        CatalogCommand::Refresh { no_commit } => {
            with_registry(|registry| refresh(root, registry, !no_commit))
        }
    }
}

/// Runs `action` on the user's registry, whose copies of remote units the
/// catalog commands fetch into.
fn with_registry(action: impl FnOnce(&Registry) -> ExitCode) -> ExitCode {
    match Registry::locate() {
        Ok(registry) => action(&registry),
        Err(error) => fail(&error.to_string(), None),
    }
}

fn init(identity: &CatalogIdentity, directory: Option<&Path>) -> ExitCode {
    match init_catalog(directory, identity) {
        Ok(directory) => {
            println!("Created the catalog {identity} in {}.", directory.display());
            println!(
                "Next: `cd {}`, then run `assemblr catalog add <REPOSITORY>` for each unit.",
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

fn add(root: &Path, registry: &Registry, reference: &str, commit: bool) -> ExitCode {
    match add_to_catalog(root, reference, registry) {
        Ok(CatalogAdded::Added { unit, refreshed }) => {
            warn_excluded(refreshed.excluded());
            let versions = refreshed
                .versions()
                .get(&unit)
                .map_or(&[][..], Vec::as_slice);
            println!(
                "Added {unit} to the catalog {}, with {}.",
                refreshed.catalog(),
                version_list(versions)
            );
            finish(root, commit, &format!("Add {unit} to the catalog"))
        }
        Ok(CatalogAdded::AlreadyDeclared { unit }) => {
            println!("The catalog declares {unit} from {reference} already; nothing changed.");
            ExitCode::SUCCESS
        }
        Err(failure) => change_failed(&failure),
    }
}

fn remove(root: &Path, registry: &Registry, unit: &UnitIdentity, commit: bool) -> ExitCode {
    match remove_from_catalog(root, unit, registry) {
        Ok((declared, refreshed)) => {
            warn_excluded(refreshed.excluded());
            println!(
                "Removed {unit}, declared from {declared}, from the catalog {}.",
                refreshed.catalog()
            );
            println!("Its repository was not changed.");
            finish(root, commit, &format!("Remove {unit} from the catalog"))
        }
        Err(failure) => change_failed(&failure),
    }
}

fn refresh(root: &Path, registry: &Registry, commit: bool) -> ExitCode {
    match refresh_catalog(root, registry) {
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
                list_versions(&refreshed);
            } else {
                println!(
                    "The index of {} is up to date: {summary}.",
                    refreshed.catalog()
                );
            }
            finish(root, commit, "Refresh the catalog index")
        }
        Err(failure) => refresh_failed(&failure),
    }
}

fn list_versions(refreshed: &Refreshed) {
    for (unit, versions) in refreshed.versions() {
        if versions.is_empty() {
            println!("  {unit}: no published version");
        } else {
            let list: Vec<String> = versions.iter().map(ToString::to_string).collect();
            println!("  {unit}: {}", list.join(", "));
        }
    }
}

/// "no published version", or the versions, in order.
fn version_list(versions: &[assemblr::domain::Version]) -> String {
    if versions.is_empty() {
        return "no published version".to_owned();
    }
    let list: Vec<String> = versions.iter().map(ToString::to_string).collect();
    format!(
        "{} {}",
        if list.len() == 1 {
            "version"
        } else {
            "versions"
        },
        list.join(", ")
    )
}

/// Commits the catalog's two files when `commit`, and says what to do next.
fn finish(root: &Path, commit: bool, message: &str) -> ExitCode {
    if !commit {
        println!("Next: commit and push {CATALOG_MANIFEST} and {CATALOG_INDEX}.");
        return ExitCode::SUCCESS;
    }
    match commit_catalog(root, message) {
        Ok(true) => {
            println!("Committed {CATALOG_MANIFEST} and {CATALOG_INDEX}.");
            println!("Next: run `git push` so that other machines can sync it.");
            ExitCode::SUCCESS
        }
        Ok(false) => ExitCode::SUCCESS,
        Err(error) => fail_with(
            &format!("could not commit the catalog: {error}"),
            &Diagnosis::note(format!(
                "{CATALOG_MANIFEST} and {CATALOG_INDEX} were written but not committed"
            ))
            .advise("fix the problem, then commit them with Git"),
        ),
    }
}

fn commit_catalog(root: &Path, message: &str) -> Result<bool, GitError> {
    GitRepository::open(root)?.commit_paths(root, &[CATALOG_MANIFEST, CATALOG_INDEX], message)
}

fn change_failed(failure: &CatalogChangeFailure) -> ExitCode {
    let unchanged = || Diagnosis::note("nothing was changed");
    let diagnosis = match failure {
        CatalogChangeFailure::Refresh(failure) => return refresh_failed(failure),
        CatalogChangeFailure::Reference(_) | CatalogChangeFailure::Unreadable { .. } => unchanged(),
        CatalogChangeFailure::DeclaredElsewhere { unit, .. } => unchanged().advise(format!(
            "run `assemblr catalog remove {unit}` first to declare it from another repository"
        )),
        CatalogChangeFailure::NotDeclared { .. } => {
            unchanged().advise(format!("{CATALOG_MANIFEST} lists the units it declares"))
        }
        CatalogChangeFailure::Edit(_) => unchanged().advise(format!(
            "edit {CATALOG_MANIFEST} by hand, then run `assemblr catalog refresh`"
        )),
        CatalogChangeFailure::Declaration { index_written, .. } => {
            if *index_written {
                Diagnosis::note(format!(
                    "{CATALOG_INDEX} was replaced, but {CATALOG_MANIFEST} was not changed"
                ))
                .advise("run `assemblr catalog refresh` to match the index to it again")
            } else {
                unchanged()
            }
        }
    };
    fail_with(&failure.to_string(), &diagnosis)
}

fn refresh_failed(failure: &RefreshFailure) -> ExitCode {
    let diagnosis = match failure {
        RefreshFailure::Read { .. } => Diagnosis::note("nothing was changed").advise(format!(
            "run the command in the catalog's directory, next to {CATALOG_MANIFEST}"
        )),
        RefreshFailure::Manifest { .. } | RefreshFailure::Rejected(_) => {
            Diagnosis::note("nothing was changed")
        }
        RefreshFailure::Write { .. } => Diagnosis::default(),
    };
    fail_with(&failure.to_string(), &diagnosis)
}
