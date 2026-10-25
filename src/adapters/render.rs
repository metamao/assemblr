//! Template rendering: the `{{ name }}` references in a unit's template
//! files, replaced by the unit's parameter values (see
//! `docs/domain/parameters-and-options.md` and `docs/adr/0009-rendering.md`).
//!
//! Rendering is a pure function of one file's text and its [`Context`]. The
//! grammar is deliberately small:
//!
//! - `{{ name }}` is replaced by the value of the parameter `name`, with any
//!   ASCII whitespace allowed inside the braces. A string value is inserted
//!   as it is, an integer in decimal, and a boolean as `true` or `false`.
//!   Nothing is quoted or escaped for the file's format.
//! - `{{ name | tojson }}` inserts the value as a JSON literal, a string
//!   quoted and escaped, and `{{ name | e }}` with the characters HTML gives
//!   meaning to escaped. Both produce exactly what Jinja's filters of those
//!   names produce for a string or an integer. One filter at most.
//! - `{{ own.name }}` refers to the amending author's own context: the
//!   amending unit's resolved parameter values, defined only when an
//!   amendment copy is rendered. A unit's own files have no such context,
//!   and the reference is an error there.
//! - `{{ "{{" }}` is the one escape: it produces a literal `{{`.
//! - Everything else that starts with `{{`, `{%`, or `{#` is unsupported and
//!   an error, never passed through. A `}}` on its own is ordinary text.
//!
//! An undefined reference is an error, never empty text. Every error in a
//! file is reported, in order of position. The text outside the expressions
//! is copied byte for byte, so line endings and the trailing newline stay as
//! they are. Inserted values are never scanned again, so a value that
//! contains `{{ name }}` renders as that text.

use std::collections::BTreeSet;
use std::fmt;

use super::git::FILE_LIMIT;
use crate::domain::{OWN, ParameterName, ParameterValue, ParameterValues};

/// The largest rendered file, in bytes: ADR 0006's bound on a unit file,
/// which a template's values must not let it exceed.
const RENDERED_LIMIT: usize = FILE_LIMIT as usize;

/// What a template's references resolve against: the resolved parameter
/// values of the unit that provides the file, and, for an amendment copy,
/// the amending unit's own values as `own`.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    values: &'a ParameterValues,
    own: Option<&'a ParameterValues>,
}

impl<'a> Context<'a> {
    /// The context of a unit's own file.
    pub fn new(values: &'a ParameterValues) -> Self {
        Self { values, own: None }
    }

    /// The context of an amendment copy: the provider's `values`, which
    /// its own references see, plus the amending unit's `own` values.
    pub fn amended(values: &'a ParameterValues, own: &'a ParameterValues) -> Self {
        Self {
            values,
            own: Some(own),
        }
    }

    /// The value a reference names, if the context defines it.
    fn resolve(&self, reference: &Reference) -> Option<&'a ParameterValue> {
        match reference {
            Reference::Parameter(name) => self.values.get(name),
            // A unit's own files have no `own` context: that belongs to
            // amendment copies.
            Reference::Own(name) => self.own?.get(name),
        }
    }
}

/// A parsed reference: what `{{ … }}` names.
#[derive(Debug, PartialEq, Eq)]
enum Reference {
    /// A parameter of the unit that provides the file.
    Parameter(ParameterName),
    /// `own.<name>`: a parameter of the amending author's own context.
    Own(ParameterName),
}

