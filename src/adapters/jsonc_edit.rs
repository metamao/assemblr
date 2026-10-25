//! Reading a JSON or JSONC document into the merge's value model, and
//! editing its text in place by value paths, keeping its layout (see
//! `docs/adr/0015-value-merge.md`).
//!
//! The reader is strict: duplicate keys are rejected, since a document with
//! two values for one key means different things to different readers. The
//! editor locates members with the span scanner of [`super::jsonc`], which
//! works on the comment-blanked text so that every offset is an offset in
//! the original, and splices the original text. It follows the file's own
//! conventions, its indentation, its line endings, and whether an object is
//! written on one line, so that an edited file reads as if its author had
//! written the change. Each edit is located afresh in the text the previous
//! edits produced; the files are small and the edits few.
//!
//! The editor only locates and splices. Whether the edited text holds the
//! planned value is checked afterwards by the merge, by reading the result
//! back (the projection check): a layout the editor misjudges is a refusal,
//! never a silently different file.

use std::fmt;

use super::jsonc::{self, Json, Member};
use crate::domain::{Edit, Value, show_path};

/// Reads `text` as a JSON or JSONC document into a value, rejecting
/// duplicate keys.
pub(crate) fn parse_value(text: &str) -> Result<Value, ValueError> {
    let json = jsonc::parse(text).map_err(ValueError::Syntax)?;
    let mut path = Vec::new();
    convert(&json, &mut path)
}

fn convert(json: &Json, path: &mut Vec<String>) -> Result<Value, ValueError> {
    Ok(match json {
        Json::Null => Value::Null,
        Json::Bool(value) => Value::Bool(*value),
        Json::Integer(value) => Value::Number(value.to_string()),
        Json::OtherNumber(text) => Value::Number(text.clone()),
        Json::String(text) => Value::String(text.clone()),
        Json::Array(items) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                // Items have no path of their own: a list is one value.
                values.push(convert(item, path)?);
            }
            Value::Array(values)
        }
        Json::Object(members) => {
            let mut values: Vec<(String, Value)> = Vec::with_capacity(members.len());
            for (key, value) in members {
                if values.iter().any(|(existing, _)| existing == key) {
                    return Err(ValueError::DuplicateKey {
                        path: show_path(path),
                        key: key.clone(),
                    });
                }
                path.push(key.clone());
                let value = convert(value, path)?;
                path.pop();
                values.push((key.clone(), value));
            }
            Value::Object(values)
        }
    })
}

/// Why text is not a document the merge can work on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueError {
    /// The text is not valid JSONC.
    Syntax(jsonc::SyntaxError),
    /// An object has two members with the same key.
    DuplicateKey {
        /// The object, as a path.
        path: String,
        /// The key.
        key: String,
    },
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(error) => write!(f, "it is not valid JSONC: {error}"),
            Self::DuplicateKey { path, key } => write!(
                f,
                "the object at {path} has the key {key:?} twice, which readers resolve \
                 differently"
            ),
        }
    }
}

impl std::error::Error for ValueError {}

/// Applies `edits` to `text`, in order, and returns the edited text.
pub(crate) fn apply(text: &str, edits: &[Edit]) -> Result<String, EditError> {
    let mut text = text.to_owned();
    for edit in edits {
        text = apply_one(&text, edit)?;
    }
    Ok(text)
}

/// The editor could not locate an edit precisely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditError {
    edit: String,
    reason: &'static str,
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "could not {}: {}", self.edit, self.reason)
    }
}

impl std::error::Error for EditError {}

/// The document's layout conventions.
struct Layout {
    newline: &'static str,
    /// One level of indentation, from the first indented member found.
    unit: String,
    /// Whether the last member of a container written over several lines
    /// takes a comma, as it does when the file has a trailing comma
    /// anywhere: the style Prettier writes JSONC in by default.
    trailing_commas: bool,
}

impl Layout {
    fn of(text: &str) -> Self {
        let newline = match text.find('\n') {
            Some(at) if text[..at].ends_with('\r') => "\r\n",
            _ => "\n",
        };
        let unit = text
            .lines()
            .find_map(|line| {
                let trimmed = line.trim_start_matches([' ', '\t']);
                (trimmed.starts_with('"') && trimmed.len() < line.len())
                    .then(|| line[..line.len() - trimmed.len()].to_owned())
            })
            .unwrap_or_else(|| "  ".to_owned());
        Self {
            newline,
            unit,
            trailing_commas: jsonc::has_trailing_comma(text),
        }
    }

