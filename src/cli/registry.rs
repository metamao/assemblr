//! `assemblr registry`: registering, listing, removing, and synchronizing
//! unit repositories, local or remote.
//!
//! Data goes to stdout and diagnostics to stderr. `list --json` derives from
//! the same index as the human listing, in the schema documented in
//! `docs/domain/registries-and-discovery.md`.

use std::path::PathBuf;
use std::process::ExitCode;

use assemblr::adapters::{
    Added, HeadManifestError, InvalidVersion, Problem, Registry, RegistryFailure, TagProblem,
};
use assemblr::domain::{Excluded, IntegrityChange, RegistryError, RegistryIndex, UnitIdentity};
use clap::Subcommand;
use serde_json::{Value, json};

use super::diagnostic::{Diagnosis, count, fail, fail_with};

#[derive(Debug, Subcommand)]
pub(super) enum RegistryCommand {
    /// Register a unit's Git repository
    ///
    /// Indexes the versions the repository has published, so that projects
    /// and other units can use them. A remote repository is fetched with
    /// your Git and SSH settings, and fetched again only by `registry sync`.
    #[command(after_long_help = "Examples:\n  \
        assemblr registry add ../web\n  \
        assemblr registry add https://github.com/acme/web.git\n  \
        assemblr registry add git@github.com:acme/web.git")]
    Add {
        /// The repository: a local path, or an `https://` or SSH URL
        #[arg(value_name = "REPOSITORY")]
        path: PathBuf,
    },
    /// List the registered units and their versions
    #[command(
        after_long_help = "Examples:\n  assemblr registry list\n  assemblr registry list --json"
    )]
    List {
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Remove a unit's registration
    ///
    /// No repository is changed. A unit of a catalog is removed with every
    /// other unit of the catalog, since they are registered together.
    #[command(after_long_help = "Examples:\n  assemblr registry remove @acme/web")]
    Remove {
        /// The unit, like `@acme/web`
        #[arg(value_name = "NAME", value_parser = UnitIdentity::parse)]
        identity: UnitIdentity,
    },
    /// Index newly published versions of every registered unit
    ///
    /// Fetches every remote repository first.
    Sync,
}

pub(super) fn run(command: RegistryCommand) -> ExitCode {
    match command {
        RegistryCommand::Add { path } => with_registry(|registry| add(registry, &path)),
        RegistryCommand::List { json } => with_registry(|registry| list(registry, json)),
        RegistryCommand::Remove { identity } => {
            with_registry(|registry| remove(registry, &identity))
        }
        RegistryCommand::Sync => with_registry(sync),
    }
}

/// Runs `action` on the user's registry.
fn with_registry(action: impl FnOnce(&Registry) -> ExitCode) -> ExitCode {
    match Registry::locate() {
        Ok(registry) => action(&registry),
        Err(error) => fail(&error.to_string(), None),
    }
}

fn add(registry: &Registry, path: &std::path::Path) -> ExitCode {
    match registry.add(path) {
        Ok(Added::Registered {
            units,
            source,
            excluded,
        }) => {
            warn_excluded(&excluded);
            match units.iter().collect::<Vec<_>>().as_slice() {
                [(identity, versions)] => println!(
                    "Registered {identity} from {source}, {}.",
                    with_versions(versions)
                ),
                units => {
                    println!(
                        "Registered the catalog {source}, with {}:",
                        count(units.len(), "unit", "units")
                    );
                    for (identity, versions) in units {
                        println!("  {identity}, {}", with_versions(versions));
                    }
                }
            }
            report_excluded(&excluded);
            ExitCode::SUCCESS
        }
        Ok(Added::AlreadyRegistered { identities, source }) => {
            println!(
                "{source} is already registered as {}; nothing changed.",
                join(&identities)
            );
            println!("Next: run `assemblr registry sync` to index newly published versions.");
            ExitCode::SUCCESS
        }
        Err(failure) => rejected("registration", &failure),
    }
}

/// How a newly registered unit's versions are reported.
fn with_versions(versions: &[assemblr::domain::Version]) -> String {
    match versions {
        [] => "with no published versions yet".to_owned(),
        [version] => format!("with version {version}"),
        versions => format!("with versions {}", join(versions)),
    }
}

fn list(registry: &Registry, json: bool) -> ExitCode {
    let index = match registry.index() {
        Ok(index) => index,
        Err(error) => return fail(&error.to_string(), None),
    };
    if json {
        match serde_json::to_string_pretty(&list_json(&index)) {
            Ok(text) => println!("{text}"),
            Err(error) => return fail(&error.to_string(), None),
        }
    } else if index.units().is_empty() {
        println!("No unit repositories are registered.");
    } else {
        for (identity, unit) in index.units() {
            let versions: Vec<_> = unit.versions().keys().collect();
            println!("{identity}");
            println!("  source: {}", unit.source());
            if versions.is_empty() {
                println!("  versions: none published");
            } else {
                println!("  versions: {}", join(&versions));
            }
        }
    }
    ExitCode::SUCCESS
}

