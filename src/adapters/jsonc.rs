//! Reading JSONC: JSON with `//` and `/* */` comments and trailing commas.
//!
//! The text is first rewritten into strict JSON by blanking out comments and
//! trailing commas. The rewrite preserves every line break and byte offset,
//! so positions reported by the JSON parser still point into the original
//! text. `serde_json` then parses it, which also bounds nesting depth. Other
//! JSON5 extensions, such as single-quoted strings or unquoted keys, remain
//! syntax errors.

use std::fmt;

use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};

/// A parsed JSON value that keeps everything validation needs to report.
///
/// Objects keep their members in source order, including duplicate keys, so
/// duplicates can be reported instead of silently overwritten. Numbers are
/// either 64-bit signed integers or [`Json::OtherNumber`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Integer(i64),
    /// A fraction, an exponent, or an integer outside the 64-bit signed range.
    OtherNumber,
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

/// Parses JSONC text.
pub(crate) fn parse(text: &str) -> Result<Json, SyntaxError> {
    let json = to_strict_json(text)?;
    serde_json::from_str(&json).map_err(|error| {
        let suffix = format!(" at line {} column {}", error.line(), error.column());
        let message = error.to_string();
        let (line, column) = position(text, offset_of(text, error.line(), error.column()));
        SyntaxError {
            line,
            column,
            message: message.strip_suffix(&suffix).unwrap_or(&message).to_owned(),
        }
    })
}

/// Text that is not valid JSONC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    line: usize,
    column: usize,
    message: String,
}

impl SyntaxError {
    /// The 1-based line of the error.
    pub fn line(&self) -> usize {
        self.line
    }

    /// The 1-based column of the error, counted in characters.
    pub fn column(&self) -> usize {
        self.column
    }

    /// What is wrong at that position.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at line {}, column {}",
            self.message, self.line, self.column
        )
    }
}

impl std::error::Error for SyntaxError {}

/// Blanks out comments and trailing commas, keeping every byte offset.
///
/// A comma is trailing only if it follows a value and the next significant
/// character closes the enclosing array or object. A comma after `[`, `{`,
/// `:`, or another comma is left for the JSON parser to reject.
pub(crate) fn to_strict_json(text: &str) -> Result<String, SyntaxError> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    let mut last_significant: Option<char> = None;
    let mut pending_comma: Option<usize> = None;

    while let Some((start, c)) = chars.next() {
        match c {
            '"' => {
                out.push(c);
                while let Some((_, c)) = chars.next() {
                    out.push(c);
                    match c {
                        '\\' => {
                            if let Some((_, escaped)) = chars.next() {
                                out.push(escaped);
                            }
                        }
                        '"' => break,
                        _ => {}
                    }
                }
                last_significant = Some('"');
                pending_comma = None;
            }
            '/' if matches!(chars.peek(), Some((_, '/'))) => {
                blank(&mut out, c);
                while let Some((_, c)) = chars.next_if(|(_, c)| *c != '\n') {
                    blank(&mut out, c);
                }
            }
            '/' if matches!(chars.peek(), Some((_, '*'))) => {
                blank(&mut out, c);
                if let Some((_, opening_star)) = chars.next() {
                    blank(&mut out, opening_star);
                }
                let mut previous = None;
                let mut closed = false;
                for (_, c) in chars.by_ref() {
                    blank(&mut out, c);
                    if previous == Some('*') && c == '/' {
                        closed = true;
                        break;
                    }
                    previous = Some(c);
                }
                if !closed {
                    let (line, column) = position(text, start);
                    return Err(SyntaxError {
                        line,
                        column,
                        message: "unterminated block comment".to_owned(),
                    });
                }
            }
            ',' => {
                out.push(c);
                pending_comma = match last_significant {
                    Some('[' | '{' | ',' | ':') | None => None,
                    Some(_) => Some(out.len() - 1),
                };
                last_significant = Some(c);
            }
            '}' | ']' => {
                if let Some(comma) = pending_comma.take() {
                    out.replace_range(comma..=comma, " ");
                }
                out.push(c);
                last_significant = Some(c);
            }
            ' ' | '\t' | '\n' | '\r' => out.push(c),
            _ => {
                out.push(c);
                last_significant = Some(c);
                pending_comma = None;
            }
        }
    }
    Ok(out)
}

/// A member of a JSON object, located in the text it was scanned from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Member {
    /// The key, decoded.
    pub(crate) key: String,
    /// The byte offset of the key's opening quote.
    pub(crate) key_start: usize,
    /// The byte offset of the value's first character.
    pub(crate) value_start: usize,
    /// The byte offset just past the value.
    pub(crate) value_end: usize,
}