    /// The comma after the last member of a container written over several
    /// lines, when the file's style has one.
    fn trailing(&self) -> &'static str {
        if self.trailing_commas { "," } else { "" }
    }

    /// `value` on one line, as a member of an object written on one line:
    /// `{ "key": value }` and `[item, item]`.
    fn render_inline(value: &Value) -> String {
        match value {
            Value::Object(members) if !members.is_empty() => {
                let members: Vec<String> = members
                    .iter()
                    .map(|(key, value)| {
                        format!(
                            "{}: {}",
                            Value::String(key.clone()),
                            Self::render_inline(value)
                        )
                    })
                    .collect();
                format!("{{ {} }}", members.join(", "))
            }
            Value::Array(items) => {
                let items: Vec<String> = items.iter().map(Self::render_inline).collect();
                format!("[{}]", items.join(", "))
            }
            scalar => scalar.to_string(),
        }
    }

    /// `value` as text, with its nested lines indented from `indent`.
    fn render(&self, value: &Value, indent: &str) -> String {
        match value {
            Value::Object(members) if !members.is_empty() => {
                let inner = format!("{indent}{}", self.unit);
                let mut out = String::from("{");
                for (index, (key, value)) in members.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(self.newline);
                    out.push_str(&inner);
                    out.push_str(&Value::String(key.clone()).to_string());
                    out.push_str(": ");
                    out.push_str(&self.render(value, &inner));
                }
                out.push_str(self.trailing());
                out.push_str(self.newline);
                out.push_str(indent);
                out.push('}');
                out
            }
            Value::Array(items)
                if !items.is_empty()
                    && items
                        .iter()
                        .any(|item| matches!(item, Value::Object(_) | Value::Array(_))) =>
            {
                let inner = format!("{indent}{}", self.unit);
                let mut out = String::from("[");
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(self.newline);
                    out.push_str(&inner);
                    out.push_str(&self.render(item, &inner));
                }
                out.push_str(self.trailing());
                out.push_str(self.newline);
                out.push_str(indent);
                out.push(']');
                out
            }
            Value::Array(items) => {
                let items: Vec<String> = items.iter().map(ToString::to_string).collect();
                format!("[{}]", items.join(", "))
            }
            scalar => scalar.to_string(),
        }
    }
}

