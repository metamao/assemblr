//! Placeholders: literal tokens a unit ships in its files for the consumer
//! to replace with their own values (see
//! `docs/domain/placeholders.md`).
//!
//! Assemblr never substitutes anything. A unit declares the placeholders
//! its files contain, each with a description, and the tokens that only
//! look like placeholders; Assemblr checks those declarations when the unit
//! is built and published, and tells the consumer which declared
//! placeholders remain in the files it manages.
//!
//! A placeholder is `__`, an uppercase ASCII letter, any number of
//! uppercase ASCII letters, digits, or underscores, and `__`, such as
//! `__PROJECT_NAME__`. Two scans find them:
//!
//! - a declared placeholder is found by its exact text, wherever it is,
//!   so `api-__PROJECT_NAME__` and `__PROJECT_NAME___db` hold it;
//! - a token that looks like a placeholder, which the author must declare
//!   or ignore, is a whole word of ASCII letters, digits, and underscores
//!   that has the placeholder form, so `__init__` and `MY__NAME__` are not
//!   tokens.
//!
//! Only UTF-8 text without NUL bytes is scanned. Lines are counted by
//! `\n`, as editors number them, and columns by characters, both from 1.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use super::composition::FileTree;
use super::identity::UnitIdentity;
use super::manifest::{Description, Manifest};
use super::path::RelativePath;
use super::text_merge::is_text;

/// A placeholder's name, which is also its literal text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlaceholderName(String);

impl PlaceholderName {
    /// Accepts `__`, an uppercase ASCII letter, then uppercase ASCII
    /// letters, digits, or underscores, then `__`.
    pub fn parse(input: &str) -> Result<Self, PlaceholderNameError> {
        if is_placeholder(input) {
            Ok(Self(input.to_owned()))
        } else {
            Err(PlaceholderNameError {
                input: input.to_owned(),
                suggestion: suggestion(input),
            })
        }
    }

    /// The placeholder's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for PlaceholderName {
    type Err = PlaceholderNameError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl fmt::Display for PlaceholderName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn is_placeholder(text: &str) -> bool {
    let Some(inner) = text
        .strip_prefix("__")
        .and_then(|rest| rest.strip_suffix("__"))
    else {
        return false;
    };
    let mut chars = inner.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// The placeholder an author most likely meant by `input`: its letters
/// and digits in uppercase, every other character an underscore, between
/// two underscores on each side. `None` when that is no placeholder, as
/// for an input without letters.
fn suggestion(input: &str) -> Option<PlaceholderName> {
    let inner: String = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    let candidate = format!("__{}__", inner.trim_matches('_'));
    is_placeholder(&candidate).then_some(PlaceholderName(candidate))
}

/// Text that is not a placeholder name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderNameError {
    input: String,
    suggestion: Option<PlaceholderName>,
}

impl PlaceholderNameError {
    /// The text as written.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// The placeholder the author most likely meant, if there is one.
    pub fn suggestion(&self) -> Option<&PlaceholderName> {
        self.suggestion.as_ref()
    }
}

impl fmt::Display for PlaceholderNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} is not a placeholder name; ", self.input)?;
        match &self.suggestion {
            Some(suggestion) => write!(f, "write it as \"{suggestion}\": ")?,
            None => f.write_str("a placeholder is ")?,
        }
        f.write_str(
            "two underscores, uppercase letters, digits, or underscores, and two underscores",
        )
    }
}

impl std::error::Error for PlaceholderNameError {}

/// What a unit's manifest declares under `placeholders`: the placeholders
/// the consumer replaces, each with its description, and the tokens that
/// only look like placeholders.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placeholders {
    replace: BTreeMap<PlaceholderName, Description>,
    ignore: BTreeSet<PlaceholderName>,
}

impl Placeholders {
    /// The declarations, or the names listed both to replace and to
    /// ignore, sorted.
    pub fn new(
        replace: BTreeMap<PlaceholderName, Description>,
        ignore: BTreeSet<PlaceholderName>,
    ) -> Result<Self, Vec<PlaceholderName>> {
        let both: Vec<PlaceholderName> = ignore
            .iter()
            .filter(|name| replace.contains_key(*name))
            .cloned()
            .collect();
        if both.is_empty() {
            Ok(Self { replace, ignore })
        } else {
            Err(both)
        }
    }