/// The documented, stable JSON form of the index: units by identity, and
/// their versions in SemVer order.
fn list_json(index: &RegistryIndex) -> Value {
    let units: Vec<Value> = index
        .units()
        .iter()
        .map(|(identity, unit)| {
            let versions: Vec<Value> = unit
                .versions()
                .iter()
                .map(|(version, provenance)| {
                    json!({
                        "version": version.to_string(),
                        "commit": provenance.commit().as_str(),
                        "tree": provenance.tree().as_str(),
                    })
                })
                .collect();
            json!({
                "identity": identity.to_string(),
                "source": unit.source().as_str(),
                "versions": versions,
            })
        })
        .collect();
    json!({ "units": units })
}

fn remove(registry: &Registry, identity: &UnitIdentity) -> ExitCode {
    match registry.remove(identity) {
        Ok(removed) => {
            let source = removed.source();
            if removed.units().len() == 1 {
                println!(
                    "Removed {identity}, registered from {source}. The repository was not \
                     modified."
                );
            } else {
                let identities: Vec<&UnitIdentity> = removed.units().keys().collect();
                println!(
                    "Removed the units of the catalog {source}, which are registered together: \
                     {}. The repository was not modified.",
                    join(&identities)
                );
            }
            ExitCode::SUCCESS
        }
        Err(failure) => rejected("removal", &failure),
    }
}

fn sync(registry: &Registry) -> ExitCode {
    let synced = match registry.sync() {
        Ok(synced) => synced,
        Err(failure) => return rejected("synchronization", &failure),
    };
    warn_excluded(synced.excluded());
    if synced.units() == 0 {
        println!("No unit repositories are registered.");
    } else if synced.added().is_empty() {
        println!(
            "Synchronized {}; no new versions.",
            count(synced.units(), "unit", "units")
        );
    } else {
        for (identity, versions) in synced.added() {
            if versions.is_empty() {
                println!("{identity}: registered, with no published versions");
            } else {
                println!("{identity}: indexed {}", join(versions));
            }
        }
    }
    report_excluded(synced.excluded());
    ExitCode::SUCCESS
}

/// Reports a failed operation, with a hint where one helps.
fn rejected(operation: &str, failure: &RegistryFailure) -> ExitCode {
    let diagnosis = match failure {
        RegistryFailure::NotRegistered(_) => {
            Diagnosis::hint("`assemblr registry list` shows the registered units")
        }
        RegistryFailure::Rejected(problems) => Diagnosis::note("the registry was not changed")
            .advise_if(rejection_hint(operation, problems)),
        RegistryFailure::Storage(_) | RegistryFailure::Reference(_) => Diagnosis::default(),
    };
    fail_with(&failure.to_string(), &diagnosis)
}

fn rejection_hint(operation: &str, problems: &[Problem]) -> Option<String> {
    problems.iter().find_map(|problem| match problem {
        Problem::Invalid(RegistryError::SourceConflict {
            identity, given, ..
        }) => Some(format!(
            "to register {given} instead, run `assemblr registry remove {identity}` first"
        )),
        Problem::Invalid(RegistryError::Integrity { change, .. }) => Some(match change {
            IntegrityChange::TagRemoved
            | IntegrityChange::TagMoved { .. }
            | IntegrityChange::UnitMissing => {
                "published versions never change: restore the tag where it was, \
                 or publish the change as a new version"
                    .to_owned()
            }
            IntegrityChange::BecameInvalid(_) => {
                "published versions never change: restore the tag where it was".to_owned()
            }
        }),
        Problem::NoIdentity {
            error: HeadManifestError::Invalid(InvalidVersion::MissingManifest),
            ..
        } => Some(
            "a unit's repository has `manifest.jsonc` at its root; check that the reference \
             names one"
                .to_owned(),
        ),
        Problem::Unfetchable { source, .. } => Some(format!(
            "check the address, and that `git ls-remote {source}` works with your Git \
             credentials"
        )),
        Problem::Unreadable { .. } if operation == "synchronization" => Some(
            "if a registered repository was moved or deleted, \
             `assemblr registry remove <identity>` removes its registration"
                .to_owned(),
        ),
        _ => None,
    })
}

/// Explains on stderr why each excluded tag is invalid.
pub(super) fn warn_excluded(excluded: &[Excluded<TagProblem>]) {
    for excluded in excluded {
        eprintln!(
            "warning: {}: excluded tag {}, which is not indexed as a published version: {}",
            excluded.subject(),
            excluded.tag(),
            excluded.reason()
        );
    }
}

/// Lists the excluded tags in the result, apart from the indexed versions.
fn report_excluded(excluded: &[Excluded<TagProblem>]) {
    if excluded.is_empty() {
        return;
    }
    let tags: Vec<String> = excluded
        .iter()
        .map(|excluded| format!("{} {}", excluded.subject(), excluded.tag()))
        .collect();
    println!(
        "Excluded {}, which {} not indexed: {}.",
        count(
            excluded.len(),
            "invalid version tag",
            "invalid version tags"
        ),
        if excluded.len() == 1 { "is" } else { "are" },
        tags.join(", ")
    );
}

fn join<T: ToString>(items: &[T]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