/// An object located in the text: its braces and its members.
struct Located {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

/// Locates the object at `path` in `text`, whose comment-blanked form is
/// `strict`.
fn locate(strict: &str, path: &[String]) -> Option<Located> {
    let mut open = jsonc::root_object(strict)?;
    for key in path {
        let (members, _) = jsonc::members(strict, open)?;
        let member = members.iter().find(|member| &member.key == key)?;
        open = member.value_start;
    }
    let (members, close) = jsonc::members(strict, open)?;
    Some(Located {
        open,
        close,
        members,
    })
}

/// The leading whitespace of the line `at` is on, when nothing but
/// whitespace precedes `at` on it.
fn indent_before(text: &str, at: usize) -> Option<String> {
    let line_start = text[..at].rfind('\n').map_or(0, |index| index + 1);
    let indent = &text[line_start..at];
    indent
        .chars()
        .all(|c| c == ' ' || c == '\t')
        .then(|| indent.to_owned())
}

/// The offset just past the comma that follows the value ending at `at`,
/// if the next significant character is a comma.
fn comma_after(strict: &str, text: &str, at: usize) -> Option<usize> {
    // The strict text blanks a trailing comma, so the original is read.
    let mut index = at;
    let bytes = strict.as_bytes();
    while index < bytes.len() && matches!(bytes[index], b' ' | b'\t' | b'\n' | b'\r') {
        // A blanked trailing comma, or a blanked comment, is spaces in
        // `strict` but not in `text`: the comma counts, and a comment is
        // skipped by its own syntax.
        let rest = &text[index..];
        if rest.starts_with(',') {
            return Some(index + 1);
        } else if rest.starts_with("//") {
            index += rest.find('\n').unwrap_or(rest.len());
        } else if rest.starts_with("/*") {
            index += rest.find("*/").map_or(rest.len(), |end| end + 2);
        } else if text.as_bytes()[index] != bytes[index] {
            return None;
        } else {
            index += 1;
        }
    }
    (text.as_bytes().get(index) == Some(&b',')).then_some(index + 1)
}

/// The offset of the end of the line `at` is on, when only whitespace, a
/// comma, or a comment follow `at` on that line: the position of its line
/// break, or the text's end.
fn line_end_after(strict: &str, text: &str, at: usize) -> Option<usize> {
    let mut end = text[at..].find('\n').map_or(text.len(), |index| at + index);
    if text[..end].ends_with('\r') {
        end -= 1;
    }
    let rest = &strict[at..end];
    let significant = rest.chars().all(|c| c == ' ' || c == '\t' || c == ',');
    significant.then_some(end)
}

fn apply_one(text: &str, edit: &Edit) -> Result<String, EditError> {
    let layout = Layout::of(text);
    let strict = jsonc::to_strict_json(text).map_err(|_| EditError {
        edit: describe(edit),
        reason: "the text is not valid JSONC",
    })?;
    let failure = |reason: &'static str| EditError {
        edit: describe(edit),
        reason,
    };
    match edit {
        Edit::Insert {
            parent,
            key,
            value,
            after,
        } => {
            let object = locate(&strict, parent)
                .ok_or_else(|| failure("its parent is not an object in the text"))?;
            if object.members.iter().any(|member| &member.key == key) {
                return Err(failure("the member already exists"));
            }
            let multiline = text[object.open..object.close].contains('\n');
            let member_indent = object
                .members
                .first()
                .and_then(|first| indent_before(text, first.key_start))
                .or_else(|| {
                    // An empty object: one level in from the line that
                    // opens it.
                    let line_start = text[..object.open].rfind('\n').map_or(0, |index| index + 1);
                    let line = &text[line_start..object.open];
                    let indent: String = line
                        .chars()
                        .take_while(|c| *c == ' ' || *c == '\t')
                        .collect();
                    Some(format!("{indent}{}", layout.unit))
                })
                .unwrap_or_else(|| layout.unit.clone());
            // A member of an object written on one line is written on one
            // line too.
            let rendered = if multiline || object.members.is_empty() {
                layout.render(value, &member_indent)
            } else {
                Layout::render_inline(value)
            };
            let member = format!("{}: {}", Value::String(key.clone()), rendered);
            let mut edited = String::with_capacity(text.len() + member.len() + 8);
            match after {
                None => {
                    if let Some(first) = object.members.first() {
                        // Before the first member, which keeps its own line.
                        let at = first.key_start;
                        edited.push_str(&text[..at]);
                        edited.push_str(&member);
                        edited.push(',');
                        if multiline {
                            edited.push_str(layout.newline);
                            edited.push_str(&member_indent);
                        } else {
                            edited.push(' ');
                        }
                        edited.push_str(&text[at..]);
                    } else {
                        // The only member of an empty object.
                        let close = object.close;
                        let closing_indent =
                            text[..object.open].rfind('\n').map_or(0, |index| index + 1);
                        let closing_indent: String = text[closing_indent..object.open]
                            .chars()
                            .take_while(|c| *c == ' ' || *c == '\t')
                            .collect();
                        edited.push_str(&text[..object.open + 1]);
                        if text.contains('\n') {
                            edited.push_str(layout.newline);
                            edited.push_str(&member_indent);
                            edited.push_str(&member);
                            edited.push_str(layout.trailing());
                            edited.push_str(layout.newline);
                            edited.push_str(&closing_indent);
                        } else {
                            edited.push(' ');
                            edited.push_str(&member);
                            edited.push(' ');
                        }
                        edited.push_str(&text[close..]);
                    }
                }
                Some(after) => {
                    let anchor = object
                        .members
                        .iter()
                        .find(|member| &member.key == after)
                        .ok_or_else(|| failure("the member it follows is not in the text"))?;
                    let comma = comma_after(&strict, text, anchor.value_end);
                    if multiline {
                        let line_end = line_end_after(&strict, text, anchor.value_end)
                            .unwrap_or(comma.unwrap_or(anchor.value_end));
                        match comma {
                            Some(_) => {
                                edited.push_str(&text[..line_end]);
                                edited.push_str(layout.newline);
                                edited.push_str(&member_indent);
                                edited.push_str(&member);
                                edited.push(',');
                                edited.push_str(&text[line_end..]);
                            }
                            None => {
                                edited.push_str(&text[..anchor.value_end]);
                                edited.push(',');
                                edited.push_str(&text[anchor.value_end..line_end]);
                                edited.push_str(layout.newline);
                                edited.push_str(&member_indent);
                                edited.push_str(&member);
                                edited.push_str(layout.trailing());
                                edited.push_str(&text[line_end..]);
                            }
                        }
                    } else {
                        let at = comma.unwrap_or(anchor.value_end);
                        edited.push_str(&text[..at]);
                        if comma.is_none() {
                            edited.push(',');
                        }
                        edited.push(' ');
                        edited.push_str(&member);
                        if comma.is_some() {
                            edited.push(',');
                        }
                        edited.push_str(&text[at..]);
                    }
                }
            }
            Ok(edited)
        }
        Edit::Set { path, value } => {
            let Some((key, parent)) = path.split_last() else {
                // The whole document.
                let open = jsonc::root_object(&strict)
                    .ok_or_else(|| failure("the document is not an object"))?;
                let (_, close) = jsonc::members(&strict, open)
                    .ok_or_else(|| failure("the document is not an object"))?;
                let mut edited = text[..open].to_owned();
                edited.push_str(&layout.render(value, ""));
                edited.push_str(&text[close + 1..]);
                return Ok(edited);
            };
            let object = locate(&strict, parent)
                .ok_or_else(|| failure("its parent is not an object in the text"))?;
            let member = object
                .members
                .iter()
                .find(|member| &member.key == key)
                .ok_or_else(|| failure("the member is not in the text"))?;
            let indent = indent_before(text, member.key_start).unwrap_or_default();
            let mut edited = text[..member.value_start].to_owned();
            edited.push_str(&layout.render(value, &indent));
            edited.push_str(&text[member.value_end..]);
            Ok(edited)
        }
        Edit::Delete { path } => {
            let (key, parent) = path
                .split_last()
                .ok_or_else(|| failure("the document itself cannot be removed"))?;
            let object = locate(&strict, parent)
                .ok_or_else(|| failure("its parent is not an object in the text"))?;
            let index = object
                .members
                .iter()
                .position(|member| &member.key == key)
                .ok_or_else(|| failure("the member is not in the text"))?;
            let member = &object.members[index];
            let comma = comma_after(&strict, text, member.value_end);
            // From the start of the member's own line when it has one, to
            // the end of its line when nothing else is on it.
            let whole_line = indent_before(text, member.key_start).is_some()
                && line_end_after(&strict, text, member.value_end).is_some();
            let (start, end) = if whole_line {
                let line_start = text[..member.key_start].rfind('\n').map_or(0, |at| at + 1);
                let line_end = line_end_after(&strict, text, member.value_end)
                    .ok_or_else(|| failure("the member's line could not be bounded"))?;
                let past_break = if text[line_end..].starts_with("\r\n") {
                    line_end + 2
                } else if text[line_end..].starts_with('\n') {
                    line_end + 1
                } else {
                    line_end
                };
                (line_start, past_break)
            } else {
                let end = comma.unwrap_or(member.value_end);
                let end = end + text[end..].len() - text[end..].trim_start_matches(' ').len();
                (member.key_start, end)
            };
            let mut edited = text[..start].to_owned();
            edited.push_str(&text[end..]);
            // The last member leaves the one before it with a comma that
            // would now be trailing: removed, unless the removed member
            // had a trailing comma itself, which is the file's style.
            if comma.is_none() && index > 0 {
                let previous = &object.members[index - 1];
                let strict_edited = jsonc::to_strict_json(&edited)
                    .map_err(|_| failure("the edited text is not valid JSONC"))?;
                let probe = jsonc::members(
                    &strict_edited,
                    locate(&strict_edited, parent).map(|o| o.open).unwrap_or(0),
                );
                if let Some((members, _)) = probe {
                    if let Some(previous) = members.iter().find(|m| m.key == previous.key) {
                        if let Some(past_comma) =
                            comma_after(&strict_edited, &edited, previous.value_end)
                        {
                            // Only a comma that is now trailing: nothing
                            // but whitespace up to the closing brace.
                            let rest = &strict_edited[past_comma..];
                            let next = rest.trim_start_matches([' ', '\t', '\r', '\n']);
                            if next.starts_with('}') {
                                edited.replace_range(past_comma - 1..past_comma, "");
                            }
                        }
                    }
                }
            }
            Ok(edited)
        }
    }
}