    /// The placeholders the consumer replaces, with their descriptions.
    pub fn replace(&self) -> &BTreeMap<PlaceholderName, Description> {
        &self.replace
    }

    /// The tokens that look like placeholders and are not.
    pub fn ignore(&self) -> &BTreeSet<PlaceholderName> {
        &self.ignore
    }

    /// Whether the unit declares nothing.
    pub fn is_empty(&self) -> bool {
        self.replace.is_empty() && self.ignore.is_empty()
    }
}

/// One unit's declaration of one placeholder.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlaceholderDeclaration {
    placeholder: PlaceholderName,
    unit: UnitIdentity,
    description: Description,
}

impl PlaceholderDeclaration {
    /// `unit`'s declaration of `placeholder`.
    pub fn new(placeholder: PlaceholderName, unit: UnitIdentity, description: Description) -> Self {
        Self {
            placeholder,
            unit,
            description,
        }
    }

    /// The placeholder.
    pub fn placeholder(&self) -> &PlaceholderName {
        &self.placeholder
    }

    /// The unit that declares it.
    pub fn unit(&self) -> &UnitIdentity {
        &self.unit
    }

    /// What the consumer replaces it with.
    pub fn description(&self) -> &Description {
        &self.description
    }
}

/// The declarations of `manifests`, sorted by placeholder and then unit.
pub fn declarations<'m>(
    manifests: impl IntoIterator<Item = &'m Manifest>,
) -> Vec<PlaceholderDeclaration> {
    let mut declarations: Vec<PlaceholderDeclaration> = manifests
        .into_iter()
        .flat_map(|manifest| {
            manifest
                .placeholders()
                .replace()
                .iter()
                .map(|(placeholder, description)| {
                    PlaceholderDeclaration::new(
                        placeholder.clone(),
                        manifest.name().clone(),
                        description.clone(),
                    )
                })
        })
        .collect();
    declarations.sort();
    declarations.dedup();
    declarations
}

/// Where a placeholder occurs: a file, and the line and column it starts
/// at, both from 1. Ordered by path, line, and column.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
    path: RelativePath,
    line: usize,
    column: usize,
}

impl Location {
    /// The file.
    pub fn path(&self) -> &RelativePath {
        &self.path
    }

    /// The line, counted by `\n` from 1.
    pub fn line(&self) -> usize {
        self.line
    }

    /// The column, in characters from 1.
    pub fn column(&self) -> usize {
        self.column
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.path, self.line)
    }
}

/// The line and column of byte offsets into one text.
struct Lines<'t> {
    text: &'t str,
    /// The byte offset each line starts at.
    starts: Vec<usize>,
}

impl<'t> Lines<'t> {
    fn new(text: &'t str) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        Self { text, starts }
    }

    fn locate(&self, path: &RelativePath, offset: usize) -> Location {
        let line = self.starts.partition_point(|start| *start <= offset);
        let start = self
            .starts
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or(0);
        let column = self
            .text
            .get(start..offset)
            .map_or(0, |prefix| prefix.chars().count());
        Location {
            path: path.clone(),
            line,
            column: column + 1,
        }
    }
}

/// Where `placeholder` occurs in `text`, by its exact text.
fn occurrences(path: &RelativePath, text: &str, placeholder: &PlaceholderName) -> Vec<Location> {
    let lines = Lines::new(text);
    text.match_indices(placeholder.as_str())
        .map(|(at, _)| lines.locate(path, at))
        .collect()
}

/// The words of `text` that have the placeholder form, each where it
/// starts. A word is a maximal run of ASCII letters, digits, and
/// underscores.
fn tokens(path: &RelativePath, text: &str) -> Vec<(PlaceholderName, Location)> {
    let lines = Lines::new(text);
    let mut found = Vec::new();
    let mut start = None;
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    for (at, c) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        match (start, word(c)) {
            (None, true) => start = Some(at),
            (Some(from), false) => {
                start = None;
                if let Some(candidate) = text.get(from..at).filter(|word| is_placeholder(word)) {
                    found.push((
                        PlaceholderName(candidate.to_owned()),
                        lines.locate(path, from),
                    ));
                }
            }
            _ => {}
        }
    }
    found
}

/// The text of `bytes`, when it is text Assemblr scans.
fn scanned(bytes: &[u8]) -> Option<&str> {
    if is_text(bytes) {
        std::str::from_utf8(bytes).ok()
    } else {
        None
    }
}

