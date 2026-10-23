//! `assemblr registry`: registering, listing, removing, and synchronizing
//! unit repositories and catalogs, local or remote.
//!
//! Data goes to stdout and diagnostics to stderr. `list --json` derives from
//! the same index as the human listing, in the schema documented in
//! `docs/domain/registries-and-discovery.md`.

use std::path::PathBuf;
use std::process::ExitCode;

use assemblr::adapters::{Added, Problem, Registry, RegistryFailure, TagProblem};
use assemblr::domain::{
    CatalogIdentity, Excluded, IntegrityChange, RegistryError, RegistryIndex, UnitIdentity,
};
use clap::Subcommand;
use serde_json::{Value, json};

use super::diagnostic::{Diagnosis, count, fail, fail_with};

#[derive(Debug, Subcommand)]
pub(super) enum RegistryCommand {
    /// Register a unit's or a catalog's Git repository
    ///
    /// Indexes the versions the repository has published, so that projects
    /// and other units can use them. A catalog registers every unit it
    /// declares, with the versions its committed index lists. A remote
    /// repository is fetched with your Git and SSH settings, and fetched
    /// again only by `registry sync`.
    #[command(after_long_help = "Examples:\n  \
        assemblr registry add ../web\n  \
        assemblr registry add https://github.com/acme/web.git\n  \
        assemblr registry add git@github.com:acme/catalog.git")]
    Add {
        /// The repository: a local path, or an `https://` or SSH URL
        #[arg(value_name = "REPOSITORY")]
        path: PathBuf,
    },
    /// List the registered units, their versions, and catalogs
    #[command(
        after_long_help = "Examples:\n  assemblr registry list\n  assemblr registry list --json"
    )]
    List {
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Remove a unit's or a catalog's registration
    ///
    /// No repository is changed. A unit a registered catalog declares stays
    /// registered through it. Removing a catalog also removes the units only
    /// it declares.
    #[command(after_long_help = "Examples:\n  \
        assemblr registry remove @acme/web\n  \
        assemblr registry remove --catalog @acme/catalog")]
    Remove {
        /// The unit, like `@acme/web`, or the catalog with `--catalog`
        #[arg(value_name = "NAME", value_parser = UnitIdentity::parse)]
        identity: UnitIdentity,
        /// Remove the catalog of this name instead of a unit
        #[arg(long)]
        catalog: bool,
    },
    /// Index newly published versions of every unit and catalog
    ///
    /// Fetches every remote repository first. Each catalog is read as
    /// committed: units it adds are registered, units it drops leave the
    /// registry, and the versions its index lists are indexed.
    Sync,
}

pub(super) fn run(command: RegistryCommand) -> ExitCode {
    match command {
        RegistryCommand::Add { path } => with_registry(|registry| add(registry, &path)),
        RegistryCommand::List { json } => with_registry(|registry| list(registry, json)),
        RegistryCommand::Remove { identity, catalog } => with_registry(|registry| {
            if catalog {
                remove_catalog(registry, &CatalogIdentity::from(identity))
            } else {
                remove(registry, &identity)
            }
        }),
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
                    "Registered {identity} from {source}, with {} {}.",
                    if versions.len() == 1 {
                        "version"
                    } else {
                        "versions"
                    },
                    join(&versions)
                );
            }
            report_excluded(&excluded);
            ExitCode::SUCCESS
        }
        Ok(Added::AlreadyRegistered { identity, source }) => {
            println!("{source} is already registered as {identity}; nothing changed.");
            println!("Next: run `assemblr registry sync` to index newly published versions.");
            ExitCode::SUCCESS
        }
        Ok(Added::CatalogRegistered {
            catalog,
            source,
            units,
            excluded,
        }) => {
            warn_excluded(&excluded);
            println!(
                "Registered the catalog {catalog} from {source}, with {}.",
                count(units.len(), "unit", "units")
            );
            for (identity, versions) in &units {
                if versions.is_empty() {
                    println!("  {identity}: no published versions");
                } else {
                    println!("  {identity}: {}", join(versions));
                }
            }
            report_excluded(&excluded);
            ExitCode::SUCCESS
        }
        Ok(Added::CatalogAlreadyRegistered { catalog, source }) => {
            println!("{source} is already registered as the catalog {catalog}; nothing changed.");
            println!("Next: run `assemblr registry sync` to read the catalog again.");
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
    } else if index.units().is_empty() && index.catalogs().is_empty() {
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
            let catalogs = index.listing(identity);
            if !catalogs.is_empty() {
                println!("  catalogs: {}", join(&catalogs));
            }
        }
        for (identity, catalog) in index.catalogs() {
            let units: Vec<_> = catalog.units().keys().collect();
            println!("catalog {identity}");
            println!("  source: {}", catalog.source());
            println!("  commit: {}", catalog.commit());
            if units.is_empty() {
                println!("  units: none");
            } else {
                println!("  units: {}", join(&units));
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
            let catalogs: Vec<String> = index
                .listing(identity)
                .iter()
                .map(ToString::to_string)
                .collect();
            json!({
                "identity": identity.to_string(),
                "source": unit.source().as_str(),
                "direct": unit.is_direct(),
                "catalogs": catalogs,
                "versions": versions,
            })
        })
        .collect();
    let catalogs: Vec<Value> = index
        .catalogs()
        .iter()
        .map(|(identity, catalog)| {
            let units: Vec<String> = catalog.units().keys().map(ToString::to_string).collect();
            json!({
                "identity": identity.to_string(),
                "source": catalog.source().as_str(),
                "commit": catalog.commit().as_str(),
                "units": units,
            })
        })
        .collect();
    json!({ "units": units, "catalogs": catalogs })
}