/// Renders `source` with `context`. On failure every error is returned, in
/// order of position.
pub fn render(source: &str, context: &Context<'_>) -> Result<String, Vec<RenderError>> {
    let mut out = String::new();
    let mut errors = Vec::new();
    let mut rest = source;
    let mut offset = 0;
    // Where the last construct began: the size bound is attributed to it.
    let mut last_at = 0;
    while let Some((start, opener)) = next_opener(rest) {
        out.push_str(&rest[..start]);
        let at = offset + start;
        last_at = at;
        let after = &rest[start + 2..];
        let Some(length) = after.find(opener.closer()) else {
            errors.push(RenderError::at(
                source,
                at,
                RenderErrorKind::Unterminated {
                    opener: opener.text().to_owned(),
                },
            ));
            // Nothing after an unterminated opener can be placed, and the
            // output is not used once an error is found.
            rest = "";
            break;
        };
        let inner = &after[..length];
        let expression = (opener == Opener::Expression).then(|| parse(inner));
        // An opener inside the construct, other than the escape's own `{{`,
        // means this opener was never closed: the closer found belongs to
        // the next construct, which is scanned on its own.
        if !matches!(expression, Some(Expression::Literal(_))) && next_opener(inner).is_some() {
            errors.push(RenderError::at(
                source,
                at,
                RenderErrorKind::Unterminated {
                    opener: opener.text().to_owned(),
                },
            ));
            rest = &rest[start + 2..];
            offset += start + 2;
            continue;
        }
        let consumed = start + 2 + length + 2;
        let construct = &rest[start..consumed];
        match expression {
            Some(Expression::Literal(text)) => out.push_str(text),
            Some(Expression::Reference(reference, filter)) => match context.resolve(&reference) {
                Some(value) => write_value(&mut out, value, filter),
                None => errors.push(RenderError::at(source, at, undefined(reference, context))),
            },
            Some(Expression::UnknownFilter(filter)) => errors.push(RenderError::at(
                source,
                at,
                RenderErrorKind::UnknownFilter { filter },
            )),
            Some(Expression::Unsupported) | None => errors.push(RenderError::at(
                source,
                at,
                RenderErrorKind::Unsupported {
                    construct: construct.to_owned(),
                },
            )),
        }
        if out.len() > RENDERED_LIMIT {
            errors.push(RenderError::at(
                source,
                at,
                RenderErrorKind::TooLarge {
                    limit: RENDERED_LIMIT,
                },
            ));
            rest = "";
            break;
        }
        rest = &rest[consumed..];
        offset += consumed;
    }
    out.push_str(rest);
    if out.len() > RENDERED_LIMIT
        && !matches!(
            errors.last().map(RenderError::kind),
            Some(RenderErrorKind::TooLarge { .. })
        )
    {
        errors.push(RenderError::at(
            source,
            last_at,
            RenderErrorKind::TooLarge {
                limit: RENDERED_LIMIT,
            },
        ));
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

/// The provider parameters `source` names: the plain `{{ name }}`
/// references, not `own.name`. Constructs that are not references are not
/// listed; rendering reports them.
pub fn parameters_referenced(source: &str) -> BTreeSet<ParameterName> {
    let mut names = BTreeSet::new();
    let mut rest = source;
    while let Some((start, opener)) = next_opener(rest) {
        let after = &rest[start + 2..];
        let Some(length) = after.find(opener.closer()) else {
            break;
        };
        if opener == Opener::Expression {
            if let Expression::Reference(Reference::Parameter(name), _) = parse(&after[..length]) {
                names.insert(name);
            }
        }
        rest = &after[length + 2..];
    }
    names
}

/// The three openers the grammar recognizes. Only expressions have meaning;
/// statements and comments are recognized so that they fail explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opener {
    Expression,
    Statement,
    Comment,
}

impl Opener {
    fn text(self) -> &'static str {
        match self {
            Self::Expression => "{{",
            Self::Statement => "{%",
            Self::Comment => "{#",
        }
    }

    fn closer(self) -> &'static str {
        match self {
            Self::Expression => "}}",
            Self::Statement => "%}",
            Self::Comment => "#}",
        }
    }
}

/// The first opener in `text`, with its byte offset.
fn next_opener(text: &str) -> Option<(usize, Opener)> {
    let bytes = text.as_bytes();
    bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'{')
        .find_map(|(index, _)| {
            let opener = match bytes.get(index + 1)? {
                b'{' => Opener::Expression,
                b'%' => Opener::Statement,
                b'#' => Opener::Comment,
                _ => return None,
            };
            Some((index, opener))
        })
}

