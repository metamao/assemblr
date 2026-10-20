//! `assemblr registry`: registering, listing, removing, and synchronizing
//! local unit repositories.
//!
//! Data goes to stdout and diagnostics to stderr. `list --json` derives from
//! the same index as the human listing, in the schema documented in
//! `docs/domain/registries-and-discovery.md`.

use std::path::PathBuf;
use std::process::ExitCode;

use assemblr::adapters::{Added, Problem, Registry, RegistryFailure, TagProblem};
use assemblr::domain::{Excluded, IntegrityChange, RegistryError, RegistryIndex, UnitIdentity};
use clap::Subcommand;
use serde_json::{Value, json};

#[derive(Debug, Subcommand)]
pub(super) enum RegistryCommand {
    /// Register a local unit repository and index its published versions.
    Add {
        /// The path of the unit's Git repository.
        path: PathBuf,
    },
    /// List the registered units and their published versions.
    List {
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Remove a unit's registration. Its repository is not touched.
    Remove {
        /// The identity of the unit, such as `@acme/node`.
        #[arg(value_parser = UnitIdentity::parse)]
        identity: UnitIdentity,
    },
    /// Read every registered repository again and index newly published
    /// versions.
    Sync,
}

pub(super) fn run(command: RegistryCommand) -> ExitCode {
    let registry = match Registry::locate() {
        Ok(registry) => registry,
        Err(error) => return fail(&error.to_string(), None),
    };
    match command {
        RegistryCommand::Add { path } => add(&registry, &path),
        RegistryCommand::List { json } => list(&registry, json),
        RegistryCommand::Remove { identity } => remove(&registry, &identity),
        RegistryCommand::Sync => sync(&registry),
    }
}

fn add(registry: &Registry, path: &std::path::Path) -> ExitCode {
    match registry.add(path) {
        Ok(Added::Registered {
            identity,
            source,
            versions,
            excluded,
        }) => {
            warn_excluded(&excluded);
            if versions.is_empty() {
                println!("Registered {identity} from {source}, with no published versions yet.");
            } else {
                println!(
                    "Registered {identity} from {source}, with published versions {}.",
                    join(&versions)
                );
            }
            report_excluded(&excluded);
            ExitCode::SUCCESS
        }
        Ok(Added::AlreadyRegistered { identity, source }) => {
            println!(
                "{source} is already registered as {identity}; nothing changed. \
                 Run `assemblr registry sync` to index newly published versions."
            );
            ExitCode::SUCCESS
        }
        Err(failure) => rejected("registration", &failure),
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
            println!(
                "Removed {identity}, registered from {}. The repository was not modified.",
                removed.source()
            );
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
            "Synchronized {} unit(s); no new published versions.",
            synced.units()
        );
    } else {
        for (identity, versions) in synced.added() {
            println!("{identity}: indexed {}", join(versions));
        }
    }
    report_excluded(synced.excluded());
    ExitCode::SUCCESS
}

/// Reports a failed operation, with a hint where one helps.
fn rejected(operation: &str, failure: &RegistryFailure) -> ExitCode {
    let hint = match failure {
        RegistryFailure::NotRegistered(_) => {
            Some("run `assemblr registry list` to see the registered units".to_owned())
        }
        RegistryFailure::Rejected(problems) => Some(rejection_hint(operation, problems).map_or_else(
            || format!("the {operation} was rejected, and the registry index was not changed"),
            |hint| {
                format!(
                    "the {operation} was rejected, and the registry index was not changed; {hint}"
                )
            },
        )),
        RegistryFailure::Storage(_) | RegistryFailure::Reference(_) => None,
    };
    fail(&failure.to_string(), hint.as_deref())
}

fn rejection_hint(operation: &str, problems: &[Problem]) -> Option<String> {
    problems.iter().find_map(|problem| match problem {
        Problem::Invalid(RegistryError::SourceConflict {
            identity, given, ..
        }) => Some(format!(
            "to register {given} instead, run `assemblr registry remove {identity}` first"
        )),
        Problem::Invalid(RegistryError::Integrity { change, .. }) => Some(match change {
            IntegrityChange::TagRemoved | IntegrityChange::TagMoved { .. } => {
                "published versions never change: restore the tag where it was, \
                 or publish the change as a new version"
                    .to_owned()
            }
            IntegrityChange::BecameInvalid(_) => {
                "published versions never change: restore the tag where it was".to_owned()
            }
        }),
        Problem::Unreadable { .. } if operation == "synchronization" => Some(
            "if a registered repository was moved or deleted, \
             `assemblr registry remove <identity>` removes its registration"
                .to_owned(),
        ),
        _ => None,
    })
}

/// Explains on stderr why each excluded tag is invalid.
fn warn_excluded(excluded: &[Excluded<TagProblem>]) {
    for excluded in excluded {
        eprintln!(
            "warning: {}: excluded tag {}, which is not indexed as a published version: {}",
            excluded.identity(),
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
        .map(|excluded| format!("{} {}", excluded.identity(), excluded.tag()))
        .collect();
    println!(
        "Excluded {} invalid version tag(s), which are not indexed: {}.",
        excluded.len(),
        tags.join(", ")
    );
}

fn fail(message: &str, hint: Option<&str>) -> ExitCode {
    eprintln!("error: {}", message.replace('\n', "\nerror: "));
    if let Some(hint) = hint {
        eprintln!("hint: {hint}");
    }
    ExitCode::FAILURE
}

fn join<T: ToString>(items: &[T]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
