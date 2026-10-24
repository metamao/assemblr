//! `assemblr status`: where a consumer project's composition stands, read
//! without changing anything.
//!
//! The report goes to stdout, and what kept part of it from being read
//! goes to stderr as warnings. `--json` derives from the same
//! [`ProjectStatus`] as the text, in the schema documented in
//! `docs/domain/consumer-projects.md`.

use std::path::Path;
use std::process::ExitCode;

use assemblr::adapters::{
    ComposeProblem, CompositionState, Origin, ProjectStatus, Registry, UnitStatus, project_status,
};
use assemblr::domain::{RelativePath, UnitIdentity};
use clap::Args;
use serde_json::{Value, json};

use super::diagnostic::{Diagnosis, count, fail, fail_with};
use super::project::problem_advice;

#[derive(Debug, Args)]
pub(super) struct StatusArgs {
    /// List every file with the units that deliver it
    #[arg(long)]
    files: bool,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

pub(super) fn run(args: &StatusArgs) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(root) => root,
        Err(error) => {
            return fail(
                &format!("could not read the current directory: {error}"),
                None,
            );
        }
    };
    let registry = match Registry::locate() {
        Ok(registry) => registry,
        Err(error) => return fail(&error.to_string(), None),
    };
    let status = match project_status(&root, &registry) {
        Ok(status) => status,
        Err(problem) => return fail_with(&problem.to_string(), &diagnosis(&problem, &root)),
    };
    for problem in status.problems() {
        eprintln!(
            "warning: {}",
            problem.to_string().replace('\n', "\nwarning: ")
        );
        if let Some(advice) = problem_advice(problem, &root) {
            eprintln!("hint: {advice}");
        }
    }
    if args.json {
        match serde_json::to_string_pretty(&status_json(&status)) {
            Ok(text) => println!("{text}"),
            Err(error) => return fail(&error.to_string(), None),
        }
    } else {
        report(&status, args.files);
    }
    ExitCode::SUCCESS
}

/// What to do about a project whose state cannot be read. `status`
/// changes nothing, so there is no note saying so.
fn diagnosis(problem: &ComposeProblem, root: &Path) -> Diagnosis {
    match problem {
        ComposeProblem::UnitRepository(_) => {
            Diagnosis::hint("in a unit's repository, `assemblr unit check` reports what is wrong")
        }
        _ => match problem_advice(problem, root) {
            Some(advice) => Diagnosis::hint(advice),
            None => Diagnosis::default(),
        },
    }
}

fn report(status: &ProjectStatus, all_files: bool) {
    let locked = status
        .units()
        .iter()
        .filter(|unit| unit.version().is_some())
        .count();
    let origin = |path: &RelativePath| {
        status
            .origins()
            .and_then(|origins| origins.get(path))
            .map(|origin| format!(", {}", describe_origin(origin)))
            .unwrap_or_default()
    };
    match status.state() {
        CompositionState::Uncomposed => println!("The project was never composed."),
        CompositionState::Composed => {
            let units = count(locked, "unit", "units");
            match status.up_to_date() {
                Some(true) => {
                    println!("Composed from {units}; `assemblr compose` has nothing to do.");
                }
                Some(false) => println!(
                    "Composed from {units}; assemblr.jsonc changed since, so `assemblr compose` \
                     would change the project."
                ),
                None => println!("Composed from {units}."),
            }
        }
        CompositionState::Pending { files, .. } => {
            println!(
                "A composition is waiting for you to resolve conflicts in {}:",
                count(files.len(), "file", "files")
            );
            for file in files {
                let state = if file.marked() {
                    "holds conflict markers"
                } else {
                    "resolved"
                };
                println!("  {}: {state}{}", file.path(), origin(file.path()));
            }
        }
        CompositionState::Interrupted => println!("A composition was interrupted."),
    }
    if !status.units().is_empty() {
        println!("Units:");
        for unit in status.units() {
            println!("  {}", unit_line(unit));
        }
    }
    if !status.edited().is_empty() {
        println!("Edited since the last composition:");
        for (path, edit) in status.edited() {
            println!("  {path}: {}{}", edit.as_str(), origin(path));
        }
    }
    if all_files {
        if let Some(origins) = status.origins() {
            println!("Files:");
            for (path, origin) in origins {
                println!("  {path}: {}", describe_origin(origin));
            }
        }
    }
    if let Some(next) = next_step(status) {
        println!("Next: {next}");
    }
}