/// What the text between `{{` and `}}` means.
#[derive(Debug, PartialEq, Eq)]
enum Expression {
    /// Text to insert as it is.
    Literal(&'static str),
    /// A reference, and the filter its value goes through.
    Reference(Reference, Option<Filter>),
    /// A reference through a filter that does not exist.
    UnknownFilter(String),
    Unsupported,
}

/// How a value is written for the format of the file it goes into, with
/// the output of Jinja's filter of the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    /// A JSON literal, valid in JSON, JavaScript, TypeScript, and YAML.
    ToJson,
    /// Text with `&`, `<`, `>`, `"`, and `'` escaped for HTML and XML.
    Escape,
}

/// The filters, by the names templates spell them.
const FILTERS: [(&str, Filter); 2] = [("tojson", Filter::ToJson), ("e", Filter::Escape)];

/// The one escape: a double-quoted `{{`.
const ESCAPE: &str = "\"{{\"";

fn parse(inner: &str) -> Expression {
    let trimmed = inner.trim_matches(|c: char| c.is_ascii_whitespace());
    if trimmed == ESCAPE {
        return Expression::Literal("{{");
    }
    let Some((reference, filter)) = trimmed.split_once('|') else {
        return match reference(trimmed) {
            Some(reference) => Expression::Reference(reference, None),
            None => Expression::Unsupported,
        };
    };
    let filter = filter.trim_matches(|c: char| c.is_ascii_whitespace());
    let reference = reference.trim_matches(|c: char| c.is_ascii_whitespace());
    // One filter, after a reference; a chain or anything else is not in
    // the grammar.
    let (Some(reference), Ok(name)) = (self::reference(reference), ParameterName::parse(filter))
    else {
        return Expression::Unsupported;
    };
    match FILTERS.iter().find(|(known, _)| *known == name.as_str()) {
        Some((_, filter)) => Expression::Reference(reference, Some(*filter)),
        None => Expression::UnknownFilter(name.as_str().to_owned()),
    }
}

/// `name` or `own.name`, as a reference.
fn reference(trimmed: &str) -> Option<Reference> {
    if let Some(name) = trimmed
        .strip_prefix(OWN)
        .and_then(|rest| rest.strip_prefix('.'))
    {
        return ParameterName::parse(name).ok().map(Reference::Own);
    }
    ParameterName::parse(trimmed).ok().map(Reference::Parameter)
}

fn undefined(reference: Reference, context: &Context<'_>) -> RenderErrorKind {
    match reference {
        Reference::Parameter(name) => RenderErrorKind::Undefined {
            name,
            declared: context.values.keys().cloned().collect(),
        },
        Reference::Own(name) => RenderErrorKind::OwnUndefined {
            name,
            declared: context.own.map(|own| own.keys().cloned().collect()),
        },
    }
}

fn write_value(out: &mut String, value: &ParameterValue, filter: Option<Filter>) {
    let text = match value {
        ParameterValue::String(text) => text.as_str(),
        ParameterValue::Integer(number) => {
            out.push_str(&number.to_string());
            return;
        }
        ParameterValue::Boolean(flag) => {
            out.push_str(if *flag { "true" } else { "false" });
            return;
        }
    };
    match filter {
        None => out.push_str(text),
        Some(Filter::ToJson) => write_json(out, text),
        Some(Filter::Escape) => write_escaped(out, text),
    }
}

/// `text` as Jinja's `tojson` writes a string: Python's `json.dumps` with
/// ASCII output, so every character outside printable ASCII is a `\u`
/// escape in lowercase hex, and then `<`, `>`, `&`, and `'` as `\u`
/// escapes too, so that the literal is safe inside HTML and inside a
/// single-quoted attribute or string.
fn write_json(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '<' | '>' | '&' | '\'' => push_unicode_escape(out, u32::from(c)),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0; 2];
                for unit in c.encode_utf16(&mut units) {
                    push_unicode_escape(out, u32::from(*unit));
                }
            }
        }
    }
    out.push('"');
}

fn push_unicode_escape(out: &mut String, unit: u32) {
    out.push_str(&format!("\\u{unit:04x}"));
}

