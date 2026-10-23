//! What a run's transcript shows about how the agent used Assemblr.
//!
//! The transcript is Claude Code's `stream-json` output: one JSON event per
//! line. Lines that are not JSON are skipped.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

#[derive(Default)]
pub(crate) struct Metrics {
    pub(crate) model: String,
    /// `assemblr` invocations, and those that failed.
    pub(crate) assemblr: usize,
    pub(crate) failed: usize,
    pub(crate) ran_guide: bool,
    pub(crate) used_json: bool,
    /// Reads of Assemblr's own state: `.assemblr/` or the registry.
    pub(crate) internal_reads: usize,
    /// Subcommands the agent tried that do not exist.
    pub(crate) unknown: BTreeSet<String>,
    pub(crate) denials: usize,
    pub(crate) cost: f64,
    pub(crate) turns: u64,
    pub(crate) seconds: f64,
    /// Whether the transcript ends with Claude Code's result event.
    pub(crate) finished: bool,
}

/// Commands that read files, for telling a look at Assemblr's state apart
/// from, say, `git add .assemblr/`.
const READERS: &[&str] = &["cat", "find", "grep", "head", "less", "ls", "sed", "tail"];

const INTERNAL: &[&str] = &[
    ".assemblr/",
    "share/assemblr",
    "Application Support/assemblr",
];

pub(crate) fn metrics(transcript: &str) -> Metrics {
    let mut metrics = Metrics::default();
    let mut calls = Vec::new();
    let mut results = BTreeMap::new();
    for event in transcript
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        match event["type"].as_str() {
            Some("system") if event["subtype"] == "init" => {
                metrics.model = event["model"].as_str().unwrap_or_default().to_owned();
            }
            Some("assistant") => calls.extend(
                content(&event)
                    .filter(|item| item["type"] == "tool_use")
                    .cloned(),
            ),
            Some("user") => {
                for item in content(&event).filter(|item| item["type"] == "tool_result") {
                    let id = item["tool_use_id"].as_str().unwrap_or_default().to_owned();
                    results.insert(id, (item["is_error"] == true, text(&item["content"])));
                }
            }
            Some("result") => {
                metrics.finished = true;
                metrics.cost = event["total_cost_usd"].as_f64().unwrap_or_default();
                metrics.turns = event["num_turns"].as_u64().unwrap_or_default();
                metrics.seconds = event["duration_ms"].as_f64().unwrap_or_default() / 1000.0;
                metrics.denials = event["permission_denials"].as_array().map_or(0, Vec::len);
            }
            _ => {}
        }
    }
    for call in &calls {
        let input = &call["input"];
        let (is_error, output) = results
            .get(call["id"].as_str().unwrap_or_default())
            .map_or((false, ""), |(is_error, output)| {
                (*is_error, output.as_str())
            });
        if let Some(command) = input["command"].as_str() {
            let segments = segments(command);
            let assemblr: Vec<_> = segments
                .iter()
                .filter(|segment| first_word(segment) == "assemblr")
                .collect();
            if !assemblr.is_empty() {
                metrics.assemblr += assemblr.len();
                metrics.failed += usize::from(is_error && !denied(output));
                metrics.ran_guide |= assemblr.iter().any(|segment| segment.contains(" guide"));
                metrics.used_json |= assemblr.iter().any(|segment| segment.contains("--json"));
                metrics.unknown.extend(unknown_subcommands(output));
            }
            metrics.internal_reads += segments
                .iter()
                .filter(|segment| READERS.contains(&first_word(segment)) && internal(segment))
                .count();
        } else {
            let path = ["file_path", "path", "pattern"]
                .iter()
                .filter_map(|key| input[key].as_str())
                .any(internal);
            metrics.internal_reads += usize::from(path);
        }
    }
    metrics
}

fn content(event: &Value) -> impl Iterator<Item = &Value> {
    event["message"]["content"].as_array().into_iter().flatten()
}

/// A tool result's text, which is a string or a list of text blocks.
fn text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The simple commands of a shell command line, split at `;`, `&&`, `||`,
/// `|`, and line breaks. Quoting is ignored, which is close enough for the
/// commands agents write.
fn segments(command: &str) -> Vec<String> {
    command
        .split(['\n', ';', '|', '&'])
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect()
}

fn first_word(segment: &str) -> &str {
    segment.split_whitespace().next().unwrap_or_default()
}

fn internal(text: &str) -> bool {
    INTERNAL.iter().any(|marker| text.contains(marker))
}

fn denied(output: &str) -> bool {
    output.contains("requires approval") || output.contains("permission")
}

/// The subcommands clap reports as unrecognized in `output`.
fn unknown_subcommands(output: &str) -> Vec<String> {
    let marker = "unrecognized subcommand '";
    output
        .match_indices(marker)
        .filter_map(|(start, _)| {
            let rest = &output[start + marker.len()..];
            rest.find('\'').map(|end| rest[..end].to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::metrics;

    #[test]
    fn counts_assemblr_commands_failures_and_unknown_subcommands() {
        let transcript = [
            r#"{"type":"system","subtype":"init","model":"claude-test"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"1","name":"Bash","input":{"command":"cd api && assemblr guide | head"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"1","content":"Assemblr ..."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"2","name":"Bash","input":{"command":"assemblr status"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"2","is_error":true,"content":[{"type":"text","text":"error: unrecognized subcommand 'status'"}]}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"3","name":"Read","input":{"file_path":"/p/.assemblr/journal.json"}},{"type":"tool_use","id":"4","name":"Bash","input":{"command":"git add .assemblr/ && cat .assemblr/x"}}]}}"#,
            "not json",
            r#"{"type":"result","total_cost_usd":0.25,"num_turns":7,"duration_ms":61000,"permission_denials":[{}]}"#,
        ]
        .join("\n");
        let metrics = metrics(&transcript);
        assert_eq!(metrics.model, "claude-test");
        assert_eq!((metrics.assemblr, metrics.failed), (2, 1));
        assert!(metrics.ran_guide && !metrics.used_json && metrics.finished);
        assert_eq!(metrics.unknown.into_iter().collect::<Vec<_>>(), ["status"]);
        assert_eq!(metrics.internal_reads, 2);
        assert_eq!((metrics.denials, metrics.turns), (1, 7));
        assert!((metrics.cost - 0.25).abs() < f64::EPSILON);
    }
}