/// The declared placeholders that remain in some files, by placeholder and
/// then by the unit that declares each meaning.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaceholderReport {
    found: BTreeMap<PlaceholderName, BTreeMap<UnitIdentity, Remaining>>,
}

/// One declaration of a placeholder that remains, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remaining {
    description: Description,
    locations: BTreeSet<Location>,
}

impl Remaining {
    /// The declaration's description.
    pub fn description(&self) -> &Description {
        &self.description
    }

    /// Every occurrence, by path, line, and column.
    pub fn locations(&self) -> &BTreeSet<Location> {
        &self.locations
    }
}

impl PlaceholderReport {
    /// Finds, in each file, the placeholders its declarations name: the
    /// declarations in the file's scope, which are those of the units that
    /// contribute the file and of their resolved graphs. A file that is not
    /// text is not scanned.
    pub fn find<'f>(
        files: impl IntoIterator<Item = (&'f RelativePath, &'f [u8], &'f [PlaceholderDeclaration])>,
    ) -> Self {
        let mut report = Self::default();
        for (path, bytes, declarations) in files {
            if declarations.is_empty() {
                continue;
            }
            let Some(text) = scanned(bytes) else {
                continue;
            };
            for declaration in declarations {
                let locations = occurrences(path, text, &declaration.placeholder);
                if locations.is_empty() {
                    continue;
                }
                report
                    .found
                    .entry(declaration.placeholder.clone())
                    .or_default()
                    .entry(declaration.unit.clone())
                    .or_insert_with(|| Remaining {
                        description: declaration.description.clone(),
                        locations: BTreeSet::new(),
                    })
                    .locations
                    .extend(locations);
            }
        }
        report
    }

    /// Whether no declared placeholder remains.
    pub fn is_empty(&self) -> bool {
        self.found.is_empty()
    }

    /// Each placeholder that remains, by name, with each declaration in
    /// scope where it remains, by unit.
    pub fn placeholders(&self) -> &BTreeMap<PlaceholderName, BTreeMap<UnitIdentity, Remaining>> {
        &self.found
    }

    /// How many files hold at least one placeholder that remains.
    pub fn files(&self) -> usize {
        self.found
            .values()
            .flat_map(BTreeMap::values)
            .flat_map(|remaining| remaining.locations.iter().map(Location::path))
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// How many files hold `placeholder`.
    pub fn files_of(&self, placeholder: &PlaceholderName) -> usize {
        self.found
            .get(placeholder)
            .into_iter()
            .flat_map(BTreeMap::values)
            .flat_map(|remaining| remaining.locations.iter().map(Location::path))
            .collect::<BTreeSet<_>>()
            .len()
    }
}

/// Which list of `placeholders` an entry is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlaceholderList {
    /// `placeholders.replace`.
    Replace,
    /// `placeholders.ignore`.
    Ignore,
}

impl fmt::Display for PlaceholderList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Replace => "placeholders.replace",
            Self::Ignore => "placeholders.ignore",
        })
    }
}

/// A problem with a unit's placeholder declarations that its own files and
/// its resolved graph show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceholderError {
    /// A file path holds a placeholder the consumer would replace, but
    /// Assemblr never renames a file.
    InPath {
        /// The placeholder.
        placeholder: PlaceholderName,
        /// The path, from the unit's top level.
        path: RelativePath,
    },
    /// The unit declares a placeholder that units of its resolved graph
    /// declare already: each placeholder has one meaning in a file.
    Redeclared {
        /// The placeholder.
        placeholder: PlaceholderName,
        /// The units that declare it, sorted.
        units: Vec<UnitIdentity>,
    },
    /// The unit ignores a token that units of its resolved graph declare as
    /// a placeholder, which would hide it from consumers.
    IgnoresDeclared {
        /// The placeholder.
        placeholder: PlaceholderName,
        /// The units that declare it, sorted.
        units: Vec<UnitIdentity>,
    },
    /// No file of the unit holds a declared or ignored entry.
    Unused {
        /// The entry.
        placeholder: PlaceholderName,
        /// The list it is in.
        list: PlaceholderList,
    },
}

