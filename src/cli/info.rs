//! `assemblr info`: what the local registry knows about one unit, for
//! people choosing it and for agents composing with it.
//!
//! Data goes to stdout and diagnostics to stderr. `--json` derives from the
//! same [`UnitInfo`] as the text, in the schema documented in
//! `docs/domain/registries-and-discovery.md`.

use std::process::ExitCode;

use assemblr::adapters::{InfoFailure, Registry, UnitInfo};
use assemblr::domain::{ParameterDefinition, ParameterType, ParameterValue, Version};
use clap::Args;
use serde_json::{Map, Value, json};

use super::project::split_unit;

#[derive(Debug, Args)]
pub(super) struct InfoArgs {
    /// The unit, `@owner/name`, optionally followed by `@` and an exact
    /// version, as in `@owner/name@1.2.0`. Without one, the newest stable
    /// version is described.
    unit: String,
    /// Print JSON instead of text.
    #[arg(long)]
    json: bool,
}

pub(super) fn run(args: &InfoArgs) -> ExitCode {
    let (identity, version) = match split_unit(&args.unit) {
        Ok((identity, version)) => (identity, version),
        Err(message) => {
            return fail(
                &message,
                Some("write the unit as @owner/name, or @owner/name@<version>"),
            );
        }
    };
    let version = match version.as_deref().map(Version::parse).transpose() {
        Ok(version) => version,
        Err(error) => {
            return fail(
                &error.to_string(),
                Some(
                    "`assemblr info` takes an exact version; `assemblr registry list` shows the indexed ones",
                ),
            );
        }
    };
    let registry = match Registry::locate() {
        Ok(registry) => registry,
        Err(error) => return fail(&error.to_string(), None),
    };
    match registry.info(&identity, version.as_ref()) {
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

/// The type as the manifest spells it, with the allowed values of an
/// enumerated string beside it.
fn type_name(parameter: &ParameterDefinition) -> &'static str {
    match parameter.parameter_type() {
        ParameterType::String | ParameterType::EnumeratedString(_) => "string",
        ParameterType::Integer => "integer",
        ParameterType::Boolean => "boolean",
    }
}

fn allowed(parameter: &ParameterDefinition) -> Option<&[String]> {
    match parameter.parameter_type() {
        ParameterType::EnumeratedString(values) => Some(values.as_slice()),
        _ => None,
    }
}

fn value_json(value: &ParameterValue) -> Value {
    match value {
        ParameterValue::String(text) => json!(text),
        ParameterValue::Integer(number) => json!(number),
        ParameterValue::Boolean(flag) => json!(flag),
    }
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
    section("parameters", manifest.params().is_empty(), || {
        for (name, parameter) in manifest.params() {
            let mut facts = vec![type_name(parameter).to_owned()];
            match parameter.default() {
                Some(default) => facts.push(format!("default {default}")),
                None => facts.push("required".to_owned()),
            }
            if let Some(values) = allowed(parameter) {
                let shown: Vec<String> = values.iter().map(|value| format!("{value:?}")).collect();
                facts.push(format!("one of {}", shown.join(", ")));
            }
            println!(
                "    {name} ({}): {}",
                facts.join("; "),
                parameter.description().as_str()
            );
        }
    });
    section("options", manifest.options().is_empty(), || {
        for (unit, values) in manifest.options() {
            let assigned: Vec<String> = values
                .iter()
                .map(|(name, value)| format!("{name} = {value}"))
                .collect();
            println!("    {unit}: {}", assigned.join(", "));
        }
    });
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
    let params: Map<String, Value> = manifest
        .params()
        .iter()
        .map(|(name, parameter)| {
            let mut entry = Map::new();
            entry.insert("type".to_owned(), json!(type_name(parameter)));
            entry.insert(
                "description".to_owned(),
                json!(parameter.description().as_str()),
            );
            entry.insert("required".to_owned(), json!(parameter.is_required()));
            if let Some(default) = parameter.default() {
                entry.insert("default".to_owned(), value_json(default));
            }
            if let Some(values) = allowed(parameter) {
                entry.insert("allowed".to_owned(), json!(values));
            }
            (name.as_str().to_owned(), Value::Object(entry))
        })
        .collect();
    let options: Map<String, Value> = manifest
        .options()
        .iter()
        .map(|(unit, values)| {
            let values: Map<String, Value> = values
                .iter()
                .map(|(name, value)| (name.as_str().to_owned(), value_json(value)))
                .collect();
            (unit.to_string(), Value::Object(values))
        })
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
        "params": params,
        "options": options,
        "amendments": amendments,
    })
}

fn fail(message: &str, hint: Option<&str>) -> ExitCode {
    eprintln!("error: {}", message.replace('\n', "\nerror: "));
    if let Some(hint) = hint {
        eprintln!("hint: {hint}");
    }
    ExitCode::FAILURE
}