/// `text` as Jinja's `e` writes it: MarkupSafe's escape of `&`, `<`, `>`,
/// `"`, and `'`.
fn write_escaped(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
}

/// A problem in a template, at the position of the construct that has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderError {
    line: usize,
    column: usize,
    kind: RenderErrorKind,
}

impl RenderError {
    /// The error at byte `offset` of `source`.
    fn at(source: &str, offset: usize, kind: RenderErrorKind) -> Self {
        let before = &source[..offset];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        Self { line, column, kind }
    }

    /// The line of the construct, counting from 1, with `\n` ending a line.
    pub fn line(&self) -> usize {
        self.line
    }

    /// The column of the construct's first character, counting characters
    /// from 1.
    pub fn column(&self) -> usize {
        self.column
    }

    /// What is wrong.
    pub fn kind(&self) -> &RenderErrorKind {
        &self.kind
    }
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line {}, column {}: {}",
            self.line, self.column, self.kind
        )
    }
}

impl std::error::Error for RenderError {}

/// What is wrong with a construct in a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderErrorKind {
    /// `{{ name }}` names a parameter the context does not define.
    Undefined {
        /// The name.
        name: ParameterName,
        /// The parameters the context defines, sorted.
        declared: Vec<ParameterName>,
    },
    /// `{{ own.name }}` names a parameter the `own` context does not
    /// define, or appears in a file that has no `own` context.
    OwnUndefined {
        /// The name after `own.`.
        name: ParameterName,
        /// The parameters `own` defines, sorted, or `None` in a file with
        /// no `own` context.
        declared: Option<Vec<ParameterName>>,
    },
    /// A construct outside the grammar: a statement, a comment, or an
    /// expression that is not a reference or the escape.
    Unsupported {
        /// The construct, from its opener to its closer.
        construct: String,
    },
    /// A reference through a filter that does not exist.
    UnknownFilter {
        /// The filter's name.
        filter: String,
    },
    /// An opener with no closer before the end of the file.
    Unterminated {
        /// The opener.
        opener: String,
    },
    /// The rendered file would be larger than Assemblr accepts for a file.
    TooLarge {
        /// The largest rendered size, in bytes.
        limit: usize,
    },
}

impl fmt::Display for RenderErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Undefined { name, declared } => {
                write!(f, "undefined reference `{name}`")?;
                if declared.is_empty() {
                    f.write_str("; the unit declares no parameters")
                } else {
                    let names: Vec<String> =
                        declared.iter().map(|name| format!("`{name}`")).collect();
                    write!(f, "; the unit's parameters are {}", names.join(", "))
                }
            }
            Self::OwnUndefined {
                name,
                declared: None,
            } => write!(
                f,
                "undefined reference `{OWN}.{name}`; `{OWN}` is defined only when an amendment \
                 copy is rendered, and a unit's own files refer to its parameters by name"
            ),
            Self::OwnUndefined {
                name,
                declared: Some(declared),
            } => {
                write!(f, "undefined reference `{OWN}.{name}`")?;
                if declared.is_empty() {
                    f.write_str("; the amending unit declares no parameters")
                } else {
                    let names: Vec<String> = declared
                        .iter()
                        .map(|name| format!("`{OWN}.{name}`"))
                        .collect();
                    write!(
                        f,
                        "; the amending unit's parameters are {}",
                        names.join(", ")
                    )
                }
            }
            Self::Unsupported { construct } => write!(
                f,
                "unsupported template construct {construct:?}; only `{{{{ name }}}}` references, \
                 optionally through one filter as in `{{{{ name | tojson }}}}`, and the escape \
                 `{{{{ {ESCAPE} }}}}` are supported"
            ),
            Self::UnknownFilter { filter } => write!(
                f,
                "unknown filter `{filter}`; the filters are `tojson`, which writes a JSON \
                 literal, and `e`, which escapes HTML"
            ),
            Self::Unterminated { opener } => {
                write!(f, "`{opener}` is never closed")
            }
            Self::TooLarge { limit } => {
                write!(
                    f,
                    "the rendered file is larger than the {limit} bytes Assemblr accepts"
                )
            }
        }
    }
}