fn remove(registry: &Registry, identity: &UnitIdentity) -> ExitCode {
    match registry.remove(identity) {
        Ok(removed) if removed.kept_by().is_empty() => {
            println!(
                "Removed {identity}, registered from {}. The repository was not modified.",
                removed.unit().source()
            );
            ExitCode::SUCCESS
        }
        Ok(removed) => {
            println!(
                "Removed the direct registration of {identity}; it stays registered through {}. \
                 The repository was not modified.",
                join(removed.kept_by())
            );
            ExitCode::SUCCESS
        }
        Err(RegistryFailure::NotRegistered(_))
            if registry.index().is_ok_and(|index| {
                index
                    .catalogs()
                    .contains_key(&CatalogIdentity::from(identity.clone()))
            }) =>
        {
            fail(
                &format!("{identity} is not a registered unit, but a registered catalog"),
                Some(&format!(
                    "run `assemblr registry remove --catalog {identity}` to remove the catalog"
                )),
            )
        }
        Err(failure) => rejected("removal", &failure),
    }
}

fn remove_catalog(registry: &Registry, identity: &CatalogIdentity) -> ExitCode {
    match registry.remove_catalog(identity) {
        Ok(removed) => {
            let source = removed.catalog().source();
            if removed.removed().is_empty() {
                println!("Removed the catalog {identity}, registered from {source}.");
            } else {
                println!(
                    "Removed the catalog {identity}, registered from {source}, and {} only it \
                     declared: {}.",
                    count(removed.removed().len(), "unit", "units"),
                    join(removed.removed())
                );
            }
            println!("No repository was modified.");
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
    let synchronized = if synced.catalogs() == 0 {
        count(synced.units(), "unit", "units")
    } else {
        format!(
            "{} and {}",
            count(synced.units(), "unit", "units"),
            count(synced.catalogs(), "catalog", "catalogs")
        )
    };
    if synced.units() == 0 && synced.catalogs() == 0 && synced.removed().is_empty() {
        println!("No unit repositories are registered.");
    } else if synced.added().is_empty() && synced.removed().is_empty() {
        println!("Synchronized {synchronized}; no new versions.");
    } else {
        for (identity, versions) in synced.added() {
            if versions.is_empty() {
                println!("{identity}: registered, with no published versions");
            } else {
                println!("{identity}: indexed {}", join(versions));
            }
        }
        for identity in synced.removed() {
            println!("{identity}: removed, since no registered catalog declares it any more");
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
        RegistryFailure::CatalogNotRegistered(_) => {
            Diagnosis::hint("`assemblr registry list` shows the registered catalogs")
        }
        RegistryFailure::Managed { catalogs, .. } => {
            Diagnosis::note("the registry was not changed").advise(format!(
                "remove it from the catalog's registry.jsonc, or remove the catalog with \
                 `assemblr registry remove --catalog {}`",
                catalogs
                    .first()
                    .map_or_else(|| "<catalog>".to_owned(), ToString::to_string)
            ))
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
            IntegrityChange::TagRemoved | IntegrityChange::TagMoved { .. } => {
                "published versions never change: restore the tag where it was, \
                 or publish the change as a new version"
                    .to_owned()
            }
            IntegrityChange::BecameInvalid(_) => {
                "published versions never change: restore the tag where it was".to_owned()
            }
        }),
        Problem::Invalid(RegistryError::CatalogConflict { catalog, given, .. }) => Some(format!(
            "to register {given} instead, run `assemblr registry remove --catalog {catalog}` first"
        )),
        Problem::Catalog { problem, .. } if problem.needs_refresh() => Some(
            "the catalog's maintainer runs `assemblr catalog refresh` in it, then commits and \
             pushes registry-index.json"
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
