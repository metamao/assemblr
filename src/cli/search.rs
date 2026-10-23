//! `assemblr search`: the registered units whose declared metadata matches
//! the terms given, read offline from the local index.
//!
//! Data goes to stdout and diagnostics to stderr. `--json` derives from the
//! same [`Search`] as the text, in the schema documented in
//! `docs/domain/registries-and-discovery.md`.

use std::process::ExitCode;

use assemblr::adapters::{Found, Registry, Search};
use assemblr::domain::SearchQuery;
use clap::Args;
use serde_json::{Value, json};

use super::diagnostic::fail;

#[derive(Debug, Args)]
pub(super) struct SearchArgs {
    /// The terms to find, like `cloudflare worker`; each must occur in the
    /// unit's name or in its newest stable version's description
    #[arg(value_name = "TERM", required = true)]
    terms: Vec<String>,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

pub(super) fn run(args: &SearchArgs) -> ExitCode {
    let query = match SearchQuery::parse(&args.terms.join(" ")) {
        Ok(query) => query,
        Err(error) => {
            return fail(
                &error.to_string(),
                Some("give at least one term, like `assemblr search cloudflare`"),
            );
        }
    };
    let registry = match Registry::locate() {
        Ok(registry) => registry,
        Err(error) => return fail(&error.to_string(), None),
    };
    let search = match registry.search(&query) {
        Ok(search) => search,
        Err(error) => return fail(&error.to_string(), None),
    };
    for failure in search.undescribed() {
        eprintln!(
            "warning: {failure}; it was matched by name only, so it may be missing from the \
             results"
        );
    }
    if args.json {
        match serde_json::to_string_pretty(&search_json(&search)) {
            Ok(text) => println!("{text}"),
            Err(error) => return fail(&error.to_string(), None),
        }
    } else {
        for found in search.found() {
            println!("{}", line(found));
        }
    }
    if search.found().is_empty() {
        eprintln!(
            "note: no registered unit matches {:?}; `assemblr registry list` lists every unit",
            args.terms.join(" ")
        );
    }
    ExitCode::SUCCESS
}

/// The name, version, and description of a unit, or why it has none.
fn line(found: &Found) -> String {
    match found.info() {
        Some(info) => format!(
            "{} {}: {}",
            found.identity(),
            info.version(),
            info.manifest().description().as_str()
        ),
        None if found.has_versions() => {
            format!(
                "{} (not described: see the warning above)",
                found.identity()
            )
        }
        None => format!("{} (no published version)", found.identity()),
    }
}

/// The documented, stable JSON form: every field of the text, once.
fn search_json(search: &Search) -> Value {
    let units: Vec<Value> = search
        .found()
        .iter()
        .map(|found| {
            let info = found.info();
            json!({
                "identity": found.identity().to_string(),
                "source": found.source().as_str(),
                "version": info.map(|info| info.version().to_string()),
                "description": info.map(|info| info.manifest().description().as_str()),
            })
        })
        .collect();
    json!({ "units": units })
}