#[cfg(test)]
mod references_tests {
    use super::parameters_referenced;

    #[test]
    fn only_plain_references_are_listed_once_each() {
        let names: Vec<String> = parameters_referenced(
            "{{ port }} {{port}} {{ own.title }} {{ \"{{\" }} {% if x %} {{ name }} {{ unterminated",
        )
        .iter()
        .map(|name| name.as_str().to_owned())
        .collect();
        assert_eq!(names, ["name", "port"]);
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        reason = "a test value that cannot be built fails the test"
    )]

    use super::{Context, RENDERED_LIMIT, RenderError, RenderErrorKind, render};
    use crate::domain::{ParameterName, ParameterValue, ParameterValues};

    fn name(input: &str) -> ParameterName {
        ParameterName::parse(input).unwrap()
    }

    fn values(entries: &[(&str, ParameterValue)]) -> ParameterValues {
        entries
            .iter()
            .map(|(parameter, value)| (name(parameter), value.clone()))
            .collect()
    }

    fn text(value: &str) -> ParameterValue {
        ParameterValue::String(value.to_owned())
    }

    fn sample() -> ParameterValues {
        values(&[
            ("appName", text("demo")),
            ("port", ParameterValue::Integer(8080)),
            ("ssr", ParameterValue::Boolean(true)),
            ("minify", ParameterValue::Boolean(false)),
        ])
    }

    fn ok(source: &str) -> String {
        render(source, &Context::new(&sample())).unwrap()
    }

    fn errors(source: &str) -> Vec<(usize, usize, RenderErrorKind)> {
        render(source, &Context::new(&sample()))
            .unwrap_err()
            .into_iter()
            .map(|error| (error.line(), error.column(), error.kind().clone()))
            .collect()
    }

    fn unsupported(construct: &str) -> RenderErrorKind {
        RenderErrorKind::Unsupported {
            construct: construct.to_owned(),
        }
    }

    #[test]
    fn references_are_replaced_by_their_values_as_text() {
        assert_eq!(
            ok("name: {{ appName }}\nport: {{port}}\nssr: {{\tssr }}\nminify: {{\n  minify\n}}\n"),
            "name: demo\nport: 8080\nssr: true\nminify: false\n"
        );
        assert_eq!(ok("{{appName}}{{appName}}"), "demodemo");
        assert_eq!(ok("no references"), "no references");
        assert_eq!(ok(""), "");
    }

    #[test]
    fn filters_write_what_jinja_s_filters_of_the_same_name_write() {
        // Each expected text is what Jinja 3.1.6 renders for the same value.
        let cases = [
            ("Bob's App", r#""Bob\u0027s App""#, "Bob&#39;s App"),
            (
                r#"say "hi" <b>&</b>"#,
                r#""say \"hi\" \u003cb\u003e\u0026\u003c/b\u003e""#,
                "say &#34;hi&#34; &lt;b&gt;&amp;&lt;/b&gt;",
            ),
            (
                "back\\slash\nnew\ttab\r\u{8}\u{c}\u{1}\u{7f}",
                r#""back\\slash\nnew\ttab\r\b\f\u0001\u007f""#,
                "back\\slash\nnew\ttab\r\u{8}\u{c}\u{1}\u{7f}",
            ),
            (
                "Café 日本 😀",
                r#""Caf\u00e9 \u65e5\u672c \ud83d\ude00""#,
                "Café 日本 😀",
            ),
            ("", r#""""#, ""),
        ];
        for (value, json, escaped) in cases {
            let values = values(&[("title", text(value))]);
            let context = Context::new(&values);
            assert_eq!(render("{{ title | tojson }}", &context).unwrap(), json);
            assert_eq!(render("{{title|e}}", &context).unwrap(), escaped);
        }
        // An integer is its decimal form either way; a boolean renders as
        // it does without a filter.
        assert_eq!(ok("{{ port | tojson }} {{ port | e }}"), "8080 8080");
        assert_eq!(ok("{{ ssr | tojson }} {{ minify | e }}"), "true false");
        // `own` references take filters too.
        let (provider, own) = (sample(), values(&[("title", text("a'b"))]));
        let context = Context::amended(&provider, &own);
        assert_eq!(
            render("{{ own.title | tojson }}", &context).unwrap(),
            r#""a\u0027b""#
        );
    }

    #[test]
    fn a_filter_outside_the_grammar_is_an_error() {
        assert_eq!(
            errors("{{ appName | upper }}"),
            [(
                1,
                1,
                RenderErrorKind::UnknownFilter {
                    filter: "upper".to_owned()
                }
            )]
        );
        assert_eq!(
            errors("{{ appName | upper }}")[0].2.to_string(),
            "unknown filter `upper`; the filters are `tojson`, which writes a JSON literal, and \
             `e`, which escapes HTML"
        );
        for construct in [
            "{{ appName | tojson | e }}",
            "{{ appName | }}",
            "{{ | e }}",
            "{{ appName | e(1) }}",
        ] {
            assert_eq!(errors(construct), [(1, 1, unsupported(construct))]);
        }
        assert!(matches!(
            &errors("{{ missing | e }}")[0].2,
            RenderErrorKind::Undefined { name, .. } if name.as_str() == "missing"
        ));
    }

    #[test]
    fn strings_are_inserted_raw_without_quoting_or_escaping() {
        let values = values(&[
            ("quoted", text(r#"say "hi" and 'bye'"#)),
            ("lines", text("one\ntwo\r\nthree")),
            (
                "braces",
                text("{{ appName }} and {% if %} and {{ \"{{\" }}"),
            ),
            ("markup", text("<b>&</b> $HOME `cmd`")),
        ]);
        let context = Context::new(&values);
        assert_eq!(
            render("a={{ quoted }}", &context).unwrap(),
            r#"a=say "hi" and 'bye'"#
        );
        assert_eq!(
            render("[{{ lines }}]", &context).unwrap(),
            "[one\ntwo\r\nthree]"
        );
        // Inserted text is never scanned again.
        assert_eq!(
            render("{{ braces }}", &context).unwrap(),
            "{{ appName }} and {% if %} and {{ \"{{\" }}"
        );
        assert_eq!(
            render("{{ markup }}", &context).unwrap(),
            "<b>&</b> $HOME `cmd`"
        );
    }

    #[test]
    fn the_escape_produces_a_literal_opener() {
        assert_eq!(ok(r#"{{ "{{" }} appName }}"#), "{{ appName }}");
        assert_eq!(ok(r#"{{"{{"}}"#), "{{");
        assert_eq!(
            ok(r#"${{ "{{" }} matrix.os }} and {{ "{{" }}{{ appName }}"#),
            "${{ matrix.os }} and {{demo"
        );
        // A lone closer is text.
        assert_eq!(ok("a }} b } c { d"), "a }} b } c { d");
    }

    #[test]
    fn text_outside_expressions_is_copied_byte_for_byte() {
        let source = "\u{feff}# {{ appName }}\r\n\ttabs\r\n  no newline at end";
        assert_eq!(
            ok(source),
            "\u{feff}# demo\r\n\ttabs\r\n  no newline at end"
        );
        assert_eq!(ok("trailing\n\n"), "trailing\n\n");
        assert_eq!(ok("ünïcödé {{ port }} ✓"), "ünïcödé 8080 ✓");
    }

    #[test]
    fn an_undefined_reference_is_an_error_that_lists_the_parameters() {
        assert_eq!(
            errors("x: {{ name }}"),
            [(
                1,
                4,
                RenderErrorKind::Undefined {
                    name: name("name"),
                    declared: vec![name("appName"), name("minify"), name("port"), name("ssr")],
                }
            )]
        );
        let message =
            RenderError::at("x: {{ name }}", 3, errors("x: {{ name }}")[0].2.clone()).to_string();
        assert_eq!(
            message,
            "line 1, column 4: undefined reference `name`; the unit's parameters are `appName`, \
             `minify`, `port`, `ssr`"
        );
        let none = ParameterValues::new();
        let error = render("{{ x }}", &Context::new(&none)).unwrap_err();
        assert!(
            error[0].to_string().contains("declares no parameters"),
            "{error:?}"
        );
    }

    #[test]
    fn own_references_parse_but_are_undefined_in_a_unit_s_own_files() {
        assert_eq!(
            errors("{{ own.appName }}"),
            [(
                1,
                1,
                RenderErrorKind::OwnUndefined {
                    name: name("appName"),
                    declared: None,
                }
            )]
        );
        let message = errors("{{ own.appName }}")[0].2.to_string();
        assert!(
            message.contains("`own` is defined only when an amendment copy is rendered"),
            "{message}"
        );
        // `own` alone, or a malformed `own.` reference, is outside the grammar.
        assert_eq!(errors("{{ own }}"), [(1, 1, unsupported("{{ own }}"))]);
        assert_eq!(errors("{{ own. }}"), [(1, 1, unsupported("{{ own. }}"))]);
        assert_eq!(
            errors("{{ own.a.b }}"),
            [(1, 1, unsupported("{{ own.a.b }}"))]
        );
        // Other dotted roots are not references either.
        assert_eq!(
            errors("{{ app.name }}"),
            [(1, 1, unsupported("{{ app.name }}"))]
        );
    }

    #[test]
    fn an_amendment_copy_sees_the_provider_s_values_and_its_own() {
        let own = values(&[
            ("appName", text("amender")),
            ("replicas", ParameterValue::Integer(3)),
        ]);
        let provider = sample();
        let context = Context::amended(&provider, &own);
        assert_eq!(
            render(
                "{{ appName }} on {{ port }} by {{ own.appName }} x{{ own.replicas }}",
                &context
            )
            .unwrap(),
            "demo on 8080 by amender x3"
        );
        // The provider's values are never reached through `own`, and the
        // amending unit's never through a plain reference.
        let errors: Vec<RenderErrorKind> = render("{{ own.port }} {{ replicas }}", &context)
            .unwrap_err()
            .into_iter()
            .map(|error| error.kind().clone())
            .collect();
        assert_eq!(
            errors,
            [
                RenderErrorKind::OwnUndefined {
                    name: name("port"),
                    declared: Some(vec![name("appName"), name("replicas")]),
                },
                RenderErrorKind::Undefined {
                    name: name("replicas"),
                    declared: vec![name("appName"), name("minify"), name("port"), name("ssr")],
                },
            ]
        );
        assert_eq!(
            errors[0].to_string(),
            "undefined reference `own.port`; the amending unit's parameters are `own.appName`, \
             `own.replicas`"
        );
        let none = ParameterValues::new();
        let error = render("{{ own.x }}", &Context::amended(&provider, &none)).unwrap_err();
        assert!(
            error[0]
                .to_string()
                .contains("the amending unit declares no parameters"),
            "{error:?}"
        );
    }

    #[test]
    fn constructs_outside_the_grammar_are_errors_never_passed_through() {
        for construct in [
            "{% if ssr %}",
            "{# a comment #}",
            "{{ port + 1 }}",
            "{{ 'literal' }}",
            "{{ \"other\" }}",
            "{{- appName }}",
            "{{ appName -}}",
            "{{ }}",
            "{{}}",
            "{{ 8080 }}",
            "{{ appName appName }}",
        ] {
            let source = format!("a {construct} b");
            assert_eq!(
                errors(&source),
                [(1, 3, unsupported(construct))],
                "{construct}"
            );
        }
        let message = errors("{% if ssr %}")[0].2.to_string();
        assert_eq!(
            message,
            "unsupported template construct \"{% if ssr %}\"; only `{{ name }}` references, \
             optionally through one filter as in `{{ name | tojson }}`, and the escape \
             `{{ \"{{\" }}` are supported"
        );
    }

    #[test]
    fn an_opener_without_a_closer_is_an_error() {
        // The next opener shows the first was never closed; it is then
        // scanned on its own.
        assert_eq!(
            errors("ok {{ appName }}\nbad {{ appName\nmore {{ x }}"),
            [
                (
                    2,
                    5,
                    RenderErrorKind::Unterminated {
                        opener: "{{".to_owned()
                    }
                ),
                (
                    3,
                    6,
                    RenderErrorKind::Undefined {
                        name: name("x"),
                        declared: vec![name("appName"), name("minify"), name("port"), name("ssr")],
                    }
                ),
            ]
        );
        assert_eq!(
            errors("{{ appName"),
            [(
                1,
                1,
                RenderErrorKind::Unterminated {
                    opener: "{{".to_owned()
                }
            )]
        );
        assert_eq!(
            errors("{% if"),
            [(
                1,
                1,
                RenderErrorKind::Unterminated {
                    opener: "{%".to_owned()
                }
            )]
        );
        assert!(
            errors("{#")[0]
                .2
                .to_string()
                .contains("`{#` is never closed")
        );
    }

    #[test]
    fn every_error_is_reported_in_order_of_position() {
        let source = "{{ a }}\n  {{ port }} {% x %}\n{{ own.b }} {{ c }}";
        let found: Vec<(usize, usize, String)> = render(source, &Context::new(&sample()))
            .unwrap_err()
            .iter()
            .map(|error| {
                let kind = match error.kind() {
                    RenderErrorKind::Undefined { name, .. } => format!("undefined {name}"),
                    RenderErrorKind::OwnUndefined { name, .. } => format!("own {name}"),
                    RenderErrorKind::Unsupported { construct } => {
                        format!("unsupported {construct}")
                    }
                    other => format!("{other:?}"),
                };
                (error.line(), error.column(), kind)
            })
            .collect();
        assert_eq!(
            found,
            [
                (1, 1, "undefined a".to_owned()),
                (2, 14, "unsupported {% x %}".to_owned()),
                (3, 1, "own b".to_owned()),
                (3, 13, "undefined c".to_owned()),
            ]
        );
    }

    #[test]
    fn positions_count_characters_not_bytes() {
        assert_eq!(errors("ünï {{ x }}")[0].1, 5);
        assert_eq!(errors("✓✓\n✓ {{ x }}")[0].0, 2);
        assert_eq!(errors("✓✓\n✓ {{ x }}")[0].1, 3);
        // A carriage return is a character on the line, not a line break.
        assert_eq!(
            errors("a\r\nb {{ x }}")[0..1],
            [(2, 3, errors("a\r\nb {{ x }}")[0].2.clone())]
        );
    }

    #[test]
    fn a_rendered_file_larger_than_the_limit_is_an_error() {
        let big = "x".repeat(RENDERED_LIMIT / 2 + 1);
        let values = values(&[("big", text(&big))]);
        let context = Context::new(&values);
        assert_eq!(
            render("{{ big }}", &context).unwrap().len(),
            RENDERED_LIMIT / 2 + 1
        );
        let errors = render("{{ big }}\n{{ big }}", &context).unwrap_err();
        assert_eq!(
            errors,
            [RenderError {
                line: 2,
                column: 1,
                kind: RenderErrorKind::TooLarge {
                    limit: RENDERED_LIMIT
                }
            }]
        );
        // Text after the last construct counts too, against that construct.
        let trailing = format!("x\n{{{{ big }}}}{}", "y".repeat(RENDERED_LIMIT / 2));
        assert_eq!(
            render(&trailing, &context).unwrap_err(),
            [RenderError {
                line: 2,
                column: 1,
                kind: RenderErrorKind::TooLarge {
                    limit: RENDERED_LIMIT
                }
            }]
        );
    }

    #[test]
    fn rendering_is_deterministic() {
        let source = "{{ appName }} {{ port }} {{ ssr }} {{ \"{{\" }} {{ x }} {% y %}";
        let first = render(source, &Context::new(&sample()));
        for _ in 0..3 {
            assert_eq!(render(source, &Context::new(&sample())), first);
        }
    }
}
