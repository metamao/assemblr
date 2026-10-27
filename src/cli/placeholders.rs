//! The placeholders that remain in a project, as `compose`, `add`,
//! `update`, and `status` show them, and the tokens a unit's build finds
//! undeclared (see `docs/domain/placeholders.md`).
//!
//! Each block is separated by a blank line: the placeholder, then each
//! declaration in scope, `<unit>: <description>`, indented once more, then
//! its locations once more again, one `path:line` per row so that terminals
//! and editors can link it, a line that holds the placeholder twice listed
//! once. Nothing is truncated.

use std::collections::BTreeSet;

use assemblr::domain::{Location, PlaceholderReport, UnresolvedTokens};
use serde_json::{Value, json};

use super::diagnostic::count;

/// The heading `compose`, `add`, and `update` print above [`body`].
pub(super) fn heading(report: &PlaceholderReport) -> String {
    format!(
        "{} to replace, in {}:",
        count(report.placeholders().len(), "placeholder", "placeholders"),
        count(report.files(), "file", "files")
    )
}

/// The hint `compose`, `add`, and `update` print below [`body`].
pub(super) const HINT: &str =
    "replace each placeholder with your value; `assemblr status` shows what remains";

/// The lines of `report`, each block after a blank line.
pub(super) fn body(report: &PlaceholderReport) -> Vec<String> {
    let mut lines = Vec::new();
    for (placeholder, declarations) in report.placeholders() {
        lines.push(String::new());
        lines.push(format!("  {placeholder}"));
        for (unit, remaining) in declarations {
            lines.push(String::new());
            lines.push(format!("    {unit}: {}", remaining.description()));
            lines.extend(location_lines(remaining.locations()));
        }
    }
    lines
}

/// The documented, stable JSON form of `report`, sorted by placeholder,
/// then unit, then location.
pub(super) fn report_json(report: &PlaceholderReport) -> Value {
    let placeholders: Vec<Value> = report
        .placeholders()
        .iter()
        .map(|(placeholder, declarations)| {
            let declarations: Vec<Value> = declarations
                .iter()
                .map(|(unit, remaining)| {
                    let locations: Vec<Value> = remaining
                        .locations()
                        .iter()
                        .map(|location| {
                            json!({
                                "path": location.path().as_str(),
                                "line": location.line(),
                                "column": location.column(),
                            })
                        })
                        .collect();
                    json!({
                        "unit": unit.to_string(),
                        "description": remaining.description().as_str(),
                        "locations": locations,
                    })
                })
                .collect();
            json!({
                "placeholder": placeholder.as_str(),
                "declarations": declarations,
            })
        })
        .collect();
    Value::Array(placeholders)
}

/// The lines of the tokens a build finds undeclared, each token after a
/// blank line with its locations below it.
pub(super) fn tokens_body(tokens: &UnresolvedTokens) -> Vec<String> {
    let mut lines = Vec::new();
    for (token, locations) in tokens {
        lines.push(String::new());
        lines.push(format!("  {token}"));
        lines.extend(location_lines(locations));
    }
    lines
}

/// One row per `path:line`, in order.
fn location_lines(locations: &BTreeSet<Location>) -> Vec<String> {
    let mut lines: Vec<String> = locations
        .iter()
        .map(|location| format!("      {location}"))
        .collect();
    lines.dedup();
    lines
}

/// The hint for tokens a build or a publication finds undeclared.
pub(super) fn tokens_hint(tokens: &UnresolvedTokens) -> String {
    let subject = if tokens.len() == 1 { "it" } else { "each one" };
    format!(
        "add {subject} to \"placeholders.replace\" with a description, or to \
         \"placeholders.ignore\" if it is not a placeholder"
    )
}