/// The members of the object that starts at byte `open` of `strict`, which
/// is JSON that [`to_strict_json`] produced and `serde_json` accepted, and
/// the offset of its closing brace.
///
/// Editing a manifest in place needs the spans of its members, which a
/// parsed value does not keep. The text is known to be valid JSON, so this
/// scanner only locates: it returns `None` on anything unexpected rather
/// than guessing, and the caller treats that as "cannot edit safely".
pub(crate) fn members(strict: &str, open: usize) -> Option<(Vec<Member>, usize)> {
    let bytes = strict.as_bytes();
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut members = Vec::new();
    let mut at = skip_whitespace(bytes, open + 1);
    loop {
        match bytes.get(at)? {
            b'}' => return Some((members, at)),
            b'"' => {}
            _ => return None,
        }
        let key_start = at;
        let key_end = skip_string(bytes, at)?;
        let key: String = serde_json::from_str(strict.get(key_start..key_end)?).ok()?;
        at = skip_whitespace(bytes, key_end);
        if bytes.get(at)? != &b':' {
            return None;
        }
        let value_start = skip_whitespace(bytes, at + 1);
        let value_end = skip_value(bytes, value_start)?;
        members.push(Member {
            key,
            key_start,
            value_start,
            value_end,
        });
        at = skip_whitespace(bytes, value_end);
        match bytes.get(at)? {
            b',' => at = skip_whitespace(bytes, at + 1),
            b'}' => return Some((members, at)),
            _ => return None,
        }
    }
}

/// The byte offset of the root object's opening brace.
pub(crate) fn root_object(strict: &str) -> Option<usize> {
    let at = skip_whitespace(strict.as_bytes(), 0);
    (strict.as_bytes().get(at) == Some(&b'{')).then_some(at)
}

