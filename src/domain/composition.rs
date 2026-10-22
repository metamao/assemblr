//! Composition of a resolved graph: the effective parameter values of every
//! unit and the file tree the units' `src/` files compose into.
//!
//! [`compose`] works on plain data: the resolved graph, each resolved unit's
//! manifest and `src/` files, the root, and the values the root supplies. It
//! either returns the composition or every problem that prevents it.
//!
//! # Parameter values
//!
//! A unit's parameter receives requirements from the `options` of the units
//! that depend on it directly and, for an explicit selection, from the root.
//! Each requirement must name a parameter the resolved version declares and
//! satisfy its type. Identical requirements are compatible. Different ones
//! are an options conflict: no requirer takes precedence, whatever its depth
//! or declaration order. A parameter without requirements takes its default.
//! One without a default is required, and the error names who must supply
//! it: the units that depend on it directly, and the root when it is an
//! explicit selection or the unit being built. A unit never configures a
//! transitive dependency, and neither does the root.
//!
//! # Files
//!
//! The composed file tree is the union of the units' `src/` files, each
//! recording the unit that contributed it, keyed by the path the file is
//! **delivered** at. A file whose name ends in [`TEMPLATE_SUFFIX`] is a
//! template: it is delivered at its path without the suffix, and rendered
//! with its unit's parameter values when the composition is written. Every
//! other file is opaque bytes with an executable bit, delivered unchanged.
//! Two files collide when their delivered paths are equal or differ only in
//! case or in Unicode normalization, which some file systems cannot keep
//! apart, or when one file's delivered path is a directory of another's
//! under the same comparison. Case is compared one character at a time, by
//! simple (one-to-one) case mappings, as case-insensitive file systems
//! compare it (see `collision_key`). Every collision is reported, whether
//! the units are independent or one depends on the other, and no unit wins.
//!
//! # Amendments
//!
//! Any member may amend files of its direct dependencies: the unit being
//! built from its working tree, and a resolved unit from its published
//! version. Its manifest declares each amended path with the provider and
//! the exact base version, and the edited copy lives at `amendments/<path>`
//! (see `docs/domain/amendments-and-composition.md`). The copy takes the
//! place of the provider's file, at the provider's delivered path and with
//! the provider's executable bit, and it is a template exactly when the
//! provider's file is. A member's copy is taken from the provider version
//! its declaration names; when the provider resolves to another, the
//! composer applies the copy's changes to the file at the resolved version
//! ([`Amendment::base`] says which version the copy was taken from), except
//! for the unit being built, whose declared base must be the version it is
//! locked to. A declaration whose copy is missing, a copy that is not
//! declared, and a copy of a file the provider does not have at the version
//! resolved are errors. Several members may amend one file of one provider
//! when the composer can combine their copies (JSON and JSONC by value, other
//! text by line): their copies are carried with their bases, and the
//! combining is the composer's business. When it cannot (a structured format
//! without a merge, or content that is not text), none is applied and the
//! members are reported together. Whether a base is the content the registry
//! records is verified by the build, not here.
//!
//! Every problem is reported, in the order [`compose`] documents.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use unicode_normalization::UnicodeNormalization;

use super::identity::UnitIdentity;
use super::manifest::{AmendmentDeclaration, Manifest, OptionValue, OptionValues};
use super::merge::MergeFormat;
use super::parameter::{
    ParameterDefinition, ParameterName, ParameterType, ParameterValue, ParameterValueError,
};
use super::path::{RelativePath, RelativePathError};
use super::resolution::{Requirer, ResolvedGraph};
use super::text_merge::is_text;
use super::version::Version;

/// A unit's `src/` files, by path relative to `src/`.
pub type FileTree = BTreeMap<RelativePath, UnitFile>;

/// The suffix that selects a `src/` file for template rendering. It is
/// matched exactly, case-sensitively, at the end of the file's name, and it
/// is removed from the delivered path: `package.json.assemblr.tmpl` is
/// delivered as `package.json`. A name ending in `.tmpl` alone is an
/// ordinary file.
pub const TEMPLATE_SUFFIX: &str = ".assemblr.tmpl";

/// Where a `src/` file is delivered: `Ok(None)` for an ordinary file, at its
/// own path; `Ok(Some(path))` for a template, at its path without
/// [`TEMPLATE_SUFFIX`]; and an error when removing the suffix leaves a name
/// that is not a valid path, such as a bare `.assemblr.tmpl`, or
/// `notes..assemblr.tmpl`, whose delivered name would end in a dot.
pub fn delivered_path(source: &RelativePath) -> Result<Option<RelativePath>, RelativePathError> {
    match source.as_str().strip_suffix(TEMPLATE_SUFFIX) {
        None => Ok(None),
        Some(delivered) => RelativePath::parse(delivered).map(Some),
    }
}

/// The effective value of each of a unit's parameters.
pub type ParameterValues = BTreeMap<ParameterName, ParameterValue>;

/// One file of a unit: its bytes and whether it is executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitFile {
    contents: Vec<u8>,
    executable: bool,
}

impl UnitFile {
    /// A file with these exact bytes.
    pub fn new(contents: Vec<u8>, executable: bool) -> Self {
        Self {
            contents,
            executable,
        }
    }

    /// The file's bytes, exactly as read.
    pub fn contents(&self) -> &[u8] {
        &self.contents
    }

    /// Whether the file is executable.
    pub fn is_executable(&self) -> bool {
        self.executable
    }
}

/// What composition needs from one unit: its manifest, its `src/` files,
/// and its amendment copies.
#[derive(Debug, Clone, Copy)]
pub struct UnitContent<'a> {
    manifest: &'a Manifest,
    files: &'a FileTree,
    amendments: &'a FileTree,
}

impl<'a> UnitContent<'a> {
    /// The content of the unit `manifest` describes: its `src/` files and
    /// its copies under `amendments/`, each by path relative to that
    /// directory.
    pub fn new(manifest: &'a Manifest, files: &'a FileTree, amendments: &'a FileTree) -> Self {
        Self {
            manifest,
            files,
            amendments,
        }
    }
}

/// What the resolved graph is composed for.
#[derive(Debug, Clone, Copy)]
pub enum Root<'a> {
    /// A consumer project. Its explicit selections are the graph's root
    /// requirements, and its values may configure only them.
    Project,
    /// A unit built from its working tree. Its manifest declares the graph's
    /// root requirements, it is composed with its dependencies, and the
    /// root's values may configure only it.
    Unit(UnitContent<'a>),
}

/// Composes `graph` for `root`.
///
/// `units` holds the content of every resolved unit at its resolved version;
/// entries for identities outside the graph are ignored. `root_values` holds
/// the values the root supplies, by the unit they configure: the project's
/// `options` for a consumer project, or the values a unit build receives for
/// the unit being built. On failure every problem is reported: each cause
/// once, in this order, each kind sorted by unit, parameter or path, and then
/// requirer:
///
/// 1. content that does not match the resolved graph;
/// 2. root values for a unit the root may not configure;
/// 3. requirements naming undeclared parameters or holding invalid values;
/// 4. options conflicts;
/// 5. missing required values;
/// 6. provider files that several members amend, by provider and path;
/// 7. amendments that cannot be applied, and copies that are not declared,
///    by amender and then path;
/// 8. templates whose delivered path is invalid;
/// 9. path collisions.
///
/// A parameter whose requirements are invalid is not also reported as
/// missing or conflicting, and a declaration of a provider file that
/// several members amend is not also checked on its own. An amendment that
/// cannot be applied leaves the provider's file in place, so collisions are
/// still found.
pub fn compose<'a>(
    graph: &ResolvedGraph,
    root: Root<'a>,
    units: &BTreeMap<UnitIdentity, UnitContent<'a>>,
    root_values: &BTreeMap<UnitIdentity, OptionValues>,
) -> Result<Composition<'a>, Vec<CompositionError>> {
    let members = members(graph, root, units)?;
    let mut errors = root_target_errors(graph, root, &members, root_values);
    let (values, parameter_errors) = parameter_values(graph, root, &members, root_values);
    errors.extend(parameter_errors);
    let (files, collisions) = compose_files(&members);
    errors.extend(collisions);
    if errors.is_empty() {
        Ok(Composition { values, files })
    } else {
        Err(errors)
    }
}

/// The result of composing a resolved graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composition<'a> {
    values: BTreeMap<UnitIdentity, ParameterValues>,
    files: BTreeMap<RelativePath, ComposedFile<'a>>,
}

impl<'a> Composition<'a> {
    /// The effective parameter values of every composed unit, including the
    /// unit being built. A unit without parameters has an empty entry.
    pub fn values(&self) -> &BTreeMap<UnitIdentity, ParameterValues> {
        &self.values
    }

    /// The composed file tree, by delivered path.
    pub fn files(&self) -> &BTreeMap<RelativePath, ComposedFile<'a>> {
        &self.files
    }
}

/// A file in the composed tree and the unit that contributed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedFile<'a> {
    unit: &'a UnitIdentity,
    source: &'a RelativePath,
    contents: &'a [u8],
    executable: bool,
    template: bool,
    /// The copies that amend the file, by amender, in requirer order: the
    /// unit being built first, then by identity.
    amendments: Vec<Amendment<'a>>,
}

/// One amender's copy of a provider's file, with the version the copy was
/// taken from. The provider may have resolved to another version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Amendment<'a> {
    amender: &'a UnitIdentity,
    copy: &'a UnitFile,
    base: &'a Version,
}

impl<'a> Amendment<'a> {
    /// The unit that amends the file.
    pub fn amender(&self) -> &'a UnitIdentity {
        self.amender
    }

    /// The amender's copy.
    pub fn copy(&self) -> &'a UnitFile {
        self.copy
    }

    /// The provider's version the copy was taken from: its exact base.
    pub fn base(&self) -> &'a Version {
        self.base
    }
}

impl<'a> ComposedFile<'a> {
    /// The unit whose `src/` provides the file.
    pub fn unit(&self) -> &'a UnitIdentity {
        self.unit
    }

    /// The file's path in that unit's `src/`. For a template it ends in
    /// [`TEMPLATE_SUFFIX`], which the delivered path does not. For an
    /// amended file it is also the copy's path under `amendments/`.
    pub fn source(&self) -> &'a RelativePath {
        self.source
    }

    /// The provider's bytes: the bytes to deliver, or, for a template, the
    /// source to render with the providing unit's parameter values. An
    /// amended file's copies are [`amendments`](Self::amendments).
    pub fn contents(&self) -> &'a [u8] {
        self.contents
    }

    /// Whether the file is executable. An amendment never changes it: the
    /// provider's bit stands.
    pub fn is_executable(&self) -> bool {
        self.executable
    }

    /// Whether the file is a template, to be rendered before delivery.
    pub fn is_template(&self) -> bool {
        self.template
    }

    /// The copies that amend the file, with their amenders, in a fixed
    /// order. One copy takes the file's place; several are combined by the
    /// merge, on rendered content, by the composer. A copy of a template is
    /// rendered with its amender's values as `own`, in addition to the
    /// provider's.
    pub fn amendments(&self) -> &[Amendment<'a>] {
        &self.amendments
    }
}

/// A value required for a parameter and who requires it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionRequirement {
    requirer: Requirer,
    value: ParameterValue,
    /// The requirer's own parameter the value is mapped from, if it is a
    /// mapping.
    from: Option<ParameterName>,
}

impl OptionRequirement {
    /// Who requires the value: the root, or a unit through its `options`.
    pub fn requirer(&self) -> &Requirer {
        &self.requirer
    }

    /// The required value.
    pub fn value(&self) -> &ParameterValue {
        &self.value
    }

    /// The requirer's own parameter the value is mapped from, when the
    /// requirement is a mapping.
    pub fn from(&self) -> Option<&ParameterName> {
        self.from.as_ref()
    }
}

impl fmt::Display for OptionRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} from {}", self.value, self.requirer)?;
        if let Some(source) = &self.from {
            write!(f, " (mapped from its `{source}`)")?;
        }
        Ok(())
    }
}

/// A file a unit contributes, at its path in that unit's `src/`. Ordered by
/// path, then unit.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ContributedPath {
    path: RelativePath,
    unit: UnitIdentity,
    delivered: RelativePath,
}

impl ContributedPath {
    /// The file's path in its unit's `src/`.
    pub fn path(&self) -> &RelativePath {
        &self.path
    }

    /// The unit that contributes it.
    pub fn unit(&self) -> &UnitIdentity {
        &self.unit
    }

    /// The path the file is delivered at: its own path, or, for a template,
    /// that path without [`TEMPLATE_SUFFIX`].
    pub fn delivered(&self) -> &RelativePath {
        &self.delivered
    }
}

impl fmt::Display for ContributedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} from {}", self.path, self.unit)?;
        if self.delivered != self.path {
            write!(f, " (delivered as {})", self.delivered)?;
        }
        Ok(())
    }
}

/// Why the root may not configure a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootTargetReason {
    /// The unit is not part of the composition.
    NotComposed,
    /// The unit is part of the composition only as a dependency of other
    /// units, not as an explicit selection.
    Transitive,
    /// A unit build's values configure only the unit being built, and this
    /// is another unit.
    NotTheBuiltUnit,
}