impl fmt::Display for PlaceholderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InPath { placeholder, path } => write!(
                f,
                "{path} has the placeholder {placeholder} in its path; Assemblr never renames \
                 files, so a placeholder belongs in a file's content"
            ),
            Self::Redeclared { placeholder, units } => {
                write!(f, "placeholders.replace declares {placeholder}, which ")?;
                write_units(f, units)?;
                f.write_str(if units.len() > 1 {
                    " each declare already"
                } else {
                    " declares already"
                })
            }
            Self::IgnoresDeclared { placeholder, units } => {
                write!(f, "placeholders.ignore lists {placeholder}, which ")?;
                write_units(f, units)?;
                f.write_str(if units.len() > 1 {
                    " declare as a placeholder"
                } else {
                    " declares as a placeholder"
                })
            }
            Self::Unused { placeholder, list } => {
                let verb = match list {
                    PlaceholderList::Replace => "declares",
                    PlaceholderList::Ignore => "lists",
                };
                write!(
                    f,
                    "{list} {verb} {placeholder}, but no file of the unit contains it"
                )
            }
        }
    }
}

impl std::error::Error for PlaceholderError {}

fn write_units(f: &mut fmt::Formatter<'_>, units: &[UnitIdentity]) -> fmt::Result {
    for (index, unit) in units.iter().enumerate() {
        let separator = match (index, units.len() - index) {
            (0, _) => "",
            (1, 1) => " and ",
            (_, 1) => ", and ",
            _ => ", ",
        };
        write!(f, "{separator}{unit}")?;
    }
    Ok(())
}

/// Tokens that look like placeholders that neither the unit nor its
/// resolved graph declares or ignores, by token, each with every place it
/// occurs.
pub type UnresolvedTokens = BTreeMap<PlaceholderName, BTreeSet<Location>>;

/// What checking a unit's placeholders found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaceholderCheck {
    /// Problems with the declarations, which reject a build: placeholders
    /// in paths by path, then redeclarations, ignored declarations, and
    /// unused entries, each by placeholder.
    pub problems: Vec<PlaceholderError>,
    /// The tokens to declare or ignore: a warning for a build, a problem
    /// for a publication.
    pub unresolved: UnresolvedTokens,
}