/// A unit, its version, why the project has it, and what is published
/// beyond it.
fn unit_line(unit: &UnitStatus) -> String {
    let mut line = unit.identity().to_string();
    if let Some(version) = unit.version() {
        line.push_str(&format!(" {version}"));
    }
    // Who requires a unit is known only from a planned composition; without
    // one, a unit that is not selected is not said to be retired.
    let required_by = unit
        .required_by()
        .filter(|units| !units.is_empty())
        .map(|units| format!("required by {}", join(units)));
    let why = match (unit.requirement(), required_by) {
        (Some(requirement), Some(required_by)) => {
            Some(format!("{requirement}; also {required_by}"))
        }
        (Some(requirement), None) => Some(requirement.to_string()),
        (None, Some(required_by)) => Some(required_by),
        (None, None) => unit
            .required_by()
            .is_some()
            .then(|| "no longer selected".to_owned()),
    };
    if let Some(why) = why {
        line.push_str(&format!(" ({why})"));
    }
    let mut beyond = Vec::new();
    if unit.version().is_none() {
        beyond.push("not composed yet".to_owned());
    }
    if let Some(update) = unit.update() {
        beyond.push(format!("update to {update} available"));
    }
    match (unit.newest(), unit.requirement()) {
        // `add` replaces the requirement of a unit selected already, which
        // is what holds it back when that requirement rejects the version.
        (Some(newest), Some(requirement)) if !requirement.matches(newest) => {
            beyond.push(format!(
                "{newest} published, outside {requirement}; `assemblr add {}@^{newest}` allows it",
                unit.identity()
            ));
        }
        (Some(newest), _) => beyond.push(format!(
            "{newest} published, outside what the units that require it allow"
        )),
        (None, _) => {}
    }
    if !beyond.is_empty() {
        line.push_str(&format!(": {}", beyond.join("; ")));
    }
    line
}

fn describe_origin(origin: &Origin) -> String {
    if origin.amenders().is_empty() {
        format!("from {}", origin.unit())
    } else {
        format!(
            "from {}, amended by {}",
            origin.unit(),
            join(origin.amenders())
        )
    }
}

fn join(units: &[UnitIdentity]) -> String {
    let names: Vec<String> = units.iter().map(ToString::to_string).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// The one thing to do next, when there is one, most urgent first.
fn next_step(status: &ProjectStatus) -> Option<String> {
    match status.state() {
        CompositionState::Pending {
            declaration_changed: true,
            ..
        } => Some(
            "put assemblr.jsonc back as the composition left it, then run `assemblr compose \
             --continue`; `assemblr compose --abort` gives up the composition instead"
                .to_owned(),
        ),
        CompositionState::Pending { files, .. } => {
            Some(if files.iter().any(|file| file.marked()) {
                "resolve the conflict markers in the files marked above, then run `assemblr compose \
             --continue`; `assemblr compose --abort` gives up the composition instead"
                .to_owned()
            } else {
                "run `assemblr compose --continue` to finish the composition".to_owned()
            })
        }
        CompositionState::Interrupted => Some(
            "run `assemblr compose`: it finishes or reverts the interrupted composition, then \
             composes"
                .to_owned(),
        ),
        CompositionState::Uncomposed if status.units().is_empty() => {
            Some("select a unit with `assemblr add <UNIT>`".to_owned())
        }
        CompositionState::Uncomposed => Some("run `assemblr compose`".to_owned()),
        CompositionState::Composed if status.up_to_date() == Some(false) => {
            Some("run `assemblr compose` to apply assemblr.jsonc".to_owned())
        }
        CompositionState::Composed if status.units().iter().any(|unit| unit.update().is_some()) => {
            Some("run `assemblr update` to move to the newest versions allowed".to_owned())
        }
        CompositionState::Composed => None,
    }
}

fn status_json(status: &ProjectStatus) -> Value {
    let state = match status.state() {
        CompositionState::Uncomposed => "uncomposed",
        CompositionState::Composed => "composed",
        CompositionState::Pending { .. } => "pending",
        CompositionState::Interrupted => "interrupted",
    };
    let names =
        |units: &[UnitIdentity]| -> Vec<String> { units.iter().map(ToString::to_string).collect() };
    let units: Vec<Value> = status
        .units()
        .iter()
        .map(|unit| {
            json!({
                "unit": unit.identity().to_string(),
                "version": unit.version().map(ToString::to_string),
                "requirement": unit.requirement().map(ToString::to_string),
                "required_by": unit.required_by().map(names),
                "update": unit.update().map(ToString::to_string),
                "newest": unit.newest().map(ToString::to_string),
            })
        })
        .collect();
    let pending: Vec<Value> = match status.state() {
        CompositionState::Pending { files, .. } => files
            .iter()
            .map(|file| json!({ "path": file.path().as_str(), "marked": file.marked() }))
            .collect(),
        _ => Vec::new(),
    };
    let edited: Vec<Value> = status
        .edited()
        .iter()
        .map(|(path, edit)| json!({ "path": path.as_str(), "edit": edit.as_str() }))
        .collect();
    let files = status.origins().map(|origins| {
        origins
            .iter()
            .map(|(path, origin)| {
                json!({
                    "path": path.as_str(),
                    "unit": origin.unit().to_string(),
                    "amended_by": names(origin.amenders()),
                })
            })
            .collect::<Vec<Value>>()
    });
    let problems: Vec<String> = status.problems().iter().map(ToString::to_string).collect();
    json!({
        "state": state,
        "up_to_date": status.up_to_date(),
        "units": units,
        "pending": pending,
        "edited": edited,
        "files": files,
        "problems": problems,
    })
}