/// A problem that prevents composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompositionError {
    /// The content supplied for a unit does not match the resolved graph:
    /// it is missing, or its manifest declares another identity, version, or
    /// set of dependencies. This is a defect in the caller, not in the unit.
    ContentMismatch {
        /// The unit whose content does not match.
        unit: UnitIdentity,
        /// The version the graph resolved, or the built unit's own version.
        version: Version,
    },
    /// The root supplies values for a unit it may not configure.
    RootTarget {
        /// The unit the values target.
        unit: UnitIdentity,
        /// Why the root may not configure it.
        reason: RootTargetReason,
    },
    /// A requirement sets a parameter that the unit's resolved version does
    /// not declare.
    UndeclaredParameter {
        /// The unit the requirement targets.
        unit: UnitIdentity,
        /// Its resolved version.
        version: Version,
        /// The parameter it does not declare.
        parameter: ParameterName,
        /// Who sets the parameter.
        requirer: Requirer,
        /// The parameters the unit declares, sorted.
        declared: Vec<ParameterName>,
    },
    /// A requirement's value does not satisfy the parameter's type or
    /// allowed values.
    InvalidValue {
        /// The unit the requirement targets.
        unit: UnitIdentity,
        /// Its resolved version.
        version: Version,
        /// The parameter.
        parameter: ParameterName,
        /// The invalid requirement.
        requirement: OptionRequirement,
        /// Why the value is invalid.
        error: ParameterValueError,
    },
    /// Requirements set different values for the same parameter.
    OptionsConflict {
        /// The unit whose parameter is in conflict.
        unit: UnitIdentity,
        /// Its resolved version.
        version: Version,
        /// The parameter.
        parameter: ParameterName,
        /// Every requirement on the parameter, ordered by requirer.
        requirements: Vec<OptionRequirement>,
    },
    /// A mapping connects parameters that cannot hold the same values.
    MappingIncompatible {
        /// The unit that writes the mapping.
        unit: UnitIdentity,
        /// Its resolved version.
        version: Version,
        /// The unit's own parameter the mapping reads.
        source: ParameterName,
        /// The dependency the mapping configures.
        target: UnitIdentity,
        /// The dependency's parameter.
        parameter: ParameterName,
        /// Why they are incompatible.
        reason: String,
    },
    /// A required parameter receives no value.
    MissingValue {
        /// The unit whose parameter has no value.
        unit: UnitIdentity,
        /// Its resolved version.
        version: Version,
        /// The required parameter.
        parameter: ParameterName,
        /// Who may supply the value: the units that depend on this one
        /// directly, and the root when it may configure this unit.
        responsible: Vec<Requirer>,
        /// The parameter's authoring-time example, which only suggests a
        /// value: it is never used as one.
        example: Option<ParameterValue>,
    },
    /// Several members amend the same file of the same provider, and the
    /// file is not one whose amendments can be combined (JSON, JSONC, and
    /// UTF-8 text are; YAML, TOML, XML, and INI are not, and neither is a
    /// file or copy that is not text). No amender takes precedence, so none
    /// is applied.
    SeveralAmenders {
        /// The amended path.
        path: RelativePath,
        /// The provider.
        unit: UnitIdentity,
        /// Its resolved version.
        version: Version,
        /// The amenders, ordered: the root first, then by identity.
        amenders: Vec<Requirer>,
    },
    /// The unit being built declares an amendment based on a version other
    /// than the one its provider resolved to, which the lock state rules
    /// out for a unit's own amendments. A published amender's base may
    /// differ from the version resolved, and is applied to it.
    AmendmentBase {
        /// The amending member.
        amender: Requirer,
        /// The amended path.
        path: RelativePath,
        /// The provider.
        unit: UnitIdentity,
        /// The declared base version.
        base: Version,
        /// The version the provider resolved to.
        resolved: Version,
    },
    /// The provider's `src/` at the resolved version has no file at the
    /// amended path.
    AmendmentNotProvided {
        /// The amending member.
        amender: Requirer,
        /// The amended path.
        path: RelativePath,
        /// The provider.
        unit: UnitIdentity,
        /// Its resolved version.
        version: Version,
        /// A file the provider does have at that delivered path: the
        /// template the path names without its suffix, or the plain file
        /// the path names with a suffix it does not have.
        similar: Option<RelativePath>,
    },
    /// A declared amendment has no copy under `amendments/`.
    AmendmentMissingCopy {
        /// The amending member.
        amender: Requirer,
        /// The amended path.
        path: RelativePath,
        /// The provider the declaration names.
        provider: UnitIdentity,
    },
    /// A copy under `amendments/` is not declared in the manifest, so its
    /// base is unknown.
    AmendmentUndeclared {
        /// The member holding the copy.
        amender: Requirer,
        /// The copy's path under `amendments/`.
        path: RelativePath,
    },
    /// A template whose path without [`TEMPLATE_SUFFIX`] is not a valid
    /// path, so it cannot be delivered.
    InvalidDeliveredPath {
        /// The template's path in its unit's `src/`.
        path: RelativePath,
        /// The unit that contributes it.
        unit: UnitIdentity,
        /// Why the delivered path is invalid.
        error: RelativePathError,
    },
    /// Files whose delivered paths are equal or differ only in case or in
    /// Unicode normalization.
    PathCollision {
        /// Every colliding file, ordered by path and then unit.
        files: Vec<ContributedPath>,
    },
    /// A file's delivered path is a directory of other files' delivered
    /// paths, compared ignoring case and Unicode normalization.
    NestedPathCollision {
        /// The file whose path the others need as a directory.
        file: ContributedPath,
        /// The files beneath it, ordered by path and then unit.
        nested: Vec<ContributedPath>,
    },
}

impl fmt::Display for CompositionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContentMismatch { unit, version } => write!(
                f,
                "internal error: the content supplied for {unit} {version} does not match \
                 the resolved dependency graph"
            ),
            Self::RootTarget { unit, reason } => match reason {
                RootTargetReason::NotComposed => write!(
                    f,
                    "values are supplied for {unit}, which is not part of the composition; \
                     values may configure only explicitly selected units"
                ),
                RootTargetReason::Transitive => write!(
                    f,
                    "values are supplied for {unit}, which is only a dependency of other \
                     units; values may configure only explicitly selected units, and the units \
                     that depend on {unit} configure it through their `options`"
                ),
                RootTargetReason::NotTheBuiltUnit => write!(
                    f,
                    "values are supplied for {unit}, which is not the unit being built; the \
                     unit configures its dependencies through its `options`"
                ),
            },
            Self::UndeclaredParameter {
                unit,
                version,
                parameter,
                requirer,
                declared,
            } => {
                write!(
                    f,
                    "{requirer} sets parameter `{parameter}` of {unit} {version}, which does \
                     not declare it; "
                )?;
                if declared.is_empty() {
                    f.write_str("it declares no parameters")
                } else {
                    f.write_str("its parameters are ")?;
                    write_list(f, declared.iter().map(|name| format!("`{name}`")))
                }
            }
            Self::InvalidValue {
                unit,
                version,
                parameter,
                requirement,
                error,
            } => write!(
                f,
                "{} sets parameter `{parameter}` of {unit} {version} to {}: {error}",
                requirement.requirer, requirement.value
            ),
            Self::OptionsConflict {
                unit,
                version,
                parameter,
                requirements,
            } => {
                write!(
                    f,
                    "options conflict on parameter `{parameter}` of {unit} {version}: "
                )?;
                write_list(f, requirements)?;
                f.write_str("; no requirement takes precedence, so they must agree")
            }
            Self::MappingIncompatible {
                unit,
                version,
                source,
                target,
                parameter,
                reason,
            } => write!(
                f,
                "{unit} {version} maps its parameter `{source}` to `{parameter}` of {target}, \
                 but {reason}"
            ),
            Self::MissingValue {
                unit,
                version,
                parameter,
                responsible,
                example,
            } => {
                write!(
                    f,
                    "required parameter `{parameter}` of {unit} {version} has no value and no \
                     default; it must be supplied by "
                )?;
                write_list(f, responsible)?;
                if let Some(example) = example {
                    write!(
                        f,
                        " (the unit's example is {example}, which is not used as a value)"
                    )?;
                }
                Ok(())
            }
            Self::SeveralAmenders {
                path,
                unit,
                version,
                amenders,
            } => {
                write!(f, "src/{path} of {unit} {version} is amended by ")?;
                write_list(f, amenders.iter().map(amender_name))?;
                f.write_str(
                    "; several amendments of one file are combined for JSON, JSONC, and UTF-8 \
                     text files, but not for YAML, TOML, XML, or INI files or a file that is not \
                     text, and no amender takes precedence, so the file is not amended",
                )
            }
            Self::AmendmentBase {
                amender,
                path,
                unit,
                base,
                resolved,
            } => match amender {
                Requirer::Root => write!(
                    f,
                    "amendments/{path} is based on {unit} {base}, but {unit} resolved to \
                     {resolved}; an amendment is applied only to its base version"
                ),
                Requirer::Unit(..) => write!(
                    f,
                    "{amender} amends src/{path} of {unit} based on {unit} {base}, but {unit} \
                     resolved to {resolved}"
                ),
            },
            Self::AmendmentNotProvided {
                amender,
                path,
                unit,
                version,
                similar,
            } => {
                match amender {
                    Requirer::Root => write!(
                        f,
                        "amendments/{path} amends a file that {unit} {version} does not \
                         provide: it has no src/{path}"
                    )?,
                    Requirer::Unit(..) => write!(
                        f,
                        "amendments/{path} of {amender} amends a file that {unit} {version} \
                         does not provide: it has no src/{path}"
                    )?,
                }
                match similar {
                    Some(similar) if similar.as_str().ends_with(TEMPLATE_SUFFIX) => write!(
                        f,
                        "; its file is the template src/{similar}, and a copy keeps the \
                         provider's path, suffix included"
                    ),
                    Some(similar) => write!(
                        f,
                        "; its file is src/{similar}, which is not a template, and a copy keeps \
                         the provider's path"
                    ),
                    None => Ok(()),
                }
            }
            Self::AmendmentMissingCopy {
                amender,
                path,
                provider,
            } => match amender {
                Requirer::Root => write!(
                    f,
                    "the manifest declares an amendment of {path} from {provider}, but \
                     amendments/{path} does not exist; restore the copy, or remove the \
                     declaration and run `assemblr unit amend` again"
                ),
                Requirer::Unit(..) => write!(
                    f,
                    "the manifest of {amender} declares an amendment of {path} from {provider}, \
                     but its published version has no amendments/{path}"
                ),
            },
            Self::AmendmentUndeclared { amender, path } => match amender {
                Requirer::Root => write!(
                    f,
                    "amendments/{path} is not declared in the manifest, so its base is unknown; \
                     if an `assemblr unit amend` was interrupted, run it again, and otherwise \
                     declare the copy in `amendments` or delete it"
                ),
                Requirer::Unit(..) => write!(
                    f,
                    "amendments/{path} of {amender} is not declared in its manifest, so its \
                     base is unknown"
                ),
            },
            Self::InvalidDeliveredPath { path, unit, error } => write!(
                f,
                "{path} from {unit} is a template, but without `{TEMPLATE_SUFFIX}` its path is \
                 invalid: {error}"
            ),
            Self::PathCollision { files } => {
                f.write_str("path collision: ")?;
                write_list(f, files)?;
                let first = files.first().map(ContributedPath::delivered);
                let first_unit = files.first().map(ContributedPath::unit);
                let one_unit = files.iter().all(|file| Some(file.unit()) == first_unit);
                if files.iter().any(|file| Some(file.delivered()) != first) {
                    f.write_str(
                        "; these paths differ only in case or in Unicode normalization, which \
                         some file systems cannot keep apart",
                    )
                } else if one_unit {
                    f.write_str("; a unit delivers each path once")
                } else {
                    f.write_str(
                        "; no unit takes precedence, and a unit changes a dependency's file \
                         through an amendment",
                    )
                }
            }
            Self::NestedPathCollision { file, nested } => {
                write!(f, "path collision: {file} is a file, but ")?;
                write_list(f, nested)?;
                f.write_str(" need it to be a directory")
            }
        }
    }
}

impl std::error::Error for CompositionError {}

/// How an amending member is named in diagnostics: the unit being built is
/// "this unit", and a resolved unit is its identity and version.
fn amender_name(amender: &Requirer) -> String {
    match amender {
        Requirer::Root => "this unit".to_owned(),
        Requirer::Unit(..) => amender.to_string(),
    }
}

fn write_list<T: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    items: impl IntoIterator<Item = T>,
) -> fmt::Result {
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{item}")?;
    }
    Ok(())
}

/// A unit taking part in the composition: a resolved unit, or the unit
/// being built.
struct Member<'a> {
    identity: &'a UnitIdentity,
    manifest: &'a Manifest,
    files: &'a FileTree,
    /// The unit's copies under `amendments/`.
    amendments: &'a FileTree,
    /// How the unit appears as the requirer of its `options`.
    requirer: Requirer,
}

/// Every unit taking part in the composition, by identity, after checking
/// that the supplied content matches the graph.
fn members<'a>(
    graph: &ResolvedGraph,
    root: Root<'a>,
    units: &BTreeMap<UnitIdentity, UnitContent<'a>>,
) -> Result<BTreeMap<&'a UnitIdentity, Member<'a>>, Vec<CompositionError>> {
    let mut members = BTreeMap::new();
    let mut errors = Vec::new();
    if let Root::Unit(content) = root {
        let manifest = content.manifest;
        if manifest.dependencies() == graph.root() && !graph.units().contains_key(manifest.name()) {
            members.insert(
                manifest.name(),
                Member {
                    identity: manifest.name(),
                    manifest,
                    files: content.files,
                    amendments: content.amendments,
                    requirer: Requirer::Root,
                },
            );
        } else {
            errors.push(CompositionError::ContentMismatch {
                unit: manifest.name().clone(),
                version: manifest.version().clone(),
            });
        }
    }
    for (identity, resolved) in graph.units() {
        let content = units.get(identity).filter(|content| {
            let manifest = content.manifest;
            manifest.name() == identity
                && manifest.version() == resolved.version()
                && manifest.dependencies() == resolved.dependencies()
        });
        match content {
            Some(content) => {
                let manifest = content.manifest;
                members.insert(
                    manifest.name(),
                    Member {
                        identity: manifest.name(),
                        manifest,
                        files: content.files,
                        amendments: content.amendments,
                        requirer: Requirer::Unit(identity.clone(), resolved.version().clone()),
                    },
                );
            }
            None => errors.push(CompositionError::ContentMismatch {
                unit: identity.clone(),
                version: resolved.version().clone(),
            }),
        }
    }
    if errors.is_empty() {
        Ok(members)
    } else {
        Err(errors)
    }
}

/// Whether the root may configure `unit`, and if not, why.
fn root_target(
    graph: &ResolvedGraph,
    root: Root<'_>,
    members: &BTreeMap<&UnitIdentity, Member<'_>>,
    unit: &UnitIdentity,
) -> Result<(), RootTargetReason> {
    match root {
        Root::Unit(content) if content.manifest.name() == unit => Ok(()),
        Root::Unit(_) => Err(RootTargetReason::NotTheBuiltUnit),
        Root::Project if graph.root().contains_key(unit) => Ok(()),
        Root::Project if members.contains_key(unit) => Err(RootTargetReason::Transitive),
        Root::Project => Err(RootTargetReason::NotComposed),
    }
}

fn root_target_errors(
    graph: &ResolvedGraph,
    root: Root<'_>,
    members: &BTreeMap<&UnitIdentity, Member<'_>>,
    root_values: &BTreeMap<UnitIdentity, OptionValues>,
) -> Vec<CompositionError> {
    root_values
        .keys()
        .filter_map(|unit| {
            root_target(graph, root, members, unit).err().map(|reason| {
                CompositionError::RootTarget {
                    unit: unit.clone(),
                    reason,
                }
            })
        })
        .collect()
}

/// A requirement on one parameter, held while the values are decided.
#[derive(Debug, Clone)]
struct Requested {
    requirer: Requirer,
    value: ParameterValue,
    /// The requirer's own parameter the value is mapped from, if it is.
    from: Option<ParameterName>,
}

/// The members in the order their values can be decided: every unit before
/// the units it depends on, since a unit's value is set by what depends on
/// it. Ties go by identity, so the order is deterministic.
fn dependents_first<'a>(members: &BTreeMap<&'a UnitIdentity, Member<'a>>) -> Vec<&'a UnitIdentity> {
    let mut waiting: BTreeMap<&UnitIdentity, usize> =
        members.keys().map(|unit| (*unit, 0)).collect();
    for member in members.values() {
        for dependency in member.manifest.dependencies().keys() {
            if let Some(count) = waiting.get_mut(dependency) {
                *count += 1;
            }
        }
    }
    let mut ready: BTreeSet<&UnitIdentity> = waiting
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(unit, _)| *unit)
        .collect();
    let mut order = Vec::with_capacity(members.len());
    while let Some(unit) = ready.pop_first() {
        order.push(unit);
        for dependency in members[unit].manifest.dependencies().keys() {
            if let Some(count) = waiting.get_mut(dependency) {
                *count -= 1;
                if *count == 0 {
                    ready.insert(
                        members
                            .get_key_value(dependency)
                            .map_or(dependency, |(key, _)| *key),
                    );
                }
            }
        }
    }
    // A cycle cannot come out of resolution; if one did, the rest are
    // decided in identity order rather than lost.
    for unit in members.keys() {
        if !order.contains(unit) {
            order.push(*unit);
        }
    }
    order
}