/// Checks the placeholders of the unit `manifest` describes against its own
/// files, `src` and `amendments`, and the manifests of every unit in its
/// resolved graph, `graph` (the unit itself is skipped there). Paths are
/// reported from the unit's top level, as `src/…` and `amendments/…`.
pub fn check_placeholders<'m>(
    manifest: &Manifest,
    src: &FileTree,
    amendments: &FileTree,
    graph: impl IntoIterator<Item = &'m Manifest>,
) -> PlaceholderCheck {
    let own = manifest.placeholders();
    let graph: Vec<&Manifest> = graph
        .into_iter()
        .filter(|other| other.name() != manifest.name())
        .collect();
    let declared_by = |placeholder: &PlaceholderName| -> Vec<UnitIdentity> {
        let mut units: Vec<UnitIdentity> = graph
            .iter()
            .filter(|other| other.placeholders().replace().contains_key(placeholder))
            .map(|other| other.name().clone())
            .collect();
        units.sort();
        units.dedup();
        units
    };
    let files: Vec<(RelativePath, &[u8])> = [("src", src), ("amendments", amendments)]
        .into_iter()
        .flat_map(|(directory, tree)| {
            tree.iter().filter_map(move |(path, file)| {
                RelativePath::parse(&format!("{directory}/{path}"))
                    .ok()
                    .map(|path| (path, file.contents()))
            })
        })
        .collect();
    let texts: Vec<(&RelativePath, &str)> = files
        .iter()
        .filter_map(|(path, bytes)| scanned(bytes).map(|text| (path, text)))
        .collect();

    let mut check = PlaceholderCheck::default();
    // Paths, against every placeholder a consumer of the unit's files
    // would replace.
    let replaced: BTreeSet<&PlaceholderName> = own
        .replace()
        .keys()
        .chain(
            graph
                .iter()
                .flat_map(|other| other.placeholders().replace().keys()),
        )
        .collect();
    let mut in_paths: Vec<(RelativePath, PlaceholderName)> = files
        .iter()
        .flat_map(|(path, _)| {
            replaced
                .iter()
                .filter(|placeholder| path.as_str().contains(placeholder.as_str()))
                .map(|placeholder| (path.clone(), (*placeholder).clone()))
        })
        .collect();
    in_paths.sort();
    check.problems.extend(
        in_paths
            .into_iter()
            .map(|(path, placeholder)| PlaceholderError::InPath { placeholder, path }),
    );
    for placeholder in own.replace().keys() {
        let units = declared_by(placeholder);
        if !units.is_empty() {
            check.problems.push(PlaceholderError::Redeclared {
                placeholder: placeholder.clone(),
                units,
            });
        }
    }
    for placeholder in own.ignore() {
        let units = declared_by(placeholder);
        if !units.is_empty() {
            check.problems.push(PlaceholderError::IgnoresDeclared {
                placeholder: placeholder.clone(),
                units,
            });
        }
    }
    let used = |placeholder: &PlaceholderName| {
        texts
            .iter()
            .any(|(_, text)| text.contains(placeholder.as_str()))
    };
    let entries = own
        .replace()
        .keys()
        .map(|placeholder| (placeholder, PlaceholderList::Replace))
        .chain(
            own.ignore()
                .iter()
                .map(|placeholder| (placeholder, PlaceholderList::Ignore)),
        );
    let mut unused: Vec<(&PlaceholderName, PlaceholderList)> = entries
        .filter(|(placeholder, _)| !used(placeholder))
        .collect();
    unused.sort();
    check.problems.extend(
        unused
            .into_iter()
            .map(|(placeholder, list)| PlaceholderError::Unused {
                placeholder: placeholder.clone(),
                list,
            }),
    );

    let resolved = |token: &PlaceholderName| {
        std::iter::once(manifest)
            .chain(graph.iter().copied())
            .any(|unit| {
                unit.placeholders().replace().contains_key(token)
                    || unit.placeholders().ignore().contains(token)
            })
    };
    for (path, text) in texts {
        for (token, location) in tokens(path, text) {
            if !resolved(&token) {
                check.unresolved.entry(token).or_default().insert(location);
            }
        }
    }
    check
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]

    use std::collections::{BTreeMap, BTreeSet};

    use super::{
        PlaceholderCheck, PlaceholderDeclaration, PlaceholderError, PlaceholderList,
        PlaceholderName, PlaceholderReport, Placeholders, check_placeholders, occurrences, tokens,
    };
    use crate::domain::{
        Description, FileTree, Manifest, RelativePath, UnitFile, UnitIdentity, Version,
    };

    fn name(input: &str) -> PlaceholderName {
        PlaceholderName::parse(input).unwrap()
    }

    fn path(input: &str) -> RelativePath {
        input.parse().unwrap()
    }

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    fn description(text: &str) -> Description {
        Description::new(text).unwrap()
    }

    fn manifest(unit: &str, replace: &[&str], ignore: &[&str]) -> Manifest {
        let placeholders = Placeholders::new(
            replace
                .iter()
                .map(|placeholder| (name(placeholder), description("A value.")))
                .collect(),
            ignore.iter().map(|placeholder| name(placeholder)).collect(),
        )
        .unwrap();
        Manifest::new(
            identity(unit),
            Version::parse("1.0.0").unwrap(),
            description("A unit."),
            BTreeMap::new(),
            placeholders,
            BTreeMap::new(),
        )
        .unwrap()
    }

    fn tree(files: &[(&str, &str)]) -> FileTree {
        files
            .iter()
            .map(|(file, contents)| {
                (
                    path(file),
                    UnitFile::new(contents.as_bytes().to_vec(), false),
                )
            })
            .collect()
    }

    fn found(text: &str) -> Vec<(String, usize, usize)> {
        tokens(&path("a.txt"), text)
            .into_iter()
            .map(|(token, at)| (token.to_string(), at.line(), at.column()))
            .collect()
    }

    #[test]
    fn a_name_has_the_fixed_form() {
        for valid in ["__A__", "__PROJECT_NAME__", "__V2__", "__A_B__", "__A___"] {
            assert!(PlaceholderName::parse(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "",
            "____",
            "__a__",
            "__2A__",
            "__A_",
            "_A__",
            "__A-B__",
            "PROJECT_NAME",
            "__É__",
            " __A__",
            "___A__",
        ] {
            assert!(PlaceholderName::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn a_malformed_name_suggests_the_normalized_form() {
        let error = PlaceholderName::parse("__project-name__").unwrap_err();
        assert_eq!(error.suggestion(), Some(&name("__PROJECT_NAME__")));
        let message = error.to_string();
        assert!(
            message.contains("write it as \"__PROJECT_NAME__\""),
            "{message}"
        );
        assert_eq!(
            PlaceholderName::parse("{{appTitle}}")
                .unwrap_err()
                .suggestion(),
            Some(&name("__APPTITLE__"))
        );
        assert_eq!(PlaceholderName::parse("--").unwrap_err().suggestion(), None);
        assert_eq!(
            PlaceholderName::parse("__2__").unwrap_err().suggestion(),
            None
        );
    }

    #[test]
    fn a_name_listed_both_to_replace_and_to_ignore_is_refused() {
        let both = Placeholders::new(
            BTreeMap::from([(name("__A__"), description("A."))]),
            BTreeSet::from([name("__A__"), name("__B__")]),
        )
        .unwrap_err();
        assert_eq!(both, [name("__A__")]);
    }

    #[test]
    fn tokens_are_whole_words_with_the_form() {
        assert_eq!(
            found("x = __API_URL__;\n  $__DEV__ __init__ MY__NAME__ __A__B __X___db\n"),
            [
                ("__API_URL__".to_owned(), 1, 5),
                ("__DEV__".to_owned(), 2, 4),
            ]
        );
    }

    #[test]
    fn columns_count_characters_and_lines_count_newlines() {
        assert_eq!(
            found("é __A__\r\n__B__"),
            [("__A__".to_owned(), 1, 3), ("__B__".to_owned(), 2, 1)]
        );
        let at: Vec<(usize, usize)> =
            occurrences(&path("a"), "api-__N__\n__N___db", &name("__N__"))
                .iter()
                .map(|location| (location.line(), location.column()))
                .collect();
        assert_eq!(at, [(1, 5), (2, 1)]);
    }

    fn declaration(placeholder: &str, unit: &str, text: &str) -> PlaceholderDeclaration {
        PlaceholderDeclaration::new(name(placeholder), identity(unit), description(text))
    }

    #[test]
    fn the_report_groups_by_placeholder_then_declaration_in_each_file_s_scope() {
        let postgres = [declaration(
            "__NAME__",
            "@acme/postgres",
            "The database name.",
        )];
        let docs = [declaration("__NAME__", "@acme/docs", "The site's name.")];
        let both = [docs[0].clone(), postgres[0].clone()];
        let (config, site, shared, binary) = (
            path("db/config.json"),
            path("docs/site.json"),
            path("shared.txt"),
            path("logo.png"),
        );
        let files: Vec<(&RelativePath, &[u8], &[PlaceholderDeclaration])> = vec![
            (&site, b"{\"title\": \"__NAME__\"}", &docs),
            (&config, b"{\n  \"db\": \"__NAME__\"\n}", &postgres),
            (&shared, b"__NAME__ __NAME__", &both),
            (&binary, b"__NAME__\0", &both),
        ];
        let report = PlaceholderReport::find(files.clone());
        let mut reversed = files;
        reversed.reverse();
        assert_eq!(report, PlaceholderReport::find(reversed));
        assert_eq!(report.files(), 3);
        assert_eq!(report.files_of(&name("__NAME__")), 3);
        let entries = &report.placeholders()[&name("__NAME__")];
        let units: Vec<String> = entries.keys().map(ToString::to_string).collect();
        assert_eq!(units, ["@acme/docs", "@acme/postgres"]);
        let at: Vec<String> = entries[&identity("@acme/postgres")]
            .locations()
            .iter()
            .map(|location| format!("{location}:{}", location.column()))
            .collect();
        assert_eq!(
            at,
            ["db/config.json:2:10", "shared.txt:1:1", "shared.txt:1:10"]
        );
    }

    #[test]
    fn a_file_without_declarations_in_scope_reports_nothing() {
        let file = path("a.txt");
        let report = PlaceholderReport::find([(&file, &b"__NAME__"[..], &[][..])]);
        assert!(report.is_empty());
    }

    fn check(
        unit: &Manifest,
        src: &[(&str, &str)],
        amendments: &[(&str, &str)],
        graph: &[&Manifest],
    ) -> PlaceholderCheck {
        check_placeholders(unit, &tree(src), &tree(amendments), graph.iter().copied())
    }

    #[test]
    fn declared_and_ignored_tokens_resolve_in_the_unit_or_its_graph() {
        let base = manifest("@acme/base", &["__PROJECT_NAME__"], &["__DEV__"]);
        let unit = manifest("@acme/app", &["__APP_TITLE__"], &["__VERSION__"]);
        let result = check(
            &unit,
            &[("index.html", "<h1>__APP_TITLE__</h1> __VERSION__ __DEV__")],
            &[(
                "package.json",
                "{\"name\": \"__PROJECT_NAME__\", \"x\": \"__API_URL__\"}",
            )],
            &[&base],
        );
        assert_eq!(result.problems, []);
        let unresolved: Vec<(String, Vec<String>)> = result
            .unresolved
            .iter()
            .map(|(token, at)| {
                (
                    token.to_string(),
                    at.iter().map(|location| format!("{location}")).collect(),
                )
            })
            .collect();
        assert_eq!(
            unresolved,
            [(
                "__API_URL__".to_owned(),
                vec!["amendments/package.json:1".to_owned()]
            )]
        );
    }

    #[test]
    fn every_declaration_problem_is_reported_in_order() {
        let base = manifest("@acme/base", &["__NAME__", "__PORT__"], &[]);
        let other = manifest("@acme/other", &["__NAME__"], &[]);
        let unit = manifest(
            "@acme/app",
            &["__NAME__", "__TITLE__", "__UNUSED__"],
            &["__PORT__", "__GONE__"],
        );
        let result = check(
            &unit,
            &[
                ("__NAME__/a.txt", "__NAME__ __TITLE__ __PORT__"),
                ("b/__TITLE__.txt", ""),
            ],
            &[],
            &[&other, &base, &unit],
        );
        assert_eq!(
            result.problems,
            [
                PlaceholderError::InPath {
                    placeholder: name("__NAME__"),
                    path: path("src/__NAME__/a.txt"),
                },
                PlaceholderError::InPath {
                    placeholder: name("__TITLE__"),
                    path: path("src/b/__TITLE__.txt"),
                },
                PlaceholderError::Redeclared {
                    placeholder: name("__NAME__"),
                    units: vec![identity("@acme/base"), identity("@acme/other")],
                },
                PlaceholderError::IgnoresDeclared {
                    placeholder: name("__PORT__"),
                    units: vec![identity("@acme/base")],
                },
                PlaceholderError::Unused {
                    placeholder: name("__GONE__"),
                    list: PlaceholderList::Ignore,
                },
                PlaceholderError::Unused {
                    placeholder: name("__UNUSED__"),
                    list: PlaceholderList::Replace,
                },
            ]
        );
        let messages: Vec<String> = result.problems.iter().map(ToString::to_string).collect();
        assert!(
            messages[2].contains("which @acme/base and @acme/other each declare already"),
            "{}",
            messages[2]
        );
        assert!(
            messages[3].contains("@acme/base declares as a placeholder"),
            "{}",
            messages[3]
        );
        assert!(
            messages[5].contains("placeholders.replace declares __UNUSED__, but no file"),
            "{}",
            messages[5]
        );
    }

    #[test]
    fn a_placeholder_only_in_a_binary_file_is_unused() {
        let unit = manifest("@acme/app", &["__NAME__"], &[]);
        let mut src = tree(&[]);
        src.insert(
            path("logo.png"),
            UnitFile::new(b"__NAME__\0".to_vec(), false),
        );
        let result = check_placeholders(&unit, &src, &tree(&[]), []);
        assert_eq!(
            result.problems,
            [PlaceholderError::Unused {
                placeholder: name("__NAME__"),
                list: PlaceholderList::Replace,
            }]
        );
    }

    #[test]
    fn the_check_does_not_depend_on_the_order_of_files_or_units() {
        let base = manifest("@acme/base", &["__A__"], &[]);
        let side = manifest("@acme/side", &["__B__"], &[]);
        let unit = manifest("@acme/app", &[], &[]);
        let files = [("z.txt", "__C__ __A__"), ("a.txt", "__D__ __B__ __C__")];
        let forward = check(&unit, &files, &[], &[&base, &side]);
        let mut reversed = files;
        reversed.reverse();
        let backward = check(&unit, &reversed, &[], &[&side, &base]);
        assert_eq!(forward, backward);
        let tokens: Vec<String> = forward.unresolved.keys().map(ToString::to_string).collect();
        assert_eq!(tokens, ["__C__", "__D__"]);
    }
}