/// The comments of a JSONC text, in order: the runs where the text differs
/// from its comment-blanked form.
pub(crate) fn comments_of(text: &str) -> Vec<String> {
    let Ok(strict) = jsonc::to_strict_json(text) else {
        return Vec::new();
    };
    let (text_bytes, strict_bytes) = (text.as_bytes(), strict.as_bytes());
    let mut comments = Vec::new();
    let mut index = 0;
    while index < text_bytes.len() {
        if text_bytes[index] == strict_bytes[index] {
            index += 1;
            continue;
        }
        // A blanked comment runs to its line's end or its closer; a blanked
        // trailing comma is one byte and no comment. Spaces inside a comment
        // match the blanked form, so the run is bounded by the comment's
        // own syntax, not by the differing bytes.
        let rest = &text[index..];
        let end = if rest.starts_with("//") {
            index + rest.find('\n').unwrap_or(rest.len())
        } else if rest.starts_with("/*") {
            index + rest.find("*/").map_or(rest.len(), |at| at + 2)
        } else {
            index += 1;
            continue;
        };
        comments.push(text[index..end].trim_end().to_owned());
        index = end;
    }
    comments
}

fn describe(edit: &Edit) -> String {
    match edit {
        Edit::Insert { parent, key, .. } => {
            format!("insert {key:?} into {}", show_path(parent))
        }
        Edit::Set { path, .. } => format!("replace the value at {}", show_path(path)),
        Edit::Delete { path } => format!("remove {}", show_path(path)),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]

    use super::{ValueError, apply, parse_value};
    use crate::domain::{Edit, Value};

    fn s(text: &str) -> Value {
        Value::String(text.to_owned())
    }

    fn p(text: &str) -> Vec<String> {
        text.split('/')
            .filter(|k| !k.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn insert(parent: &str, key: &str, value: Value, after: Option<&str>) -> Edit {
        Edit::Insert {
            parent: p(parent),
            key: key.to_owned(),
            value,
            after: after.map(str::to_owned),
        }
    }

    /// Applies and checks that the result reads back as `expected`.
    fn edited(text: &str, edits: &[Edit]) -> String {
        let result = apply(text, edits).unwrap();
        parse_value(&result).unwrap();
        result
    }

    const PACKAGE: &str = "{\n  \"name\": \"app\",\n  \"scripts\": {\n    \"build\": \"tsc\",\n    \"dev\": \"wrangler dev\"\n  },\n  \"files\": [\"dist\"]\n}\n";

    #[test]
    fn the_reader_rejects_duplicate_keys_and_keeps_numbers() {
        assert_eq!(
            parse_value(r#"{"a": {"b": 1, "b": 2}}"#),
            Err(ValueError::DuplicateKey {
                path: "/a".to_owned(),
                key: "b".to_owned()
            })
        );
        let value = parse_value("{ // c\n \"n\": 1.50, \"m\": 7, }").unwrap();
        assert_eq!(
            value,
            Value::Object(vec![
                ("n".to_owned(), Value::Number("1.5".to_owned())),
                ("m".to_owned(), Value::Number("7".to_owned())),
            ])
        );
        assert!(matches!(parse_value("{"), Err(ValueError::Syntax(_))));
    }

    #[test]
    fn members_are_inserted_after_an_anchor_on_their_own_line() {
        let text = edited(
            PACKAGE,
            &[insert(
                "/scripts",
                "check",
                s("tsc --noEmit"),
                Some("build"),
            )],
        );
        assert_eq!(
            text,
            "{\n  \"name\": \"app\",\n  \"scripts\": {\n    \"build\": \"tsc\",\n    \"check\": \
             \"tsc --noEmit\",\n    \"dev\": \"wrangler dev\"\n  },\n  \"files\": [\"dist\"]\n}\n"
        );
        // After the last member, with no trailing comma in a JSON file.
        let text = edited(
            PACKAGE,
            &[insert("/", "version", s("1.0.0"), Some("files"))],
        );
        assert!(
            text.ends_with("\"files\": [\"dist\"],\n  \"version\": \"1.0.0\"\n}\n"),
            "{text}"
        );
        // First.
        let text = edited(PACKAGE, &[insert("/scripts", "a", s("1"), None)]);
        assert!(
            text.contains("\"scripts\": {\n    \"a\": \"1\",\n    \"build\": \"tsc\","),
            "{text}"
        );
    }

    #[test]
    fn single_line_objects_stay_on_one_line() {
        let text = edited(
            "{ \"a\": 1, \"b\": 2 }",
            &[insert("/", "k", s("v"), Some("a"))],
        );
        assert_eq!(text, "{ \"a\": 1, \"k\": \"v\", \"b\": 2 }");
        let text = edited("{\"a\": 1}", &[insert("/", "k", s("v"), Some("a"))]);
        assert_eq!(text, "{\"a\": 1, \"k\": \"v\"}");
        let text = edited("{\"a\": 1}", &[insert("/", "k", s("v"), None)]);
        assert_eq!(text, "{\"k\": \"v\", \"a\": 1}");
    }

    #[test]
    fn an_empty_object_opens_up_and_nested_values_follow_the_file_s_indentation() {
        let text = edited(
            "{\n  \"engines\": {}\n}\n",
            &[insert("/engines", "node", s(">=22"), None)],
        );
        assert_eq!(
            text,
            "{\n  \"engines\": {\n    \"node\": \">=22\"\n  }\n}\n"
        );
        let text = edited(
            "{\n    \"a\": 1\n}\n",
            &[insert(
                "/",
                "peer",
                Value::Object(vec![
                    ("react".to_owned(), s("^19")),
                    ("tags".to_owned(), Value::Array(vec![s("x")])),
                ]),
                Some("a"),
            )],
        );
        assert_eq!(
            text,
            "{\n    \"a\": 1,\n    \"peer\": {\n        \"react\": \"^19\",\n        \"tags\": [\"x\"]\n    }\n}\n"
        );
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_survive_an_edit() {
        let text = "{\n  // The database bindings.\n  \"d1\": {\n    \"db\": \"main\", // primary\n  },\n}\n";
        let result = edited(text, &[insert("/d1", "audit", s("log"), Some("db"))]);
        assert_eq!(
            result,
            "{\n  // The database bindings.\n  \"d1\": {\n    \"db\": \"main\", // primary\n    \"audit\": \"log\",\n  },\n}\n"
        );
        let result = edited(text, &[insert("/", "name", s("w"), Some("d1"))]);
        assert_eq!(
            result,
            "{\n  // The database bindings.\n  \"d1\": {\n    \"db\": \"main\", // primary\n  },\n  \"name\": \"w\",\n}\n"
        );
    }

    #[test]
    fn values_are_replaced_in_place_and_members_removed_with_their_line() {
        let text = edited(
            PACKAGE,
            &[Edit::Set {
                path: p("/scripts/dev"),
                value: s("wrangler dev --port 8787"),
            }],
        );
        assert!(
            text.contains("\"dev\": \"wrangler dev --port 8787\"\n  },"),
            "{text}"
        );
        let text = edited(
            PACKAGE,
            &[Edit::Set {
                path: p("/files"),
                value: Value::Array(vec![s("dist"), s("README.md")]),
            }],
        );
        assert!(
            text.contains("\"files\": [\"dist\", \"README.md\"]\n}"),
            "{text}"
        );

        // A middle member: its line goes.
        let text = edited(
            PACKAGE,
            &[Edit::Delete {
                path: p("/scripts/build"),
            }],
        );
        assert!(
            text.contains("\"scripts\": {\n    \"dev\": \"wrangler dev\"\n  },"),
            "{text}"
        );
        // The last member: the previous comma goes too.
        let text = edited(PACKAGE, &[Edit::Delete { path: p("/files") }]);
        assert!(
            text.ends_with("\"dev\": \"wrangler dev\"\n  }\n}\n"),
            "{text}"
        );
        // The last member in a file with trailing commas keeps the style.
        let text = edited(
            "{\n  \"a\": 1,\n  \"b\": 2,\n}\n",
            &[Edit::Delete { path: p("/b") }],
        );
        assert_eq!(text, "{\n  \"a\": 1,\n}\n");
        // Single line.
        let text = edited("{ \"a\": 1, \"b\": 2 }", &[Edit::Delete { path: p("/a") }]);
        assert_eq!(text, "{ \"b\": 2 }");
        let text = edited("{ \"a\": 1, \"b\": 2 }", &[Edit::Delete { path: p("/b") }]);
        assert_eq!(text, "{ \"a\": 1 }");
    }

    #[test]
    fn a_comment_between_a_value_and_its_comma_is_stepped_over() {
        let text = "{\n  \"a\": 1 /* one */,\n  \"b\": 2\n}\n";
        let result = edited(text, &[insert("/", "k", s("v"), Some("a"))]);
        assert_eq!(
            result,
            "{\n  \"a\": 1 /* one */,\n  \"k\": \"v\",\n  \"b\": 2\n}\n"
        );
    }

    #[test]
    fn a_file_with_trailing_commas_gets_them_in_what_is_inserted() {
        // As Prettier writes JSONC by default.
        let text = "{\n  \"dependencies\": {},\n  \"options\": {},\n}\n";
        let object = Value::Object(vec![("name".to_owned(), s("a"))]);
        let result = edited(
            text,
            &[
                insert("/dependencies", "@acme/a", s("^1.0.0"), None),
                insert("/options", "@acme/a", object.clone(), None),
            ],
        );
        assert_eq!(
            result,
            "{\n  \"dependencies\": {\n    \"@acme/a\": \"^1.0.0\",\n  },\n  \"options\": {\n    \
             \"@acme/a\": {\n      \"name\": \"a\",\n    },\n  },\n}\n"
        );
        // After a last member that has no comma of its own, in a file that
        // has them elsewhere.
        let text = "{\n  \"a\": [1,],\n  \"b\": 2\n}\n";
        let result = edited(text, &[insert("/", "c", object, Some("b"))]);
        assert_eq!(
            result,
            "{\n  \"a\": [1,],\n  \"b\": 2,\n  \"c\": {\n    \"name\": \"a\",\n  },\n}\n"
        );
        // A file without them gets none.
        let result = edited(PACKAGE, &[insert("/", "o", s("x"), Some("files"))]);
        assert!(
            result.ends_with("\"files\": [\"dist\"],\n  \"o\": \"x\"\n}\n"),
            "{result}"
        );
    }

    #[test]
    fn a_value_inserted_into_a_one_line_object_stays_on_its_line() {
        let text = "{\n  \"params\": { \"a\": { \"type\": \"string\" } }\n}\n";
        let value = Value::Object(vec![
            ("type".to_owned(), s("string")),
            ("allowed".to_owned(), Value::Array(vec![s("x"), s("y")])),
        ]);
        let result = edited(text, &[insert("/params", "b", value, Some("a"))]);
        assert_eq!(
            result,
            "{\n  \"params\": { \"a\": { \"type\": \"string\" }, \"b\": { \"type\": \"string\", \
             \"allowed\": [\"x\", \"y\"] } }\n}\n"
        );
    }

    #[test]
    fn crlf_files_keep_their_line_endings() {
        let text = "{\r\n  \"a\": 1\r\n}\r\n";
        let result = edited(text, &[insert("/", "b", s("2"), Some("a"))]);
        assert_eq!(result, "{\r\n  \"a\": 1,\r\n  \"b\": \"2\"\r\n}\r\n");
        let result = edited(result.as_str(), &[Edit::Delete { path: p("/b") }]);
        assert_eq!(result, text);
    }

    #[test]
    fn an_edit_that_cannot_be_located_is_an_error() {
        let error = apply(PACKAGE, &[insert("/missing", "k", s("v"), None)]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "could not insert \"k\" into /missing: its parent is not an object in the text"
        );
        let error = apply(PACKAGE, &[insert("/scripts", "build", s("v"), None)]).unwrap_err();
        assert!(error.to_string().contains("already exists"));
    }
}