/// Why a parameter cannot be mapped to a dependency's parameter, if it
/// cannot: only known incompatibilities, never a coercion.
fn mapping_problem(source: &ParameterDefinition, target: &ParameterDefinition) -> Option<String> {
    match (source.parameter_type(), target.parameter_type()) {
        (ParameterType::Integer, ParameterType::Integer)
        | (ParameterType::Boolean, ParameterType::Boolean)
        | (ParameterType::String | ParameterType::EnumeratedString(_), ParameterType::String) => {
            None
        }
        (ParameterType::EnumeratedString(own), ParameterType::EnumeratedString(accepted)) => {
            let outside: Vec<&String> = own
                .as_slice()
                .iter()
                .filter(|value| !accepted.contains(value))
                .collect();
            if outside.is_empty() {
                None
            } else {
                Some(format!(
                    "it allows {}, which the dependency's parameter does not",
                    outside
                        .iter()
                        .map(|value| format!("{value:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }
        (ParameterType::String, ParameterType::EnumeratedString(_)) => Some(
            "a free string may hold values the dependency's parameter does not allow".to_owned(),
        ),
        (own, accepted) => Some(format!(
            "it is {}, and the dependency's parameter is {}",
            own.value_kind(),
            accepted.value_kind()
        )),
    }
}

/// The effective parameter values of every member, and the errors that
/// prevent some of them, in the documented order.
///
/// Values are decided from the dependents to the dependencies: what the
/// root and a unit's `options` require of a unit is known before its own
/// value is decided, and a mapping carries that decided value on to the
/// units below. No ordering rule settles a conflict.
fn parameter_values(
    graph: &ResolvedGraph,
    root: Root<'_>,
    members: &BTreeMap<&UnitIdentity, Member<'_>>,
    root_values: &BTreeMap<UnitIdentity, OptionValues>,
) -> (
    BTreeMap<UnitIdentity, ParameterValues>,
    Vec<CompositionError>,
) {
    // Every requirement on each parameter of each unit, from the root
    // values the root may supply and, as the units are decided, from their
    // `options`.
    let mut requested: BTreeMap<(UnitIdentity, ParameterName), Vec<Requested>> = BTreeMap::new();
    for (target, values) in root_values {
        if root_target(graph, root, members, target).is_ok() {
            for (parameter, value) in values {
                requested
                    .entry((target.clone(), parameter.clone()))
                    .or_default()
                    .push(Requested {
                        requirer: Requirer::Root,
                        value: value.clone(),
                        from: None,
                    });
            }
        }
    }

    // Each error with the unit and parameter it belongs to, to be reported
    // in that order whatever the order the units were decided in.
    type Keyed = ((UnitIdentity, ParameterName), CompositionError);
    let mut invalid: Vec<Keyed> = Vec::new();
    let mut conflicts: Vec<Keyed> = Vec::new();
    let mut missing: Vec<Keyed> = Vec::new();
    let mut values = BTreeMap::new();
    // Parameters a mapping would set, whose source has no value: the source
    // is the one reported.
    let mut awaiting: BTreeSet<(UnitIdentity, ParameterName)> = BTreeSet::new();
    for identity in dependents_first(members) {
        let member = &members[identity];
        let version = member.manifest.version();
        let mut effective = ParameterValues::new();
        // Requirements on parameters the unit does not declare.
        let declared: Vec<ParameterName> = member.manifest.params().keys().cloned().collect();
        for ((unit, parameter), requirements) in &requested {
            if unit != identity || member.manifest.params().contains_key(parameter) {
                continue;
            }
            let mut requirements = requirements.clone();
            requirements.sort_by(|left, right| left.requirer.cmp(&right.requirer));
            for requirement in requirements {
                invalid.push((
                    (unit.clone(), parameter.clone()),
                    CompositionError::UndeclaredParameter {
                        unit: unit.clone(),
                        version: version.clone(),
                        parameter: parameter.clone(),
                        requirer: requirement.requirer,
                        declared: declared.clone(),
                    },
                ));
            }
        }
        for (parameter, definition) in member.manifest.params() {
            let key = (identity.clone(), parameter.clone());
            let mut requirements = requested.get(&key).cloned().unwrap_or_default();
            requirements.sort_by(|left, right| left.requirer.cmp(&right.requirer));
            let invalid_values: Vec<Keyed> = requirements
                .iter()
                .filter_map(|requirement| {
                    definition.check(&requirement.value).err().map(|error| {
                        CompositionError::InvalidValue {
                            unit: identity.clone(),
                            version: version.clone(),
                            parameter: parameter.clone(),
                            requirement: requirement_of(requirement),
                            error,
                        }
                    })
                })
                .map(|error| (key.clone(), error))
                .collect();
            match requirements.first() {
                _ if !invalid_values.is_empty() => invalid.extend(invalid_values),
                Some(first)
                    if requirements
                        .iter()
                        .all(|requirement| requirement.value == first.value) =>
                {
                    effective.insert(parameter.clone(), first.value.clone());
                }
                Some(_) => conflicts.push((
                    key,
                    CompositionError::OptionsConflict {
                        unit: identity.clone(),
                        version: version.clone(),
                        parameter: parameter.clone(),
                        requirements: requirements.iter().map(requirement_of).collect(),
                    },
                )),
                None => match definition.default() {
                    Some(default) => {
                        effective.insert(parameter.clone(), default.clone());
                    }
                    None if awaiting.contains(&key) => {}
                    None => missing.push((
                        key,
                        CompositionError::MissingValue {
                            unit: identity.clone(),
                            version: version.clone(),
                            parameter: parameter.clone(),
                            responsible: responsible(graph, root, members, identity),
                            example: definition.example().cloned(),
                        },
                    )),
                },
            }
        }

        // What this unit requires of its dependencies, now that its own
        // values are decided.
        for (target, options) in member.manifest.options() {
            let target_member = members.get(target);
            for (parameter, option) in options {
                let key = (target.clone(), parameter.clone());
                let value = match option {
                    OptionValue::Literal(value) => value.clone(),
                    OptionValue::From(source) => {
                        let Some(source_definition) = member.manifest.params().get(source) else {
                            continue;
                        };
                        let Some(target_definition) = target_member
                            .and_then(|target| target.manifest.params().get(parameter))
                        else {
                            // The dependency does not declare it: reported
                            // once, with the dependency's other undeclared
                            // parameters, by the value path below.
                            if let Some(value) = effective.get(source) {
                                requested.entry(key).or_default().push(Requested {
                                    requirer: member.requirer.clone(),
                                    value: value.clone(),
                                    from: Some(source.clone()),
                                });
                            }
                            continue;
                        };
                        if let Some(reason) = mapping_problem(source_definition, target_definition)
                        {
                            invalid.push((
                                key,
                                CompositionError::MappingIncompatible {
                                    unit: identity.clone(),
                                    version: version.clone(),
                                    source: source.clone(),
                                    target: target.clone(),
                                    parameter: parameter.clone(),
                                    reason,
                                },
                            ));
                            continue;
                        }
                        // A source that has no value is reported as such.
                        let Some(value) = effective.get(source) else {
                            awaiting.insert(key);
                            continue;
                        };
                        value.clone()
                    }
                };
                requested.entry(key).or_default().push(Requested {
                    requirer: member.requirer.clone(),
                    value,
                    from: match option {
                        OptionValue::From(source) => Some(source.clone()),
                        OptionValue::Literal(_) => None,
                    },
                });
            }
        }
        values.insert(identity.clone(), effective);
    }

    let mut errors = Vec::new();
    for mut group in [invalid, conflicts, missing] {
        group.sort_by(|left, right| left.0.cmp(&right.0));
        errors.extend(group.into_iter().map(|(_, error)| error));
    }
    (values, errors)
}

fn requirement_of(requested: &Requested) -> OptionRequirement {
    OptionRequirement {
        requirer: requested.requirer.clone(),
        value: requested.value.clone(),
        from: requested.from.clone(),
    }
}

/// Who may supply a value for `unit`'s parameters: the members that depend
/// on it directly, and the root when it may configure the unit.
fn responsible(
    graph: &ResolvedGraph,
    root: Root<'_>,
    members: &BTreeMap<&UnitIdentity, Member<'_>>,
    unit: &UnitIdentity,
) -> Vec<Requirer> {
    let mut responsible: BTreeSet<Requirer> = members
        .values()
        .filter(|member| member.manifest.dependencies().contains_key(unit))
        .map(|member| member.requirer.clone())
        .collect();
    if root_target(graph, root, members, unit).is_ok() {
        responsible.insert(Requirer::Root);
    }
    responsible.into_iter().collect()
}

/// The amendments that apply: for each provider's file, the amending units
/// and their copies, in requirer order.
type Applied<'a> = BTreeMap<
    (&'a UnitIdentity, &'a RelativePath),
    Vec<(Requirer, &'a UnitIdentity, &'a UnitFile, &'a Version)>,
>;

/// The amendments the members declare that can be applied, and every
/// problem with the others, in the documented order.
fn amendments<'a>(
    members: &BTreeMap<&'a UnitIdentity, Member<'a>>,
) -> (Applied<'a>, Vec<CompositionError>) {
    // The members amending each provider file. Members are ordered by
    // identity, and the root is the unit being built, so this order is
    // deterministic too.
    let mut amenders: BTreeMap<(&UnitIdentity, &RelativePath), Vec<&Requirer>> = BTreeMap::new();
    for member in members.values() {
        for (path, declaration) in member.manifest.amendments() {
            amenders
                .entry((declaration.unit(), path))
                .or_default()
                .push(&member.requirer);
        }
    }
    let mut several = Vec::new();
    let mut contested = BTreeSet::new();
    for ((provider, path), requirers) in &amenders {
        let (Some(member), true) = (members.get(provider), requirers.len() > 1) else {
            continue;
        };
        // Several amendments of one file are combined by the composer
        // (OC-10): by value for JSON and JSONC, by line for any other file
        // whose provider file and copies are all text. Anything else could
        // only be combined by letting one amender win, so none applies.
        let delivered = delivered_path(path).ok().flatten();
        let all_text = || {
            member
                .files
                .get(*path)
                .is_none_or(|file| is_text(file.contents()))
                && requirers.iter().all(|requirer| {
                    members
                        .values()
                        .filter(|candidate| candidate.requirer == **requirer)
                        .filter_map(|candidate| candidate.amendments.get(*path))
                        .all(|copy| is_text(copy.contents()))
                })
        };
        let combinable = match MergeFormat::of_path(delivered.as_ref().unwrap_or(path)) {
            MergeFormat::Json => true,
            MergeFormat::Text => all_text(),
            MergeFormat::Unsupported => false,
        };
        if combinable {
            continue;
        }
        let mut requirers: Vec<Requirer> = requirers.iter().map(|r| (*r).clone()).collect();
        requirers.sort();
        several.push(CompositionError::SeveralAmenders {
            path: (*path).clone(),
            unit: (*provider).clone(),
            version: member.manifest.version().clone(),
            amenders: requirers,
        });
        contested.insert((*provider, *path));
    }

    let mut applied = Applied::new();
    let mut problems = Vec::new();
    for member in members.values() {
        let declared = member.manifest.amendments();
        for (path, declaration) in declared {
            if contested.contains(&(declaration.unit(), path)) {
                continue;
            }
            if let Some(problem) = amendment(members, member, path, declaration, &mut applied) {
                problems.push(problem);
            }
        }
        problems.extend(
            member
                .amendments
                .keys()
                .filter(|path| !declared.contains_key(*path))
                .map(|path| CompositionError::AmendmentUndeclared {
                    amender: member.requirer.clone(),
                    path: path.clone(),
                }),
        );
    }
    several.extend(problems);
    for copies in applied.values_mut() {
        copies.sort_by(|a, b| a.0.cmp(&b.0));
    }
    (applied, several)
}

/// Records the amendment of `path` that `member` declares, or why it cannot
/// be applied.
fn amendment<'a>(
    members: &BTreeMap<&'a UnitIdentity, Member<'a>>,
    member: &Member<'a>,
    path: &'a RelativePath,
    declaration: &'a AmendmentDeclaration,
    applied: &mut Applied<'a>,
) -> Option<CompositionError> {
    // Every target is a member: amendments target declared dependencies,
    // which the graph resolved.
    let provider = members.get(declaration.unit())?;
    let resolved = provider.manifest.version();
    // The unit being built amends the version it is locked to, by
    // construction of the lock state; a published amender's base may differ
    // from the version a composition resolves, and its changes are applied
    // to the resolved file (OC-52, OC-53).
    if resolved != declaration.base() && member.requirer == Requirer::Root {
        return Some(CompositionError::AmendmentBase {
            amender: member.requirer.clone(),
            path: path.clone(),
            unit: provider.identity.clone(),
            base: declaration.base().clone(),
            resolved: resolved.clone(),
        });
    }
    if !provider.files.contains_key(path) {
        let with_suffix = RelativePath::parse(&format!("{path}{TEMPLATE_SUFFIX}")).ok();
        let without_suffix = delivered_path(path).ok().flatten();
        let similar = [with_suffix, without_suffix]
            .into_iter()
            .flatten()
            .find(|candidate| provider.files.contains_key(candidate));
        return Some(CompositionError::AmendmentNotProvided {
            amender: member.requirer.clone(),
            path: path.clone(),
            unit: provider.identity.clone(),
            version: resolved.clone(),
            similar,
        });
    }
    let Some(copy) = member.amendments.get(path) else {
        return Some(CompositionError::AmendmentMissingCopy {
            amender: member.requirer.clone(),
            path: path.clone(),
            provider: provider.identity.clone(),
        });
    };
    applied.entry((provider.identity, path)).or_default().push((
        member.requirer.clone(),
        member.identity,
        copy,
        declaration.base(),
    ));
    None
}

/// The composed file tree, or every problem that prevents it.
fn compose_files<'a>(
    members: &BTreeMap<&'a UnitIdentity, Member<'a>>,
) -> (
    BTreeMap<RelativePath, ComposedFile<'a>>,
    Vec<CompositionError>,
) {
    let (applied, mut errors) = amendments(members);
    // Files by the collision key of their delivered path.
    let mut folded: BTreeMap<String, Vec<ContributedPath>> = BTreeMap::new();
    let mut files = BTreeMap::new();
    let mut invalid = Vec::new();
    for member in members.values() {
        for (path, file) in member.files {
            let (delivered, template) = match delivered_path(path) {
                Ok(None) => (path.clone(), false),
                Ok(Some(delivered)) => (delivered, true),
                Err(error) => {
                    invalid.push((path.clone(), member.identity.clone(), error));
                    continue;
                }
            };
            folded
                .entry(collision_key(&delivered))
                .or_default()
                .push(ContributedPath {
                    path: path.clone(),
                    unit: member.identity.clone(),
                    delivered: delivered.clone(),
                });
            let amendments = applied
                .get(&(member.identity, path))
                .map(|copies| {
                    copies
                        .iter()
                        .map(|(_, amender, copy, base)| Amendment {
                            amender,
                            copy,
                            base,
                        })
                        .collect()
                })
                .unwrap_or_default();
            files.insert(
                delivered,
                ComposedFile {
                    unit: member.identity,
                    source: path,
                    contents: file.contents(),
                    executable: file.is_executable(),
                    template,
                    amendments,
                },
            );
        }
    }

    invalid.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    let mut collisions: Vec<CompositionError> = invalid
        .into_iter()
        .map(|(path, unit, error)| CompositionError::InvalidDeliveredPath { path, unit, error })
        .collect();
    let groups: Vec<CompositionError> = folded
        .values()
        .filter(|group| group.len() > 1)
        .map(|group| {
            let mut files = group.clone();
            files.sort();
            CompositionError::PathCollision { files }
        })
        .collect();
    collisions.extend(groups);

    let mut nested: BTreeMap<&ContributedPath, Vec<ContributedPath>> = BTreeMap::new();
    for (key, group) in &folded {
        let directories = key.match_indices('/').map(|(index, _)| &key[..index]);
        for directory in directories {
            for file in folded.get(directory).into_iter().flatten() {
                nested
                    .entry(file)
                    .or_default()
                    .extend(group.iter().cloned());
            }
        }
    }
    collisions.extend(nested.into_iter().map(|(file, mut beneath)| {
        beneath.sort();
        CompositionError::NestedPathCollision {
            file: file.clone(),
            nested: beneath,
        }
    }));

    errors.extend(collisions);
    (files, errors)
}

/// The key under which paths collide: `NFD(case(NFD(path)))`, where `case`
/// maps each character to its lowercase and then to its uppercase, each only
/// when that mapping is a single character.
///
/// Two paths with the same key differ only in canonically equivalent
/// spellings, such as a precomposed `é` and an `e` followed by a combining
/// acute accent, or only in case. This is not Unicode's canonical caseless
/// matching, which uses full case folding: a mapping to several characters,
/// such as `ß` to `SS` or `ﬁ` to `FI`, is not applied, because file systems
/// compare names by one-to-one mappings and keep `ß` and `ss` apart. Mapping
/// to lowercase and then uppercase also equates the characters that NTFS
/// equates through their uppercase, such as `ς` and `σ`, `µ` and `μ`, `ſ` and
/// `s`, or `ı` and `i`, which lowercasing alone keeps apart. Compatibility
/// equivalents stay distinct, as file systems keep them apart. Each character
/// is mapped on its own, never by context, and none maps to `/`, so a key's
/// directories are the keys of the path's directories.
fn collision_key(path: &RelativePath) -> String {
    let cased: String = path.as_str().nfd().map(simple_case).collect();
    cased.nfd().collect()
}

/// `c`'s uppercase after its lowercase, each only when it is one character.
fn simple_case(c: char) -> char {
    let lower = single(c.to_lowercase()).unwrap_or(c);
    single(lower.to_uppercase()).unwrap_or(lower)
}

/// The only item of `chars`, if it has exactly one.
fn single(mut chars: impl Iterator<Item = char>) -> Option<char> {
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        CompositionError, ContributedPath, FileTree, OptionRequirement, Root, RootTargetReason,
        TEMPLATE_SUFFIX, UnitContent, UnitFile, compose, delivered_path,
    };
    use crate::domain::{
        AllowedValues, AmendmentDeclaration, AvailableVersions, Dependencies, Description, Lock,
        Manifest, OptionValue, OptionValues, ParameterDefinition, ParameterName, ParameterType,
        ParameterValue, ParameterValueError, RelativePath, Requirer, UnitIdentity,
        UnitOptionValues, ValueKind, Version, resolve,
    };

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    fn version(input: &str) -> Version {
        input.parse().unwrap()
    }

    fn name(input: &str) -> ParameterName {
        input.parse().unwrap()
    }

    fn path(input: &str) -> RelativePath {
        input.parse().unwrap()
    }

    fn text(value: &str) -> ParameterValue {
        ParameterValue::String(value.to_owned())
    }

    fn unit(name: &str, resolved: &str) -> Requirer {
        Requirer::Unit(identity(name), version(resolved))
    }

    fn required(parameter_type: ParameterType) -> ParameterDefinition {
        ParameterDefinition::new(
            parameter_type,
            Description::new("A parameter.").unwrap(),
            None,
        )
        .unwrap()
    }

    fn optional(parameter_type: ParameterType, default: ParameterValue) -> ParameterDefinition {
        ParameterDefinition::new(
            parameter_type,
            Description::new("A parameter.").unwrap(),
            Some(default),
        )
        .unwrap()
    }

    fn enumerated(values: &[&str]) -> ParameterType {
        ParameterType::EnumeratedString(
            AllowedValues::new(values.iter().map(|value| (*value).to_owned()).collect()).unwrap(),
        )
    }

    /// A unit for a test: its manifest fields and its `src/` files.
    #[derive(Clone)]
    struct Spec {
        name: &'static str,
        version: &'static str,
        dependencies: Vec<(&'static str, &'static str)>,
        options: Vec<(&'static str, &'static str, ParameterValue)>,
        /// Mapped options: dependency, its parameter, and the unit's own.
        mappings: Vec<(&'static str, &'static str, &'static str)>,
        params: Vec<(&'static str, ParameterDefinition)>,
        files: Vec<(&'static str, Vec<u8>, bool)>,
        /// Declared amendments: path, provider, and base version.
        amendments: Vec<(&'static str, &'static str, &'static str)>,
        /// Copies under `amendments/`: path and contents.
        copies: Vec<(&'static str, Vec<u8>)>,
    }

    impl Spec {
        fn new(name: &'static str, version: &'static str) -> Self {
            Self {
                name,
                version,
                dependencies: Vec::new(),
                options: Vec::new(),
                mappings: Vec::new(),
                params: Vec::new(),
                files: Vec::new(),
                amendments: Vec::new(),
                copies: Vec::new(),
            }
        }

        /// Declares an amendment of `file` from `provider` at `base`.
        fn amends(
            mut self,
            file: &'static str,
            provider: &'static str,
            base: &'static str,
        ) -> Self {
            self.amendments.push((file, provider, base));
            self
        }

        /// Adds a copy under `amendments/`.
        fn copy(mut self, file: &'static str, contents: &str) -> Self {
            self.copies.push((file, contents.as_bytes().to_vec()));
            self
        }

        /// Adds a copy under `amendments/` that is not necessarily text.
        fn copy_bytes(mut self, file: &'static str, contents: &[u8]) -> Self {
            self.copies.push((file, contents.to_vec()));
            self
        }

        fn depends(mut self, target: &'static str, range: &'static str) -> Self {
            self.dependencies.push((target, range));
            self
        }

        /// Maps the unit's own parameter `source` to `parameter` of `target`.
        fn map(
            mut self,
            target: &'static str,
            parameter: &'static str,
            source: &'static str,
        ) -> Self {
            self.mappings.push((target, parameter, source));
            self
        }

        fn option(
            mut self,
            target: &'static str,
            parameter: &'static str,
            value: ParameterValue,
        ) -> Self {
            self.options.push((target, parameter, value));
            self
        }

        fn param(mut self, parameter: &'static str, definition: ParameterDefinition) -> Self {
            self.params.push((parameter, definition));
            self
        }

        /// Adds a file whose contents name the unit and the path.
        fn file(self, file: &'static str) -> Self {
            let contents = format!("{} {file}", self.name).into_bytes();
            self.file_with(file, contents, false)
        }

        fn file_with(mut self, file: &'static str, contents: Vec<u8>, executable: bool) -> Self {
            self.files.push((file, contents, executable));
            self
        }

        fn manifest(&self) -> Manifest {
            let mut options: BTreeMap<UnitIdentity, UnitOptionValues> = BTreeMap::new();
            for (target, parameter, value) in &self.options {
                options
                    .entry(identity(target))
                    .or_default()
                    .insert(name(parameter), OptionValue::Literal(value.clone()));
            }
            for (target, parameter, source) in &self.mappings {
                options
                    .entry(identity(target))
                    .or_default()
                    .insert(name(parameter), OptionValue::From(name(source)));
            }
            Manifest::new(
                identity(self.name),
                version(self.version),
                Description::new("A unit.").unwrap(),
                dependencies(&self.dependencies),
                options,
                self.params
                    .iter()
                    .map(|(parameter, definition)| (name(parameter), definition.clone()))
                    .collect(),
                self.amendments
                    .iter()
                    .map(|(file, provider, base)| {
                        (
                            path(file),
                            AmendmentDeclaration::new(identity(provider), version(base)),
                        )
                    })
                    .collect(),
            )
            .unwrap()
        }

        fn tree(&self) -> FileTree {
            self.files
                .iter()
                .map(|(file, contents, executable)| {
                    (path(file), UnitFile::new(contents.clone(), *executable))
                })
                .collect()
        }

        fn copies(&self) -> FileTree {
            self.copies
                .iter()
                .map(|(file, contents)| (path(file), UnitFile::new(contents.clone(), false)))
                .collect()
        }
    }

    fn dependencies(entries: &[(&str, &str)]) -> Dependencies {
        entries
            .iter()
            .map(|(target, range)| (identity(target), range.parse().unwrap()))
            .collect()
    }

    /// What a test composes for: a consumer project with its explicit
    /// selections, or a unit being built.
    enum For {
        Project(Vec<(&'static str, &'static str)>),
        Build(Spec),
    }

    /// A composition with owned, comparable parts.
    #[derive(Debug, PartialEq, Eq)]
    struct Composed {
        values: Vec<(String, Vec<(String, ParameterValue)>)>,
        /// Delivered path, unit, contents, and executable bit.
        files: Vec<(String, String, Vec<u8>, bool)>,
        /// The templates: delivered path and source path.
        templates: Vec<(String, String)>,
        /// The amended files: delivered path and amending unit.
        amended: Vec<(String, String)>,
        /// The base version of each amendment: delivered path and version.
        bases: Vec<(String, String)>,
    }

    type RootValue = (&'static str, &'static str, ParameterValue);

    /// Resolves `published` for `root` and composes the result.
    fn run(
        root: &For,
        published: &[Spec],
        root_values: &[RootValue],
    ) -> Result<Composed, Vec<CompositionError>> {
        let contents: Vec<(Manifest, FileTree, FileTree)> = published
            .iter()
            .map(|spec| (spec.manifest(), spec.tree(), spec.copies()))
            .collect();
        let mut available = AvailableVersions::new();
        for (manifest, _, _) in &contents {
            available
                .entry(manifest.name().clone())
                .or_default()
                .insert(manifest.version().clone(), manifest.dependencies().clone());
        }
        let built = match root {
            For::Build(spec) => Some((spec.manifest(), spec.tree(), spec.copies())),
            For::Project(_) => None,
        };
        let requirements = match (root, &built) {
            (_, Some((manifest, _, _))) => manifest.dependencies().clone(),
            (For::Project(selections), None) => dependencies(selections),
            (For::Build(_), None) => unreachable!(),
        };
        let resolution = resolve(
            built.as_ref().map(|(manifest, _, _)| manifest.name()),
            &requirements,
            &available,
            &Lock::new(),
        )
        .unwrap();
        let units: BTreeMap<UnitIdentity, UnitContent<'_>> = contents
            .iter()
            .filter(|(manifest, _, _)| {
                resolution
                    .graph()
                    .units()
                    .get(manifest.name())
                    .is_some_and(|resolved| resolved.version() == manifest.version())
            })
            .map(|(manifest, files, copies)| {
                (
                    manifest.name().clone(),
                    UnitContent::new(manifest, files, copies),
                )
            })
            .collect();
        let root = match &built {
            Some((manifest, files, copies)) => {
                Root::Unit(UnitContent::new(manifest, files, copies))
            }
            None => Root::Project,
        };
        let mut values: BTreeMap<UnitIdentity, OptionValues> = BTreeMap::new();
        for (target, parameter, value) in root_values {
            values
                .entry(identity(target))
                .or_default()
                .insert(name(parameter), value.clone());
        }
        compose(resolution.graph(), root, &units, &values).map(|composition| Composed {
            values: composition
                .values()
                .iter()
                .map(|(unit, values)| {
                    (
                        unit.to_string(),
                        values
                            .iter()
                            .map(|(parameter, value)| (parameter.to_string(), value.clone()))
                            .collect(),
                    )
                })
                .collect(),
            files: composition
                .files()
                .iter()
                .map(|(file, composed)| {
                    // One copy takes the file's place, as the build
                    // delivers it; several are the merge's business.
                    let contents = match composed.amendments() {
                        [only] => only.copy().contents(),
                        _ => composed.contents(),
                    };
                    (
                        file.to_string(),
                        composed.unit().to_string(),
                        contents.to_vec(),
                        composed.is_executable(),
                    )
                })
                .collect(),
            templates: composition
                .files()
                .iter()
                .filter(|(_, composed)| composed.is_template())
                .map(|(file, composed)| (file.to_string(), composed.source().to_string()))
                .collect(),
            amended: composition
                .files()
                .iter()
                .flat_map(|(file, composed)| {
                    composed
                        .amendments()
                        .iter()
                        .map(|amendment| (file.to_string(), amendment.amender().to_string()))
                })
                .collect(),
            bases: composition
                .files()
                .iter()
                .flat_map(|(file, composed)| {
                    composed
                        .amendments()
                        .iter()
                        .map(|amendment| (file.to_string(), amendment.base().to_string()))
                })
                .collect(),
        })
    }

    fn values_of(composed: &Composed, target: &str) -> Vec<(String, ParameterValue)> {
        composed
            .values
            .iter()
            .find(|(unit, _)| unit == target)
            .map(|(_, values)| values.clone())
            .unwrap()
    }

    fn values(entries: &[(&str, ParameterValue)]) -> Vec<(String, ParameterValue)> {
        entries
            .iter()
            .map(|(parameter, value)| ((*parameter).to_owned(), value.clone()))
            .collect()
    }

    fn requirement(requirer: Requirer, value: ParameterValue) -> OptionRequirement {
        OptionRequirement {
            requirer,
            value,
            from: None,
        }
    }

    fn contributed(file: &str, contributor: &str) -> ContributedPath {
        ContributedPath {
            path: path(file),
            unit: identity(contributor),
            delivered: path(file),
        }
    }

    /// A template, delivered at its path without the suffix.
    fn templated(file: &str, contributor: &str) -> ContributedPath {
        ContributedPath {
            path: path(file),
            unit: identity(contributor),
            delivered: path(file.strip_suffix(TEMPLATE_SUFFIX).unwrap()),
        }
    }

    fn project(selections: &[(&'static str, &'static str)]) -> For {
        For::Project(selections.to_vec())
    }

    fn node() -> Spec {
        Spec::new("@metamao/node", "1.0.0")
            .param(
                "packageManager",
                optional(enumerated(&["npm", "pnpm", "bun"]), text("npm")),
            )
            .param(
                "version",
                optional(ParameterType::Integer, ParameterValue::Integer(22)),
            )
    }

    // Parameter values.

    #[test]
    fn defaults_apply_when_nothing_is_required() {
        let composed = run(&project(&[("@metamao/node", "^1")]), &[node()], &[]).unwrap();
        assert_eq!(
            values_of(&composed, "@metamao/node"),
            values(&[
                ("packageManager", text("npm")),
                ("version", ParameterValue::Integer(22)),
            ])
        );
    }

    #[test]
    fn every_composed_unit_has_values_even_without_parameters() {
        let units = [
            Spec::new("@metamao/app", "1.0.0").depends("@metamao/plain", "^1"),
            Spec::new("@metamao/plain", "1.0.0"),
        ];
        let composed = run(&project(&[("@metamao/app", "^1")]), &units, &[]).unwrap();
        let listed: Vec<&str> = composed
            .values
            .iter()
            .map(|(unit, _)| unit.as_str())
            .collect();
        assert_eq!(listed, ["@metamao/app", "@metamao/plain"]);
        assert!(composed.values.iter().all(|(_, values)| values.is_empty()));
    }

    #[test]
    fn a_dependent_s_options_override_a_default() {
        let units = [
            Spec::new("@metamao/bun-app", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("bun")),
            node(),
        ];
        let composed = run(&project(&[("@metamao/bun-app", "^1")]), &units, &[]).unwrap();
        assert_eq!(
            values_of(&composed, "@metamao/node"),
            values(&[
                ("packageManager", text("bun")),
                ("version", ParameterValue::Integer(22)),
            ])
        );
    }

    #[test]
    fn consumer_values_configure_explicit_selections() {
        let composed = run(
            &project(&[("@metamao/node", "^1")]),
            &[node()],
            &[("@metamao/node", "version", ParameterValue::Integer(24))],
        )
        .unwrap();
        assert_eq!(
            values_of(&composed, "@metamao/node"),
            values(&[
                ("packageManager", text("npm")),
                ("version", ParameterValue::Integer(24)),
            ])
        );
    }

    #[test]
    fn identical_requirements_are_compatible() {
        let units = [
            Spec::new("@metamao/api", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("pnpm")),
            Spec::new("@metamao/web", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("pnpm")),
            node(),
        ];
        let composed = run(
            &project(&[
                ("@metamao/api", "^1"),
                ("@metamao/web", "^1"),
                ("@metamao/node", "^1"),
            ]),
            &units,
            &[("@metamao/node", "packageManager", text("pnpm"))],
        )
        .unwrap();
        assert_eq!(
            values_of(&composed, "@metamao/node")[0],
            ("packageManager".to_owned(), text("pnpm"))
        );
    }

    #[test]
    fn different_requirements_are_an_options_conflict_naming_every_requirer() {
        let units = [
            Spec::new("@metamao/api", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("pnpm")),
            Spec::new("@metamao/web", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("bun")),
            node(),
        ];
        let errors = run(
            &project(&[
                ("@metamao/api", "^1"),
                ("@metamao/web", "^1"),
                ("@metamao/node", "^1"),
            ]),
            &units,
            &[("@metamao/node", "packageManager", text("npm"))],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::OptionsConflict {
                unit: identity("@metamao/node"),
                version: version("1.0.0"),
                parameter: name("packageManager"),
                requirements: vec![
                    requirement(Requirer::Root, text("npm")),
                    requirement(unit("@metamao/api", "1.0.0"), text("pnpm")),
                    requirement(unit("@metamao/web", "1.0.0"), text("bun")),
                ],
            }]
        );
        let message = errors[0].to_string();
        for expected in [
            "options conflict",
            "`packageManager` of @metamao/node 1.0.0",
            "\"npm\" from the root",
            "\"pnpm\" from @metamao/api 1.0.0",
            "\"bun\" from @metamao/web 1.0.0",
            "no requirement takes precedence",
        ] {
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn a_default_never_conflicts_with_a_requirement() {
        let units = [
            Spec::new("@metamao/api", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("pnpm")),
            Spec::new("@metamao/web", "1.0.0").depends("@metamao/node", "^1"),
            node(),
        ];
        let composed = run(
            &project(&[("@metamao/api", "^1"), ("@metamao/web", "^1")]),
            &units,
            &[],
        )
        .unwrap();
        assert_eq!(
            values_of(&composed, "@metamao/node")[0],
            ("packageManager".to_owned(), text("pnpm"))
        );
    }

    #[test]
    fn a_requirement_must_name_a_parameter_the_resolved_version_declares() {
        let units = [
            Spec::new("@metamao/app", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("pnpm")),
            node(),
            // The resolved version no longer declares `packageManager`.
            Spec::new("@metamao/node", "1.1.0")
                .param(
                    "version",
                    optional(ParameterType::Integer, ParameterValue::Integer(22)),
                )
                .param("runtime", optional(ParameterType::String, text("node"))),
        ];
        let errors = run(&project(&[("@metamao/app", "^1")]), &units, &[]).unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::UndeclaredParameter {
                unit: identity("@metamao/node"),
                version: version("1.1.0"),
                parameter: name("packageManager"),
                requirer: unit("@metamao/app", "1.0.0"),
                declared: vec![name("runtime"), name("version")],
            }]
        );
        let message = errors[0].to_string();
        for expected in [
            "@metamao/app 1.0.0 sets parameter `packageManager` of @metamao/node 1.1.0",
            "does not declare it",
            "`runtime`, `version`",
        ] {
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn an_undeclared_parameter_of_a_unit_without_parameters_says_so() {
        let errors = run(
            &project(&[("@metamao/plain", "^1")]),
            &[Spec::new("@metamao/plain", "1.0.0")],
            &[("@metamao/plain", "anything", ParameterValue::Boolean(true))],
        )
        .unwrap_err();
        let message = errors[0].to_string();
        assert!(
            message.contains("the root sets parameter `anything`"),
            "{message}"
        );
        assert!(message.contains("it declares no parameters"), "{message}");
    }

    #[test]
    fn values_must_match_the_parameter_type_and_allowed_values() {
        let units = [
            Spec::new("@metamao/app", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("yarn")),
            node(),
        ];
        let errors = run(
            &project(&[("@metamao/app", "^1"), ("@metamao/node", "^1")]),
            &units,
            &[("@metamao/node", "version", text("22"))],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [
                CompositionError::InvalidValue {
                    unit: identity("@metamao/node"),
                    version: version("1.0.0"),
                    parameter: name("packageManager"),
                    requirement: requirement(unit("@metamao/app", "1.0.0"), text("yarn")),
                    error: ParameterValueError::NotAllowed {
                        value: "yarn".to_owned(),
                        allowed: AllowedValues::new(vec![
                            "npm".to_owned(),
                            "pnpm".to_owned(),
                            "bun".to_owned(),
                        ])
                        .unwrap(),
                    },
                },
                CompositionError::InvalidValue {
                    unit: identity("@metamao/node"),
                    version: version("1.0.0"),
                    parameter: name("version"),
                    requirement: requirement(Requirer::Root, text("22")),
                    error: ParameterValueError::TypeMismatch {
                        expected: ValueKind::Integer,
                        found: ValueKind::String,
                    },
                },
            ]
        );
        let message = errors[1].to_string();
        assert!(
            message.contains("the root sets parameter `version` of @metamao/node 1.0.0 to \"22\""),
            "{message}"
        );
        assert!(message.contains("expected an integer"), "{message}");
    }

    #[test]
    fn an_invalid_requirement_is_reported_once_not_also_as_missing_or_conflicting() {
        let units = [
            Spec::new("@metamao/api", "1.0.0")
                .depends("@metamao/db", "^1")
                .option("@metamao/db", "port", text("5432")),
            Spec::new("@metamao/web", "1.0.0")
                .depends("@metamao/db", "^1")
                .option("@metamao/db", "port", ParameterValue::Integer(6543)),
            Spec::new("@metamao/db", "1.0.0").param("port", required(ParameterType::Integer)),
        ];
        let errors = run(
            &project(&[("@metamao/api", "^1"), ("@metamao/web", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(matches!(errors[0], CompositionError::InvalidValue { .. }));
    }

    #[test]
    fn a_missing_value_for_an_explicit_selection_is_the_root_s_to_supply() {
        let units = [Spec::new("@metamao/worker", "1.0.0")
            .param("accountId", required(ParameterType::String))];
        let errors = run(&project(&[("@metamao/worker", "^1")]), &units, &[]).unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::MissingValue {
                unit: identity("@metamao/worker"),
                version: version("1.0.0"),
                parameter: name("accountId"),
                responsible: vec![Requirer::Root],
                example: None,
            }]
        );
        let message = errors[0].to_string();
        for expected in [
            "required parameter `accountId` of @metamao/worker 1.0.0",
            "no default",
            "supplied by the root",
        ] {
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn a_missing_transitive_value_is_attributed_to_the_units_that_depend_on_it() {
        let units = [
            Spec::new("@metamao/app", "1.0.0").depends("@metamao/api", "^1"),
            Spec::new("@metamao/api", "1.0.0").depends("@metamao/db", "^1"),
            Spec::new("@metamao/admin", "1.0.0").depends("@metamao/db", "^1"),
            Spec::new("@metamao/db", "1.0.0").param("url", required(ParameterType::String)),
        ];
        let errors = run(
            &project(&[("@metamao/app", "^1"), ("@metamao/admin", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::MissingValue {
                unit: identity("@metamao/db"),
                version: version("1.0.0"),
                parameter: name("url"),
                responsible: vec![
                    unit("@metamao/admin", "1.0.0"),
                    unit("@metamao/api", "1.0.0"),
                ],
                example: None,
            }]
        );
        let message = errors[0].to_string();
        assert!(!message.contains("the root"), "{message}");
        assert!(
            message.contains("supplied by @metamao/admin 1.0.0, @metamao/api 1.0.0"),
            "{message}"
        );
    }

    #[test]
    fn a_dependent_satisfies_a_required_parameter_through_its_options() {
        let units = [
            Spec::new("@metamao/api", "1.0.0")
                .depends("@metamao/db", "^1")
                .option("@metamao/db", "url", text("postgres://db")),
            Spec::new("@metamao/db", "1.0.0").param("url", required(ParameterType::String)),
        ];
        let composed = run(&project(&[("@metamao/api", "^1")]), &units, &[]).unwrap();
        assert_eq!(
            values_of(&composed, "@metamao/db"),
            values(&[("url", text("postgres://db"))])
        );
    }

    #[test]
    fn consumer_values_on_a_transitive_unit_are_rejected() {
        let units = [
            Spec::new("@metamao/app", "1.0.0").depends("@metamao/node", "^1"),
            node(),
        ];
        let errors = run(
            &project(&[("@metamao/app", "^1")]),
            &units,
            &[("@metamao/node", "packageManager", text("bun"))],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::RootTarget {
                unit: identity("@metamao/node"),
                reason: RootTargetReason::Transitive,
            }]
        );
        let message = errors[0].to_string();
        assert!(
            message.contains("only a dependency of other units"),
            "{message}"
        );
        assert!(message.contains("explicitly selected units"), "{message}");
    }

    #[test]
    fn consumer_values_on_a_unit_outside_the_composition_are_rejected() {
        let errors = run(
            &project(&[("@metamao/node", "^1")]),
            &[node()],
            &[("@metamao/absent", "anything", ParameterValue::Boolean(true))],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::RootTarget {
                unit: identity("@metamao/absent"),
                reason: RootTargetReason::NotComposed,
            }]
        );
        assert!(
            errors[0]
                .to_string()
                .contains("not part of the composition")
        );
    }

    // Unit builds.

    #[test]
    fn a_unit_build_composes_the_built_unit_with_its_dependencies() {
        let built = Spec::new("@metamao/bun-app", "0.1.0")
            .depends("@metamao/node", "^1")
            .option("@metamao/node", "packageManager", text("bun"))
            .param("title", required(ParameterType::String))
            .file("index.ts");
        let composed = run(
            &For::Build(built),
            &[node().file("package.json")],
            &[("@metamao/bun-app", "title", text("Demo"))],
        )
        .unwrap();
        assert_eq!(
            values_of(&composed, "@metamao/bun-app"),
            values(&[("title", text("Demo"))])
        );
        assert_eq!(
            values_of(&composed, "@metamao/node")[0],
            ("packageManager".to_owned(), text("bun"))
        );
        let files: Vec<(&str, &str)> = composed
            .files
            .iter()
            .map(|(file, contributor, _, _)| (file.as_str(), contributor.as_str()))
            .collect();
        assert_eq!(
            files,
            [
                ("index.ts", "@metamao/bun-app"),
                ("package.json", "@metamao/node")
            ]
        );
    }

    #[test]
    fn a_unit_build_s_values_configure_only_the_built_unit() {
        let built = Spec::new("@metamao/bun-app", "0.1.0").depends("@metamao/node", "^1");
        let errors = run(
            &For::Build(built),
            &[node()],
            &[("@metamao/node", "packageManager", text("bun"))],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::RootTarget {
                unit: identity("@metamao/node"),
                reason: RootTargetReason::NotTheBuiltUnit,
            }]
        );
        assert!(errors[0].to_string().contains("through its `options`"));
    }

    #[test]
    fn a_built_unit_s_missing_value_is_the_root_s_to_supply() {
        let built = Spec::new("@metamao/bun-app", "0.1.0")
            .depends("@metamao/db", "^1")
            .param("title", required(ParameterType::String));
        let db = Spec::new("@metamao/db", "1.0.0").param("url", required(ParameterType::String));
        let errors = run(&For::Build(built), &[db], &[]).unwrap_err();
        assert_eq!(
            errors,
            [
                CompositionError::MissingValue {
                    unit: identity("@metamao/bun-app"),
                    version: version("0.1.0"),
                    parameter: name("title"),
                    responsible: vec![Requirer::Root],
                    example: None,
                },
                CompositionError::MissingValue {
                    unit: identity("@metamao/db"),
                    version: version("1.0.0"),
                    parameter: name("url"),
                    responsible: vec![Requirer::Root],
                    example: None,
                },
            ]
        );
    }

    // Files.

    #[test]
    fn files_are_the_union_of_every_unit_s_files_with_their_contributor() {
        let units = [
            Spec::new("@metamao/app", "1.0.0")
                .depends("@metamao/node", "^1")
                .file("src/app.ts")
                .file("README.md"),
            node().file("package.json"),
        ];
        let composed = run(&project(&[("@metamao/app", "^1")]), &units, &[]).unwrap();
        let files: Vec<(&str, &str, &[u8])> = composed
            .files
            .iter()
            .map(|(file, contributor, contents, _)| {
                (file.as_str(), contributor.as_str(), contents.as_slice())
            })
            .collect();
        assert_eq!(
            files,
            [
                (
                    "README.md",
                    "@metamao/app",
                    b"@metamao/app README.md".as_slice()
                ),
                (
                    "package.json",
                    "@metamao/node",
                    b"@metamao/node package.json"
                ),
                ("src/app.ts", "@metamao/app", b"@metamao/app src/app.ts"),
            ]
        );
    }

    #[test]
    fn file_bytes_and_the_executable_bit_are_preserved_without_interpolation() {
        let binary = vec![0, 159, 146, 150, 255, b'\r', b'\n'];
        let template = b"name: {{ title }}\nrun: ${{ matrix.os }}\n".to_vec();
        let units = [Spec::new("@metamao/tools", "1.0.0")
            .param("title", optional(ParameterType::String, text("Demo")))
            .file_with("bin/run", binary.clone(), true)
            .file_with("ci.yml", template.clone(), false)];
        let composed = run(&project(&[("@metamao/tools", "^1")]), &units, &[]).unwrap();
        assert_eq!(
            composed.files,
            [
                (
                    "bin/run".to_owned(),
                    "@metamao/tools".to_owned(),
                    binary,
                    true
                ),
                (
                    "ci.yml".to_owned(),
                    "@metamao/tools".to_owned(),
                    template,
                    false
                ),
            ]
        );
    }

    #[test]
    fn independent_units_providing_the_same_path_collide() {
        let units = [
            Spec::new("@metamao/api", "1.0.0").file("README.md"),
            Spec::new("@metamao/web", "1.0.0").file("README.md"),
        ];
        let errors = run(
            &project(&[("@metamao/web", "^1"), ("@metamao/api", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::PathCollision {
                files: vec![
                    contributed("README.md", "@metamao/api"),
                    contributed("README.md", "@metamao/web"),
                ],
            }]
        );
        let message = errors[0].to_string();
        for expected in [
            "path collision",
            "README.md from @metamao/api",
            "README.md from @metamao/web",
            "amendment",
        ] {
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn a_unit_and_its_dependency_providing_the_same_path_collide() {
        let built = Spec::new("@metamao/bun-app", "0.1.0")
            .depends("@metamao/node", "^1")
            .file("package.json");
        let errors = run(&For::Build(built), &[node().file("package.json")], &[]).unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::PathCollision {
                files: vec![
                    contributed("package.json", "@metamao/bun-app"),
                    contributed("package.json", "@metamao/node"),
                ],
            }]
        );
    }

    #[test]
    fn paths_that_differ_only_in_case_collide() {
        let units = [
            Spec::new("@metamao/api", "1.0.0").file("README.md"),
            Spec::new("@metamao/web", "1.0.0").file("readme.md"),
        ];
        let errors = run(
            &project(&[("@metamao/api", "^1"), ("@metamao/web", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::PathCollision {
                files: vec![
                    contributed("README.md", "@metamao/api"),
                    contributed("readme.md", "@metamao/web"),
                ],
            }]
        );
        assert!(errors[0].to_string().contains("differ only in case"));
    }

    #[test]
    fn one_unit_s_paths_that_differ_only_in_case_collide() {
        let units = [Spec::new("@metamao/docs", "1.0.0")
            .file("Guide/Intro.md")
            .file("guide/intro.md")];
        let errors = run(&project(&[("@metamao/docs", "^1")]), &units, &[]).unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::PathCollision {
                files: vec![
                    contributed("Guide/Intro.md", "@metamao/docs"),
                    contributed("guide/intro.md", "@metamao/docs"),
                ],
            }]
        );
        assert!(errors[0].to_string().contains("differ only in case"));
    }

    // Templates.

    #[test]
    fn templates_are_delivered_without_the_suffix_and_ordinary_files_as_they_are() {
        let units = [Spec::new("@metamao/app", "1.0.0")
            .file("package.json.assemblr.tmpl")
            .file("config/app.yaml.assemblr.tmpl")
            // `.tmpl` alone, or the suffix elsewhere in the path, selects nothing.
            .file("notes.tmpl")
            .file("assemblr.tmpl/README.md")
            .file("Dockerfile")];
        let composed = run(&project(&[("@metamao/app", "^1")]), &units, &[]).unwrap();
        let delivered: Vec<&str> = composed
            .files
            .iter()
            .map(|(file, _, _, _)| file.as_str())
            .collect();
        assert_eq!(
            delivered,
            [
                "Dockerfile",
                "assemblr.tmpl/README.md",
                "config/app.yaml",
                "notes.tmpl",
                "package.json",
            ]
        );
        assert_eq!(
            composed.templates,
            [
                (
                    "config/app.yaml".to_owned(),
                    "config/app.yaml.assemblr.tmpl".to_owned()
                ),
                (
                    "package.json".to_owned(),
                    "package.json.assemblr.tmpl".to_owned()
                ),
            ]
        );
        // A template's source is delivered for rendering, unchanged here.
        let (_, _, contents, _) = &composed.files[4];
        assert_eq!(contents, b"@metamao/app package.json.assemblr.tmpl");
    }

    /// `@metamao/node` 1.0.0 providing a template and a script, published
    /// alone and with 1.1.0.
    fn provider() -> Vec<Spec> {
        vec![
            node().file("package.json.assemblr.tmpl").file_with(
                "bin/run",
                b"#!/bin/sh".to_vec(),
                true,
            ),
            node()
                .clone_as("1.1.0")
                .file("package.json.assemblr.tmpl")
                .file_with("bin/run", b"#!/bin/sh".to_vec(), true),
        ]
    }

    impl Spec {
        fn clone_as(mut self, version: &'static str) -> Self {
            self.version = version;
            self
        }
    }

    #[test]
    fn an_amendment_copy_takes_the_provider_s_file_s_place() {
        let built = Spec::new("@metamao/app", "0.1.0")
            .depends("@metamao/node", "=1.0.0")
            .amends("package.json.assemblr.tmpl", "@metamao/node", "1.0.0")
            .amends("bin/run", "@metamao/node", "1.0.0")
            .copy(
                "package.json.assemblr.tmpl",
                "{ \"name\": \"{{ own.name }}\" }",
            )
            .copy("bin/run", "#!/bin/sh\necho amended")
            .file("index.ts");
        let composed = run(&For::Build(built), &provider(), &[]).unwrap();
        let files: Vec<(&str, &str, &[u8], bool)> = composed
            .files
            .iter()
            .map(|(file, unit, contents, executable)| {
                (
                    file.as_str(),
                    unit.as_str(),
                    contents.as_slice(),
                    *executable,
                )
            })
            .collect();
        assert_eq!(
            files,
            [
                // The copy's bytes, the provider's bit.
                (
                    "bin/run",
                    "@metamao/node",
                    b"#!/bin/sh\necho amended".as_slice(),
                    true
                ),
                (
                    "index.ts",
                    "@metamao/app",
                    b"@metamao/app index.ts".as_slice(),
                    false
                ),
                (
                    "package.json",
                    "@metamao/node",
                    b"{ \"name\": \"{{ own.name }}\" }".as_slice(),
                    false
                ),
            ]
        );
        assert_eq!(
            composed.templates,
            [(
                "package.json".to_owned(),
                "package.json.assemblr.tmpl".to_owned()
            )]
        );
        assert_eq!(
            composed.amended,
            [
                ("bin/run".to_owned(), "@metamao/app".to_owned()),
                ("package.json".to_owned(), "@metamao/app".to_owned()),
            ]
        );
    }

    #[test]
    fn every_amendment_problem_is_reported_and_the_provider_s_file_stands() {
        let built = Spec::new("@metamao/app", "0.1.0")
            .depends("@metamao/node", "^1")
            // Resolved to 1.1.0, not the base.
            .amends("bin/run", "@metamao/node", "1.0.0")
            .copy("bin/run", "#!/bin/sh\necho amended")
            // The provider's file is the template; the copy lacks the suffix.
            .amends("package.json", "@metamao/node", "1.1.0")
            .copy("package.json", "{}")
            // The provider's file is not a template; the copy has the suffix.
            .amends("bin/run.assemblr.tmpl", "@metamao/node", "1.1.0")
            .copy("bin/run.assemblr.tmpl", "")
            // Not provided at all.
            .amends("missing.txt", "@metamao/node", "1.1.0")
            .copy("missing.txt", "")
            // Declared without a copy.
            .amends("package.json.assemblr.tmpl", "@metamao/node", "1.1.0")
            // A copy without a declaration.
            .copy("stray.txt", "")
            // The unit's own file at an amended path collides.
            .file("package.json.assemblr.tmpl");
        let errors = run(&For::Build(built), &provider(), &[]).unwrap_err();
        let path = |input: &str| path(input);
        assert_eq!(
            errors,
            [
                CompositionError::AmendmentBase {
                    amender: Requirer::Root,
                    path: path("bin/run"),
                    unit: identity("@metamao/node"),
                    base: version("1.0.0"),
                    resolved: version("1.1.0"),
                },
                CompositionError::AmendmentNotProvided {
                    amender: Requirer::Root,
                    path: path("bin/run.assemblr.tmpl"),
                    unit: identity("@metamao/node"),
                    version: version("1.1.0"),
                    similar: Some(path("bin/run")),
                },
                CompositionError::AmendmentNotProvided {
                    amender: Requirer::Root,
                    path: path("missing.txt"),
                    unit: identity("@metamao/node"),
                    version: version("1.1.0"),
                    similar: None,
                },
                CompositionError::AmendmentNotProvided {
                    amender: Requirer::Root,
                    path: path("package.json"),
                    unit: identity("@metamao/node"),
                    version: version("1.1.0"),
                    similar: Some(path("package.json.assemblr.tmpl")),
                },
                CompositionError::AmendmentMissingCopy {
                    amender: Requirer::Root,
                    path: path("package.json.assemblr.tmpl"),
                    provider: identity("@metamao/node"),
                },
                CompositionError::AmendmentUndeclared {
                    amender: Requirer::Root,
                    path: path("stray.txt"),
                },
                CompositionError::PathCollision {
                    files: vec![
                        templated("package.json.assemblr.tmpl", "@metamao/app"),
                        templated("package.json.assemblr.tmpl", "@metamao/node"),
                    ],
                },
            ]
        );
        let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
        assert!(
            messages[0].contains("applied only to its base version"),
            "{}",
            messages[0]
        );
        assert!(
            messages[1].contains("src/bin/run, which is not a template"),
            "{}",
            messages[1]
        );
        assert!(
            messages[2].ends_with("has no src/missing.txt"),
            "{}",
            messages[2]
        );
        assert!(
            messages[3].contains("template src/package.json.assemblr.tmpl"),
            "{}",
            messages[3]
        );
        assert!(messages[4].contains("does not exist"), "{}", messages[4]);
        assert!(messages[5].contains("not declared"), "{}", messages[5]);
    }

    /// `@metamao/amender` 1.0.0, a published unit amending two files of
    /// `@metamao/node` 1.0.0.
    fn amender() -> Spec {
        Spec::new("@metamao/amender", "1.0.0")
            .depends("@metamao/node", "=1.0.0")
            .amends("bin/run", "@metamao/node", "1.0.0")
            .copy("bin/run", "#!/bin/sh\necho amended by amender")
            .amends("package.json.assemblr.tmpl", "@metamao/node", "1.0.0")
            .copy(
                "package.json.assemblr.tmpl",
                "{ \"name\": \"{{ own.name }}\" }",
            )
            .file("amender.txt")
    }

    #[test]
    fn a_dependency_s_published_amendments_are_applied_at_the_base() {
        // The amender is a transitive dependency: the root depends on it
        // only through `@metamao/web`.
        let web = Spec::new("@metamao/web", "1.0.0")
            .depends("@metamao/amender", "^1")
            .file("web.txt");
        let built = Spec::new("@metamao/app", "0.1.0")
            .depends("@metamao/web", "^1")
            .file("index.ts");
        let mut published = provider();
        published.push(amender());
        published.push(web);
        let composed = run(&For::Build(built), &published, &[]).unwrap();
        let files: Vec<(&str, &str, &[u8], bool)> = composed
            .files
            .iter()
            .map(|(file, unit, contents, executable)| {
                (
                    file.as_str(),
                    unit.as_str(),
                    contents.as_slice(),
                    *executable,
                )
            })
            .collect();
        assert_eq!(
            files,
            [
                (
                    "amender.txt",
                    "@metamao/amender",
                    b"@metamao/amender amender.txt".as_slice(),
                    false
                ),
                (
                    "bin/run",
                    "@metamao/node",
                    b"#!/bin/sh\necho amended by amender".as_slice(),
                    true
                ),
                (
                    "index.ts",
                    "@metamao/app",
                    b"@metamao/app index.ts".as_slice(),
                    false
                ),
                (
                    "package.json",
                    "@metamao/node",
                    b"{ \"name\": \"{{ own.name }}\" }".as_slice(),
                    false
                ),
                (
                    "web.txt",
                    "@metamao/web",
                    b"@metamao/web web.txt".as_slice(),
                    false
                ),
            ]
        );
        assert_eq!(
            composed.amended,
            [
                ("bin/run".to_owned(), "@metamao/amender".to_owned()),
                ("package.json".to_owned(), "@metamao/amender".to_owned()),
            ]
        );
    }

    #[test]
    fn a_published_amendment_of_another_version_than_its_base_is_applied_to_the_resolved_one() {
        // The root requires 1.1.0, so the amender's base 1.0.0 is not the
        // resolved version: the amendment is carried with its base, for the
        // composer to apply to the resolved file.
        let amender = Spec::new("@metamao/amender", "1.0.0")
            .depends("@metamao/node", "^1")
            .amends("bin/run", "@metamao/node", "1.0.0")
            .copy("bin/run", "#!/bin/sh\necho amended")
            .file("amender.txt");
        let built = Spec::new("@metamao/app", "0.1.0")
            .depends("@metamao/amender", "^1")
            .depends("@metamao/node", "^1.1");
        let mut published = provider();
        published.push(amender);
        let composed = run(&For::Build(built), &published, &[]).unwrap();
        assert_eq!(
            composed.amended,
            [("bin/run".to_owned(), "@metamao/amender".to_owned())]
        );
        assert_eq!(composed.bases, [("bin/run".to_owned(), "1.0.0".to_owned())]);
    }

    #[test]
    fn the_unit_being_built_must_amend_the_version_it_resolves() {
        let built = Spec::new("@metamao/app", "0.1.0")
            .depends("@metamao/node", "^1.1")
            .amends("bin/run", "@metamao/node", "1.0.0")
            .copy("bin/run", "#!/bin/sh\necho amended");
        let errors = run(&For::Build(built), &provider(), &[]).unwrap_err();
        assert!(
            matches!(
                errors.as_slice(),
                [CompositionError::AmendmentBase {
                    amender: Requirer::Root,
                    ..
                }]
            ),
            "{errors:?}"
        );
    }

    #[test]
    fn several_amenders_of_one_provider_file_are_refused_together() {
        // The root and a published unit amend `bin/run`, one of the copies
        // not being text, which is refused for every amender; two published
        // units amend
        // `package.json.assemblr.tmpl`, a JSON file, whose copies are
        // carried for the merge; and a file only one member amends still
        // takes that copy.
        let other = Spec::new("@metamao/other", "1.0.0")
            .depends("@metamao/node", "=1.0.0")
            .amends("package.json.assemblr.tmpl", "@metamao/node", "1.0.0")
            .copy("package.json.assemblr.tmpl", "{}")
            .amends("README.md", "@metamao/node", "1.0.0")
            .copy("README.md", "amended readme")
            .file("other.txt");
        let built = Spec::new("@metamao/app", "0.1.0")
            .depends("@metamao/amender", "^1")
            .depends("@metamao/other", "^1")
            .depends("@metamao/node", "=1.0.0")
            .amends("bin/run", "@metamao/node", "1.0.0")
            .copy_bytes("bin/run", b"#!/bin/sh\0binary")
            .file("index.ts");
        let mut published = provider();
        published[0] = published[0].clone().file("README.md");
        published.push(amender());
        published.push(other);
        let errors = run(&For::Build(built.clone()), &published, &[]).unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::SeveralAmenders {
                path: path("bin/run"),
                unit: identity("@metamao/node"),
                version: version("1.0.0"),
                amenders: vec![
                    Requirer::Root,
                    Requirer::Unit(identity("@metamao/amender"), version("1.0.0")),
                ],
            }]
        );
        let message = errors[0].to_string();
        for part in [
            "src/bin/run of @metamao/node 1.0.0 is amended by this unit, @metamao/amender 1.0.0",
            "combined for JSON, JSONC, and UTF-8 text files, but not for YAML, TOML, XML, or INI",
            "no amender takes precedence",
        ] {
            assert!(message.contains(part), "{message}");
        }

        // Without the text file's amenders, the JSON file's copies are both
        // carried, in requirer order, with the provider's own contents.
        let built = Spec::new("@metamao/app", "0.1.0")
            .depends("@metamao/amender", "^1")
            .depends("@metamao/other", "^1")
            .depends("@metamao/node", "=1.0.0")
            .amends("package.json.assemblr.tmpl", "@metamao/node", "1.0.0")
            .copy("package.json.assemblr.tmpl", "{ \"app\": true }")
            .file("index.ts");
        let composed = run(&For::Build(built), &published, &[]).unwrap();
        assert_eq!(
            composed.amended,
            [
                ("README.md".to_owned(), "@metamao/other".to_owned()),
                ("bin/run".to_owned(), "@metamao/amender".to_owned()),
                ("package.json".to_owned(), "@metamao/app".to_owned()),
                ("package.json".to_owned(), "@metamao/amender".to_owned()),
                ("package.json".to_owned(), "@metamao/other".to_owned()),
            ]
        );
        let package = composed
            .files
            .iter()
            .find(|(file, _, _, _)| file == "package.json")
            .unwrap();
        assert_eq!(package.2, b"@metamao/node package.json.assemblr.tmpl");
        let readme = composed
            .files
            .iter()
            .find(|(file, _, _, _)| file == "README.md")
            .unwrap();
        assert_eq!(readme.2, b"amended readme");
    }

    #[test]
    fn several_text_amenders_of_one_file_are_carried_for_the_merge() {
        // The root and a published unit both amend `bin/run`, and both
        // copies are text: the composer keeps both, in requirer order, with
        // the provider's own contents, for the line merge.
        let built = Spec::new("@metamao/app", "0.1.0")
            .depends("@metamao/amender", "^1")
            .depends("@metamao/node", "=1.0.0")
            .amends("bin/run", "@metamao/node", "1.0.0")
            .copy("bin/run", "#!/bin/sh\necho amended by app")
            .file("index.ts");
        let mut published = provider();
        published.push(amender());
        let composed = run(&For::Build(built), &published, &[]).unwrap();
        assert_eq!(
            composed.amended,
            [
                ("bin/run".to_owned(), "@metamao/app".to_owned()),
                ("bin/run".to_owned(), "@metamao/amender".to_owned()),
                // The published unit's other amendment, of a JSON file.
                ("package.json".to_owned(), "@metamao/amender".to_owned()),
            ]
        );
        let run_file = composed
            .files
            .iter()
            .find(|(file, _, _, _)| file == "bin/run")
            .unwrap();
        assert_eq!(run_file.2, b"#!/bin/sh");
    }

    #[test]
    fn a_template_and_an_ordinary_file_delivered_at_the_same_path_collide() {
        let units = [
            Spec::new("@metamao/api", "1.0.0").file("package.json.assemblr.tmpl"),
            Spec::new("@metamao/web", "1.0.0").file("package.json"),
        ];
        let errors = run(
            &project(&[("@metamao/web", "^1"), ("@metamao/api", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::PathCollision {
                files: vec![
                    contributed("package.json", "@metamao/web"),
                    templated("package.json.assemblr.tmpl", "@metamao/api"),
                ],
            }]
        );
        let message = errors[0].to_string();
        for expected in [
            "package.json from @metamao/web",
            "package.json.assemblr.tmpl from @metamao/api (delivered as package.json)",
            "no unit takes precedence",
        ] {
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn one_unit_s_template_and_ordinary_file_at_the_same_delivered_path_collide() {
        let units = [Spec::new("@metamao/app", "1.0.0")
            .file("wrangler.jsonc")
            .file("wrangler.jsonc.assemblr.tmpl")];
        let errors = run(&project(&[("@metamao/app", "^1")]), &units, &[]).unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::PathCollision {
                files: vec![
                    contributed("wrangler.jsonc", "@metamao/app"),
                    templated("wrangler.jsonc.assemblr.tmpl", "@metamao/app"),
                ],
            }]
        );
        let message = errors[0].to_string();
        assert!(
            message.contains("a unit delivers each path once"),
            "{message}"
        );
        assert!(!message.contains("amendment"), "{message}");
    }

    #[test]
    fn delivered_paths_are_compared_like_any_other_path() {
        // Case and nesting are checked on the delivered path.
        let units = [
            Spec::new("@metamao/api", "1.0.0")
                .file("README.md.assemblr.tmpl")
                .file("docs.assemblr.tmpl"),
            Spec::new("@metamao/web", "1.0.0")
                .file("readme.md")
                .file("docs/intro.md"),
        ];
        let errors = run(
            &project(&[("@metamao/web", "^1"), ("@metamao/api", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [
                CompositionError::PathCollision {
                    files: vec![
                        templated("README.md.assemblr.tmpl", "@metamao/api"),
                        contributed("readme.md", "@metamao/web"),
                    ],
                },
                CompositionError::NestedPathCollision {
                    file: templated("docs.assemblr.tmpl", "@metamao/api"),
                    nested: vec![contributed("docs/intro.md", "@metamao/web")],
                },
            ]
        );
    }

    #[test]
    fn a_template_whose_delivered_path_is_invalid_is_reported() {
        // A reserved name such as `con.assemblr.tmpl` is already an invalid
        // source path; these are valid sources whose delivered path is not.
        let units = [Spec::new("@metamao/app", "1.0.0")
            .file("notes..assemblr.tmpl")
            .file("config/.assemblr.tmpl")];
        let errors = run(&project(&[("@metamao/app", "^1")]), &units, &[]).unwrap_err();
        let reported: Vec<String> = errors
            .iter()
            .map(|error| match error {
                CompositionError::InvalidDeliveredPath { path, unit, .. } => {
                    format!("{path} {unit}")
                }
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(
            reported,
            [
                "config/.assemblr.tmpl @metamao/app",
                "notes..assemblr.tmpl @metamao/app"
            ]
        );
        let message = errors[0].to_string();
        for expected in [
            "config/.assemblr.tmpl from @metamao/app is a template",
            "without `.assemblr.tmpl` its path is invalid",
        ] {
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn delivered_path_strips_the_suffix_exactly() {
        assert_eq!(delivered_path(&path("a/b.json")), Ok(None));
        assert_eq!(delivered_path(&path("a/b.tmpl")), Ok(None));
        assert_eq!(delivered_path(&path("a/b.ASSEMBLR.TMPL")), Ok(None));
        assert_eq!(
            delivered_path(&path("a/b.json.assemblr.tmpl")),
            Ok(Some(path("a/b.json")))
        );
        assert_eq!(
            delivered_path(&path("a/b.assemblr.tmpl.assemblr.tmpl")),
            Ok(Some(path("a/b.assemblr.tmpl")))
        );
        assert!(delivered_path(&path(".assemblr.tmpl")).is_err());
        assert!(delivered_path(&path("a/.assemblr.tmpl")).is_err());
    }

    #[test]
    fn one_unit_s_file_where_it_also_needs_a_directory_collides() {
        let units = [Spec::new("@metamao/docs", "1.0.0")
            .file("guide")
            .file("Guide/intro.md")];
        let errors = run(&project(&[("@metamao/docs", "^1")]), &units, &[]).unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::NestedPathCollision {
                file: contributed("guide", "@metamao/docs"),
                nested: vec![contributed("Guide/intro.md", "@metamao/docs")],
            }]
        );
    }

    #[test]
    fn paths_that_differ_only_in_unicode_normalization_collide() {
        // A precomposed `é` and an `e` with a combining acute accent.
        let units = [
            Spec::new("@metamao/api", "1.0.0").file("caf\u{e9}.md"),
            Spec::new("@metamao/web", "1.0.0").file("cafe\u{301}.md"),
        ];
        let errors = run(
            &project(&[("@metamao/api", "^1"), ("@metamao/web", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::PathCollision {
                files: vec![
                    contributed("cafe\u{301}.md", "@metamao/web"),
                    contributed("caf\u{e9}.md", "@metamao/api"),
                ],
            }]
        );
        assert!(errors[0].to_string().contains("Unicode normalization"));
    }

    #[test]
    fn case_and_normalization_differences_combine() {
        // An uppercase precomposed `É`, a lowercase decomposed `é`, and
        // combining marks in another canonical order.
        let units = [Spec::new("@metamao/docs", "1.0.0")
            .file("\u{c9}t\u{e9}.md")
            .file("e\u{301}te\u{301}.md")
            .file("a\u{301}\u{323}.md")
            .file("a\u{323}\u{301}.md")];
        let errors = run(&project(&[("@metamao/docs", "^1")]), &units, &[]).unwrap_err();
        assert_eq!(
            errors,
            [
                CompositionError::PathCollision {
                    files: vec![
                        contributed("a\u{301}\u{323}.md", "@metamao/docs"),
                        contributed("a\u{323}\u{301}.md", "@metamao/docs"),
                    ],
                },
                CompositionError::PathCollision {
                    files: vec![
                        contributed("e\u{301}te\u{301}.md", "@metamao/docs"),
                        contributed("\u{c9}t\u{e9}.md", "@metamao/docs"),
                    ],
                },
            ]
        );
    }

    #[test]
    fn a_file_where_a_normalization_variant_needs_a_directory_collides() {
        let units = [Spec::new("@metamao/docs", "1.0.0")
            .file("caf\u{e9}")
            .file("CAFE\u{301}/menu.md")];
        let errors = run(&project(&[("@metamao/docs", "^1")]), &units, &[]).unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::NestedPathCollision {
                file: contributed("caf\u{e9}", "@metamao/docs"),
                nested: vec![contributed("CAFE\u{301}/menu.md", "@metamao/docs")],
            }]
        );
    }

    #[test]
    fn characters_a_case_insensitive_file_system_equates_collide() {
        // NTFS compares names by uppercase, which equates each pair, though
        // lowercasing alone keeps them apart.
        for (first, second) in [
            ("\u{3c2}.md", "\u{3c3}.md"),
            ("\u{b5}.md", "\u{3bc}.md"),
            ("\u{17f}.md", "s.md"),
            ("\u{131}.md", "i.md"),
            ("\u{1e9e}.md", "\u{df}.md"),
            ("\u{212a}.md", "k.md"),
        ] {
            let units = [Spec::new("@metamao/docs", "1.0.0").file(first).file(second)];
            let errors = run(&project(&[("@metamao/docs", "^1")]), &units, &[]).unwrap_err();
            assert!(
                matches!(errors.as_slice(), [CompositionError::PathCollision { .. }]),
                "{first} {second}: {errors:?}"
            );
        }
    }

    #[test]
    fn mappings_to_several_characters_are_not_applied() {
        // `ß` uppercases to `SS`, `ﬁ` to `FI`, and `İ` lowercases to `i` and
        // a combining dot; file systems keep each pair apart.
        for (first, second) in [
            ("\u{df}.md", "ss.md"),
            ("\u{fb01}le.md", "file.md"),
            ("\u{130}.md", "i.md"),
        ] {
            let units = [Spec::new("@metamao/docs", "1.0.0").file(first).file(second)];
            let composed = run(&project(&[("@metamao/docs", "^1")]), &units, &[]).unwrap();
            assert_eq!(composed.files.len(), 2, "{first} {second}");
        }
    }

    #[test]
    fn compatibility_equivalent_paths_do_not_collide() {
        // The `ﬁ` ligature is only compatibility-equivalent to `fi`, and file
        // systems keep the two apart.
        let units = [Spec::new("@metamao/docs", "1.0.0")
            .file("\u{fb01}le.md")
            .file("file.md")];
        let composed = run(&project(&[("@metamao/docs", "^1")]), &units, &[]).unwrap();
        assert_eq!(composed.files.len(), 2);
    }

    #[test]
    fn every_contributor_to_a_colliding_path_is_named_once() {
        let units = [
            Spec::new("@metamao/a", "1.0.0").file("LICENSE"),
            Spec::new("@metamao/b", "1.0.0").file("LICENSE"),
            Spec::new("@metamao/c", "1.0.0").file("license"),
        ];
        let errors = run(
            &project(&[
                ("@metamao/c", "^1"),
                ("@metamao/b", "^1"),
                ("@metamao/a", "^1"),
            ]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::PathCollision {
                files: vec![
                    contributed("LICENSE", "@metamao/a"),
                    contributed("LICENSE", "@metamao/b"),
                    contributed("license", "@metamao/c"),
                ],
            }]
        );
    }

    #[test]
    fn a_file_where_another_unit_needs_a_directory_collides() {
        let units = [
            Spec::new("@metamao/api", "1.0.0").file("config"),
            Spec::new("@metamao/web", "1.0.0")
                .file("Config/app.json")
                .file("config/nested/db.json"),
        ];
        let errors = run(
            &project(&[("@metamao/api", "^1"), ("@metamao/web", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::NestedPathCollision {
                file: contributed("config", "@metamao/api"),
                nested: vec![
                    contributed("Config/app.json", "@metamao/web"),
                    contributed("config/nested/db.json", "@metamao/web"),
                ],
            }]
        );
        let message = errors[0].to_string();
        assert!(
            message.contains("config from @metamao/api is a file"),
            "{message}"
        );
        assert!(message.contains("need it to be a directory"), "{message}");
    }

    #[test]
    fn every_collision_is_reported() {
        let units = [
            Spec::new("@metamao/api", "1.0.0")
                .file(".gitignore")
                .file("Dockerfile")
                .file("docs"),
            Spec::new("@metamao/web", "1.0.0")
                .file(".gitignore")
                .file("dockerfile")
                .file("docs/index.md"),
        ];
        let errors = run(
            &project(&[("@metamao/api", "^1"), ("@metamao/web", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [
                CompositionError::PathCollision {
                    files: vec![
                        contributed(".gitignore", "@metamao/api"),
                        contributed(".gitignore", "@metamao/web"),
                    ],
                },
                CompositionError::PathCollision {
                    files: vec![
                        contributed("Dockerfile", "@metamao/api"),
                        contributed("dockerfile", "@metamao/web"),
                    ],
                },
                CompositionError::NestedPathCollision {
                    file: contributed("docs", "@metamao/api"),
                    nested: vec![contributed("docs/index.md", "@metamao/web")],
                },
            ]
        );
    }

    // Inputs and ordering.

    #[test]
    fn content_that_does_not_match_the_graph_is_reported() {
        let app = Spec::new("@metamao/app", "1.0.0").depends("@metamao/node", "^1");
        let (app_manifest, app_files) = (app.manifest(), app.tree());
        let (node_manifest, node_files) = (node().manifest(), node().tree());
        let stale = Spec::new("@metamao/node", "0.9.0");
        let (stale_manifest, stale_files) = (stale.manifest(), stale.tree());
        let mut available = AvailableVersions::new();
        for manifest in [&app_manifest, &node_manifest] {
            available
                .entry(manifest.name().clone())
                .or_default()
                .insert(manifest.version().clone(), manifest.dependencies().clone());
        }
        let resolution = resolve(
            None,
            &dependencies(&[("@metamao/app", "^1")]),
            &available,
            &Lock::new(),
        )
        .unwrap();
        let mismatch = |unit: &str, resolved: &str| CompositionError::ContentMismatch {
            unit: identity(unit),
            version: version(resolved),
        };

        let none = FileTree::new();
        let missing = BTreeMap::new();
        assert_eq!(
            compose(
                resolution.graph(),
                Root::Project,
                &missing,
                &BTreeMap::new()
            )
            .unwrap_err(),
            [
                mismatch("@metamao/app", "1.0.0"),
                mismatch("@metamao/node", "1.0.0"),
            ]
        );

        let wrong_version: BTreeMap<_, _> = [
            (
                identity("@metamao/app"),
                UnitContent::new(&app_manifest, &app_files, &none),
            ),
            (
                identity("@metamao/node"),
                UnitContent::new(&stale_manifest, &stale_files, &none),
            ),
        ]
        .into();
        let errors = compose(
            resolution.graph(),
            Root::Project,
            &wrong_version,
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert_eq!(errors, [mismatch("@metamao/node", "1.0.0")]);
        assert!(errors[0].to_string().contains("does not match"));

        // A built unit whose dependencies differ from the graph's root.
        let built = Root::Unit(UnitContent::new(&node_manifest, &node_files, &none));
        let complete: BTreeMap<_, _> = [
            (
                identity("@metamao/app"),
                UnitContent::new(&app_manifest, &app_files, &none),
            ),
            (
                identity("@metamao/node"),
                UnitContent::new(&node_manifest, &node_files, &none),
            ),
        ]
        .into();
        assert_eq!(
            compose(resolution.graph(), built, &complete, &BTreeMap::new()).unwrap_err(),
            [mismatch("@metamao/node", "1.0.0")]
        );
    }

    /// Units whose composition fails in every way at once.
    fn every_problem() -> (For, Vec<Spec>, Vec<RootValue>) {
        let units = vec![
            Spec::new("@metamao/api", "1.0.0")
                .depends("@metamao/node", "^1")
                .depends("@metamao/db", "^1")
                .option("@metamao/node", "packageManager", text("pnpm"))
                .option("@metamao/node", "colour", text("blue"))
                .option("@metamao/db", "port", text("5432"))
                .amends("README.md", "@metamao/node", "1.0.0")
                .copy("README.md", "api readme")
                .file("README.md")
                .file("docs"),
            Spec::new("@metamao/web", "1.0.0")
                .depends("@metamao/node", "^1")
                .depends("@metamao/db", "^1")
                .option("@metamao/node", "packageManager", text("bun"))
                .amends("README.md", "@metamao/node", "1.0.0")
                .copy_bytes("README.md", b"web readme\0")
                .amends("missing.txt", "@metamao/node", "1.0.0")
                .copy("stray.txt", "stray")
                .file("readme.md")
                .file("docs/index.md")
                .file("notes..assemblr.tmpl"),
            node().file("README.md"),
            Spec::new("@metamao/db", "1.0.0")
                .param("port", required(ParameterType::Integer))
                .param("url", required(ParameterType::String)),
        ];
        let root = project(&[("@metamao/api", "^1"), ("@metamao/web", "^1")]);
        let root_values = vec![
            ("@metamao/node", "version", ParameterValue::Integer(24)),
            ("@metamao/web", "theme", text("dark")),
        ];
        (root, units, root_values)
    }

    #[test]
    fn every_problem_is_reported_in_a_stable_order() {
        let (root, units, root_values) = every_problem();
        let errors = run(&root, &units, &root_values).unwrap_err();
        let kinds: Vec<String> = errors
            .iter()
            .map(|error| match error {
                CompositionError::ContentMismatch { unit, .. } => format!("mismatch {unit}"),
                CompositionError::RootTarget { unit, .. } => format!("target {unit}"),
                CompositionError::UndeclaredParameter {
                    unit,
                    parameter,
                    requirer,
                    ..
                } => format!("undeclared {unit} {parameter} {requirer}"),
                CompositionError::InvalidValue {
                    unit, parameter, ..
                } => format!("invalid {unit} {parameter}"),
                CompositionError::MappingIncompatible {
                    unit, parameter, ..
                } => format!("mapping {unit} {parameter}"),
                CompositionError::OptionsConflict {
                    unit, parameter, ..
                } => format!("conflict {unit} {parameter}"),
                CompositionError::MissingValue {
                    unit, parameter, ..
                } => format!("missing {unit} {parameter}"),
                CompositionError::SeveralAmenders { unit, path, .. } => {
                    format!("several {unit} {path}")
                }
                CompositionError::AmendmentBase { amender, path, .. }
                | CompositionError::AmendmentNotProvided { amender, path, .. }
                | CompositionError::AmendmentMissingCopy { amender, path, .. }
                | CompositionError::AmendmentUndeclared { amender, path, .. } => {
                    format!("amendment {amender} {path}")
                }
                CompositionError::InvalidDeliveredPath { path, unit, .. } => {
                    format!("undeliverable {path} {unit}")
                }
                CompositionError::PathCollision { files } => format!("collision {}", files[0]),
                CompositionError::NestedPathCollision { file, .. } => format!("nested {file}"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "target @metamao/node",
                "invalid @metamao/db port",
                "undeclared @metamao/node colour @metamao/api 1.0.0",
                "undeclared @metamao/web theme the root",
                "conflict @metamao/node packageManager",
                "missing @metamao/db url",
                "several @metamao/node README.md",
                "amendment @metamao/web 1.0.0 missing.txt",
                "amendment @metamao/web 1.0.0 stray.txt",
                "undeliverable notes..assemblr.tmpl @metamao/web",
                "collision README.md from @metamao/api",
                "nested docs from @metamao/api",
            ]
        );
    }

    #[test]
    fn declaration_and_enumeration_order_never_change_the_outcome() {
        let (root, units, root_values) = every_problem();
        let succeeding = (
            project(&[("@metamao/bun-app", "^1"), ("@metamao/node", "^1")]),
            vec![
                Spec::new("@metamao/bun-app", "1.0.0")
                    .depends("@metamao/node", "^1")
                    .option("@metamao/node", "packageManager", text("bun"))
                    .file("src/index.ts")
                    .file("bunfig.toml"),
                node().file("package.json").file(".nvmrc"),
            ],
            vec![("@metamao/node", "version", ParameterValue::Integer(24))],
        );
        for (root, units, root_values) in [(root, units, root_values), succeeding] {
            let expected = run(&root, &units, &root_values);
            let reorder = |spec: &Spec| {
                let mut spec = spec.clone();
                spec.dependencies.reverse();
                spec.options.reverse();
                spec.params.reverse();
                spec.files.reverse();
                spec
            };
            let reordered_root = match &root {
                For::Project(selections) => {
                    let mut selections = selections.clone();
                    selections.reverse();
                    For::Project(selections)
                }
                For::Build(spec) => For::Build(reorder(spec)),
            };
            for rotation in 0..units.len() {
                let mut shuffled: Vec<Spec> = units.iter().map(reorder).collect();
                shuffled.rotate_left(rotation);
                let mut values = root_values.clone();
                let count = values.len().max(1);
                values.rotate_left(rotation % count);
                assert_eq!(run(&root, &shuffled, &values), expected);
                assert_eq!(run(&reordered_root, &shuffled, &values), expected);
            }
        }
    }

    // --- parameter mappings ---

    /// A stack that maps its `appName` to a Worker unit, which maps it on to
    /// the application unit: three levels, one name.
    fn stack() -> Vec<Spec> {
        vec![
            Spec::new("@metamao/app", "1.0.0")
                .param("appName", optional(ParameterType::String, text("my-app"))),
            Spec::new("@metamao/worker", "1.0.0")
                .depends("@metamao/app", "^1")
                .param("appName", optional(ParameterType::String, text("my-app")))
                .map("@metamao/app", "appName", "appName"),
            Spec::new("@metamao/db", "1.0.0")
                .depends("@metamao/app", "^1")
                .depends("@metamao/worker", "^1")
                .param("appName", optional(ParameterType::String, text("my-app")))
                .param("database", optional(ParameterType::String, text("app")))
                .map("@metamao/app", "appName", "appName")
                .map("@metamao/worker", "appName", "appName"),
        ]
    }

    #[test]
    fn a_mapping_carries_the_source_default_through_every_level() {
        let composed = run(&project(&[("@metamao/db", "^1")]), &stack(), &[]).unwrap();
        for unit in ["@metamao/app", "@metamao/worker", "@metamao/db"] {
            assert_eq!(
                values_of(&composed, unit)
                    .into_iter()
                    .find(|(name, _)| name == "appName")
                    .unwrap()
                    .1,
                text("my-app"),
                "{unit}"
            );
        }
    }

    #[test]
    fn one_value_the_consumer_gives_reaches_every_dependency_once() {
        let composed = run(
            &project(&[("@metamao/db", "^1")]),
            &stack(),
            &[("@metamao/db", "appName", text("acme-portal"))],
        )
        .unwrap();
        for unit in ["@metamao/app", "@metamao/worker", "@metamao/db"] {
            assert!(
                values_of(&composed, unit).contains(&("appName".to_owned(), text("acme-portal"))),
                "{unit}"
            );
        }
        // The identical requirements of two paths (the stack's own and the
        // worker's) agree, and the unit's other parameters are untouched.
        assert!(
            values_of(&composed, "@metamao/db").contains(&("database".to_owned(), text("app")))
        );
    }

    #[test]
    fn a_value_set_on_a_dependency_directly_conflicts_with_a_mapping_and_says_so() {
        let errors = run(
            &project(&[("@metamao/db", "^1"), ("@metamao/app", "^1")]),
            &stack(),
            &[("@metamao/app", "appName", text("other"))],
        )
        .unwrap_err();
        let conflict = errors
            .iter()
            .find(|error| matches!(error, CompositionError::OptionsConflict { .. }))
            .unwrap();
        let message = conflict.to_string();
        assert!(message.contains("\"other\" from the root"), "{message}");
        assert!(
            message.contains("\"my-app\" from @metamao/worker 1.0.0 (mapped from its `appName`)"),
            "{message}"
        );
        assert!(
            message.contains("no requirement takes precedence"),
            "{message}"
        );
    }

    #[test]
    fn mappings_never_convert_values_and_known_incompatibilities_are_refused() {
        let incompatible = |source: ParameterDefinition, target: ParameterDefinition| {
            let units = vec![
                Spec::new("@metamao/lib", "1.0.0").param("x", target),
                Spec::new("@metamao/top", "1.0.0")
                    .depends("@metamao/lib", "^1")
                    .param("y", source)
                    .map("@metamao/lib", "x", "y"),
            ];
            run(
                &project(&[("@metamao/top", "^1")]),
                &units,
                &[("@metamao/top", "y", text("a"))],
            )
        };
        // A free string cannot configure an enumerated string.
        let errors = incompatible(
            optional(ParameterType::String, text("a")),
            optional(enumerated(&["a", "b"]), text("a")),
        )
        .unwrap_err();
        assert!(
            matches!(
                errors.as_slice(),
                [CompositionError::MappingIncompatible { .. }]
            ),
            "{errors:?}"
        );
        assert!(
            errors[0]
                .to_string()
                .contains("a free string may hold values"),
            "{}",
            errors[0]
        );
        // An integer cannot configure a string.
        let errors = incompatible(
            optional(ParameterType::Integer, ParameterValue::Integer(1)),
            optional(ParameterType::String, text("a")),
        )
        .unwrap_err();
        assert!(
            errors[0].to_string().contains("it is an integer"),
            "{}",
            errors[0]
        );
        // An enumerated string whose values the target allows, and a plain
        // string target, are fine; one with a value the target lacks is not.
        assert!(
            incompatible(
                optional(enumerated(&["a"]), text("a")),
                optional(enumerated(&["a", "b"]), text("a")),
            )
            .is_ok()
        );
        assert!(
            incompatible(
                optional(enumerated(&["a", "c"]), text("a")),
                optional(ParameterType::String, text("a")),
            )
            .is_ok()
        );
        let errors = incompatible(
            optional(enumerated(&["a", "c"]), text("a")),
            optional(enumerated(&["a", "b"]), text("a")),
        )
        .unwrap_err();
        assert!(
            errors[0].to_string().contains("allows \"c\""),
            "{}",
            errors[0]
        );
    }

    #[test]
    fn the_resolved_value_is_validated_even_when_the_definitions_are_compatible() {
        // Both are enumerated and the source's values fit, but the consumer
        // gives the source a value that is not one of them.
        let units = vec![
            Spec::new("@metamao/lib", "1.0.0").param(
                "mode",
                optional(enumerated(&["dev", "prod", "test"]), text("dev")),
            ),
            Spec::new("@metamao/top", "1.0.0")
                .depends("@metamao/lib", "^1")
                .param("mode", optional(enumerated(&["dev", "prod"]), text("dev")))
                .map("@metamao/lib", "mode", "mode"),
        ];
        let errors = run(
            &project(&[("@metamao/top", "^1")]),
            &units,
            &[("@metamao/top", "mode", text("test"))],
        )
        .unwrap_err();
        assert!(
            errors.iter().all(|error| matches!(error, CompositionError::InvalidValue { unit, .. } if *unit == identity("@metamao/top"))),
            "{errors:?}"
        );
    }

    #[test]
    fn a_mapping_from_a_required_parameter_without_a_value_reports_the_source_once() {
        let units = vec![
            Spec::new("@metamao/lib", "1.0.0").param("name", required(ParameterType::String)),
            Spec::new("@metamao/top", "1.0.0")
                .depends("@metamao/lib", "^1")
                .param("name", required(ParameterType::String))
                .map("@metamao/lib", "name", "name"),
        ];
        let errors = run(&project(&[("@metamao/top", "^1")]), &units, &[]).unwrap_err();
        // The source is the one to supply; the dependency's parameter is not
        // also reported, since the source's value will reach it.
        let missing: Vec<String> = errors
            .iter()
            .filter_map(|error| match error {
                CompositionError::MissingValue { unit, .. } => Some(unit.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(missing, ["@metamao/top"]);
    }

    #[test]
    fn a_mapping_to_a_parameter_the_dependency_does_not_declare_is_reported() {
        let units = vec![
            Spec::new("@metamao/lib", "1.0.0"),
            Spec::new("@metamao/top", "1.0.0")
                .depends("@metamao/lib", "^1")
                .param("name", optional(ParameterType::String, text("n")))
                .map("@metamao/lib", "nme", "name"),
        ];
        let errors = run(&project(&[("@metamao/top", "^1")]), &units, &[]).unwrap_err();
        assert!(
            matches!(errors.as_slice(), [CompositionError::UndeclaredParameter { parameter, .. }] if parameter.as_str() == "nme"),
            "{errors:?}"
        );
    }

    #[test]
    fn mappings_give_the_same_values_whatever_order_the_units_come_in() {
        let forward = run(
            &project(&[("@metamao/db", "^1")]),
            &stack(),
            &[("@metamao/db", "appName", text("acme"))],
        )
        .unwrap();
        let mut reversed = stack();
        reversed.reverse();
        let backward = run(
            &project(&[("@metamao/db", "^1")]),
            &reversed,
            &[("@metamao/db", "appName", text("acme"))],
        )
        .unwrap();
        assert_eq!(forward.values, backward.values);
    }
}
