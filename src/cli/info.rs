//! `assemblr info`: what the local registry knows about one unit, for
//! people choosing it and for agents composing with it.
//!
//! Data goes to stdout and diagnostics to stderr. `--json` derives from the
//! same [`UnitInfo`] as the text, in the schema documented in
//! `docs/domain/registries-and-discovery.md`.

use std::process::ExitCode;

use assemblr::adapters::{InfoFailure, Registry, UnitInfo};
use assemblr::domain::{UnitIdentity, Version};
use clap::Args;
use serde_json::{Map, Value, json};

use super::diagnostic::fail;
use super::project::unit_argument;

#[derive(Debug, Args)]
pub(super) struct InfoArgs {
    /// The unit, like `@acme/web` or `@acme/web@1.2.0` [default version:
    /// the newest stable one]
    #[arg(value_name = "UNIT", value_parser = unit_version)]
    unit: (UnitIdentity, Option<Version>),
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

/// Parses `@owner/name`, optionally followed by `@` and an exact version.
fn unit_version(argument: &str) -> Result<(UnitIdentity, Option<Version>), String> {
    let parsed = unit_argument(argument)?;
    let version = parsed
        .version()
        .map(|version| Version::parse(version).map_err(|error| error.to_string()))
        .transpose()?;
    Ok((parsed.identity().clone(), version))
}

pub(super) fn run(args: &InfoArgs) -> ExitCode {
    let (identity, version) = &args.unit;
    let registry = match Registry::locate() {
        Ok(registry) => registry,
        Err(error) => return fail(&error.to_string(), None),
    };
    match registry.info(identity, version.as_ref()) {
        Ok(info) => {
            if args.json {
                match serde_json::to_string_pretty(&info_json(&info)) {
                    Ok(text) => println!("{text}"),
                    Err(error) => return fail(&error.to_string(), None),
                }
            } else {
                print_text(&info);
            }
            ExitCode::SUCCESS
        }
        Err(failure) => fail(&failure.to_string(), hint(&failure).as_deref()),
    }
}

fn hint(failure: &InfoFailure) -> Option<String> {
    Some(match failure {
        InfoFailure::NotRegistered(_) => {
            "`assemblr registry list` shows the registered units; `assemblr registry add <path>` \
             registers a repository"
                .to_owned()
        }
        InfoFailure::NoVersions { .. } => {
            "publish a version with `assemblr unit publish`".to_owned()
        }
        InfoFailure::UnknownVersion { .. } => {
            "`assemblr registry sync` indexes newly published versions".to_owned()
        }
        InfoFailure::Source { identity, .. } => format!(
            "if the repository was moved or deleted, `assemblr registry remove {identity}` \
             removes its registration"
        ),
        InfoFailure::Invalid { .. } | InfoFailure::NotIndexed { .. } => {
            "published versions never change: restore the tag where it was, then run \
             `assemblr registry sync`"
                .to_owned()
        }
        InfoFailure::Storage(_) => return None,
    })
}

fn print_text(info: &UnitInfo) {
    let manifest = info.manifest();
    println!("{} {}", info.identity(), info.version());
    println!("  description: {}", manifest.description().as_str());
    println!("  source: {}", info.source());
    let versions: Vec<String> = info.versions().iter().map(ToString::to_string).collect();
    println!("  published versions: {}", versions.join(", "));
    println!("  commit: {}", info.provenance().commit().as_str());
    println!("  tree: {}", info.provenance().tree().as_str());
    section("dependencies", manifest.dependencies().is_empty(), || {
        for (unit, requirement) in manifest.dependencies() {
            println!("    {unit} {requirement}");
        }
    });
    let placeholders = manifest.placeholders();
    section("placeholders", placeholders.replace().is_empty(), || {
        for (placeholder, description) in placeholders.replace() {
            println!("    {placeholder}: {description}");
        }
    });
    if !placeholders.ignore().is_empty() {
        let ignored: Vec<&str> = placeholders
            .ignore()
            .iter()
            .map(|token| token.as_str())
            .collect();
        println!("  not placeholders: {}", ignored.join(", "));
    }
    section("amendments", manifest.amendments().is_empty(), || {
        for (path, declaration) in manifest.amendments() {
            println!(
                "    {path} of {} {}",
                declaration.unit(),
                declaration.base()
            );
        }
    });
}

/// A heading and its lines, or `none` when there is nothing to list.
fn section(name: &str, empty: bool, lines: impl FnOnce()) {
    if empty {
        println!("  {name}: none");
    } else {
        println!("  {name}:");
        lines();
    }
}

/// The documented, stable JSON form: every field of the text, once.
fn info_json(info: &UnitInfo) -> Value {
    let manifest = info.manifest();
    let dependencies: Map<String, Value> = manifest
        .dependencies()
        .iter()
        .map(|(unit, requirement)| (unit.to_string(), json!(requirement.to_string())))
        .collect();
    let placeholders = manifest.placeholders();
    let replace: Map<String, Value> = placeholders
        .replace()
        .iter()
        .map(|(placeholder, description)| {
            (placeholder.as_str().to_owned(), json!(description.as_str()))
        })
        .collect();
    let ignore: Vec<&str> = placeholders
        .ignore()
        .iter()
        .map(|token| token.as_str())
        .collect();
    let amendments: Map<String, Value> = manifest
        .amendments()
        .iter()
        .map(|(path, declaration)| {
            (
                path.to_string(),
                json!({
                    "unit": declaration.unit().to_string(),
                    "base": declaration.base().to_string(),
                }),
            )
        })
        .collect();
    json!({
        "identity": info.identity().to_string(),
        "version": info.version().to_string(),
        "description": manifest.description().as_str(),
        "source": info.source().as_str(),
        "versions": info.versions().iter().map(ToString::to_string).collect::<Vec<_>>(),
        "commit": info.provenance().commit().as_str(),
        "tree": info.provenance().tree().as_str(),
        "dependencies": dependencies,
        "placeholders": { "replace": replace, "ignore": ignore },
        "amendments": amendments,
    })
}