fn skip_whitespace(bytes: &[u8], mut at: usize) -> usize {
    while matches!(bytes.get(at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        at += 1;
    }
    at
}

/// The offset just past the string that opens at `at`.
fn skip_string(bytes: &[u8], at: usize) -> Option<usize> {
    if bytes.get(at) != Some(&b'"') {
        return None;
    }
    let mut at = at + 1;
    loop {
        match bytes.get(at)? {
            b'\\' => at += 2,
            b'"' => return Some(at + 1),
            _ => at += 1,
        }
    }
}

/// The offset just past the value that starts at `at`.
fn skip_value(bytes: &[u8], at: usize) -> Option<usize> {
    match bytes.get(at)? {
        b'"' => skip_string(bytes, at),
        open @ (b'{' | b'[') => {
            let close = if *open == b'{' { b'}' } else { b']' };
            let mut at = at + 1;
            loop {
                match bytes.get(at)? {
                    b'"' => at = skip_string(bytes, at)?,
                    b'{' | b'[' => at = skip_value(bytes, at)?,
                    byte if *byte == close => return Some(at + 1),
                    b'}' | b']' => return None,
                    _ => at += 1,
                }
            }
        }
        b',' | b'}' | b']' | b':' => None,
        _ => {
            let mut end = at;
            while bytes.get(end).is_some_and(|byte| {
                !matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
            }) {
                end += 1;
            }
            Some(end)
        }
    }
}

/// Replaces a commented-out character with spaces of the same byte length,
/// keeping line breaks so line numbers are preserved.
fn blank(out: &mut String, c: char) {
    if c == '\n' || c == '\r' {
        out.push(c);
    } else {
        out.extend(std::iter::repeat_n(' ', c.len_utf8()));
    }
}

/// The byte offset of a `serde_json` error position. Its column counts bytes,
/// so the offset is moved back to the start of the character it falls in.
fn offset_of(text: &str, line: usize, column: usize) -> usize {
    let line_start = match line.checked_sub(2) {
        Some(breaks) => text
            .match_indices('\n')
            .nth(breaks)
            .map_or(text.len(), |(index, _)| index + 1),
        None => 0,
    };
    let mut offset = (line_start + column.saturating_sub(1)).min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// The 1-based line and column, in characters, of a byte offset.
fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let column = text[line_start..offset].chars().count() + 1;
    (line, column)
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonVisitor)
    }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Json, E> {
        Ok(Json::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Json, E> {
        Ok(Json::Integer(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Json, E> {
        Ok(i64::try_from(value).map_or(Json::OtherNumber, Json::Integer))
    }

    fn visit_f64<E: de::Error>(self, _value: f64) -> Result<Json, E> {
        Ok(Json::OtherNumber)
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Json, E> {
        Ok(Json::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Json, E> {
        Ok(Json::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut elements = Vec::new();
        while let Some(element) = seq.next_element()? {
            elements.push(element);
        }
        Ok(Json::Array(elements))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut members = Vec::new();
        while let Some(member) = map.next_entry::<String, Json>()? {
            members.push(member);
        }
        Ok(Json::Object(members))
    }
}

#[cfg(test)]
mod tests {
    use super::{Json, members, parse, root_object, to_strict_json};

    fn object(members: &[(&str, Json)]) -> Json {
        Json::Object(
            members
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        )
    }

    #[test]
    fn comments_are_blanked_without_moving_anything() {
        let text = "{ // line\n  \"a\": /* block\n é */ 1 }";
        let json = to_strict_json(text).unwrap();
        assert_eq!(json.len(), text.len());
        assert_eq!(json.lines().count(), text.lines().count());
        assert_eq!(parse(text).unwrap(), object(&[("a", Json::Integer(1))]));
    }

    #[test]
    fn comment_markers_inside_strings_are_content() {
        let text = r#"{"url": "https://example.com/*x*/", "quote": "a\"//b"}"#;
        assert_eq!(
            parse(text).unwrap(),
            object(&[
                ("url", Json::String("https://example.com/*x*/".to_owned())),
                ("quote", Json::String("a\"//b".to_owned())),
            ])
        );
    }

    #[test]
    fn trailing_commas_after_values_are_accepted() {
        for text in [
            "[1,]",
            "[1, ]",
            "{\"a\": 1,}",
            "{\"a\": [1,],}",
            "{\"a\": 1, /* c */ }",
            "{\"a\": 1, // c\n}",
        ] {
            assert!(parse(text).is_ok(), "{text}");
        }
    }

    #[test]
    fn commas_that_do_not_follow_a_value_are_rejected() {
        for text in ["[,]", "{,}", "[1,,]", "{\"a\":,}", "[1,,2]", ","] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn json5_extensions_are_rejected() {
        for text in [
            "{'a': 1}",
            "{a: 1}",
            "[0x10]",
            "[+1]",
            "[.5]",
            "[NaN]",
            "[Infinity]",
            "# comment\n{}",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn syntax_errors_point_into_the_original_text() {
        let error = parse("{\n  // comment\n  \"a\": x\n}").unwrap_err();
        assert_eq!((error.line(), error.column()), (3, 8));
        assert_eq!(error.message(), "expected value");
        assert_eq!(error.to_string(), "expected value at line 3, column 8");
    }

    #[test]
    fn unterminated_block_comments_are_reported_where_they_start() {
        for (text, position) in [("{\n  /* open", (2, 3)), ("/*/", (1, 1))] {
            let error = parse(text).unwrap_err();
            assert_eq!((error.line(), error.column()), position, "{text}");
            assert_eq!(error.message(), "unterminated block comment");
        }
        assert!(parse("/**/ {}").is_ok());
    }

    #[test]
    fn columns_count_characters_on_every_error_path() {
        // `é` and `→` are one column each but two and three bytes.
        for (text, position) in [
            ("{\"é→\": x}", (1, 8)),
            ("[\"é→\", /* open", (1, 8)),
            ("[\"é\", é]", (1, 7)),
            ("{\n  \"é\": [1,\n  →", (3, 3)),
        ] {
            let error = parse(text).unwrap_err();
            assert_eq!((error.line(), error.column()), position, "{text}");
        }
    }

    #[test]
    fn duplicate_keys_are_kept_in_source_order() {
        assert_eq!(
            parse(r#"{"a": 1, "a": 2}"#).unwrap(),
            object(&[("a", Json::Integer(1)), ("a", Json::Integer(2))])
        );
    }

    #[test]
    fn numbers_are_integers_only_within_the_signed_64_bit_range() {
        assert_eq!(
            parse("[-9223372036854775808, 9223372036854775807, 9223372036854775808, 1.0, 1e3]")
                .unwrap(),
            Json::Array(vec![
                Json::Integer(i64::MIN),
                Json::Integer(i64::MAX),
                Json::OtherNumber,
                Json::OtherNumber,
                Json::OtherNumber,
            ])
        );
    }

    #[test]
    fn deep_nesting_is_an_error_not_a_crash() {
        let text = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(parse(&text).is_err());
    }

    /// Each member of the root object as `(key, value text)`, from the
    /// original text.
    fn spans(text: &str) -> Vec<(String, String)> {
        let strict = to_strict_json(text).unwrap();
        let open = root_object(&strict).unwrap();
        let (found, close) = members(&strict, open).unwrap();
        assert_eq!(strict.as_bytes()[close], b'}');
        found
            .into_iter()
            .map(|member| {
                (
                    member.key,
                    text[member.value_start..member.value_end].to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn members_are_located_in_the_original_text() {
        let text = r#"  { // c
          "a": "x\"y}", /* { */ "b\u0041": [1, {"c": "]"}, ],
          "d": { "e": true, },
          "f": -1.5e3,
        }"#;
        assert_eq!(
            spans(text),
            [
                ("a".to_owned(), r#""x\"y}""#.to_owned()),
                ("bA".to_owned(), r#"[1, {"c": "]"}, ]"#.to_owned()),
                ("d".to_owned(), r#"{ "e": true, }"#.to_owned()),
                ("f".to_owned(), "-1.5e3".to_owned()),
            ]
        );
        assert_eq!(spans("{}"), []);
        assert_eq!(spans("{\"a\":1}"), [("a".to_owned(), "1".to_owned())]);
    }

    #[test]
    fn the_scanner_declines_what_it_does_not_expect() {
        assert_eq!(root_object("  [1]"), None);
        assert_eq!(members("{\"a\": }", 0), None);
        assert_eq!(members("{\"a\" 1}", 0), None);
        assert_eq!(members("{a: 1}", 0), None);
        assert_eq!(members("{\"a\": 1", 0), None);
        assert_eq!(members("{\"a\": [1}", 0), None);
        assert_eq!(members("\"a\"", 0), None);
    }
}
