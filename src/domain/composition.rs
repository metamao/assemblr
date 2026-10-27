//! Composition of a resolved graph: the file tree the units' `src/` files
//! compose into.
//!
//! [`compose`] works on plain data: the resolved graph, each resolved unit's
//! manifest and `src/` files, and the root. It either returns the
//! composition or every problem that prevents it.
//!
//! # Files
//!
//! The composed file tree is the union of the units' `src/` files, each
//! recording the unit that contributed it, keyed by its path. Every file is
//! opaque bytes with an executable bit, delivered unchanged: nothing is
//! rendered or substituted. Two files collide when their paths are equal or
//! differ only in case or in Unicode normalization, which some file systems
//! cannot keep apart, or when one file's path is a directory of another's
//! under the same comparison. Case is compared one character at a time, by
//! simple (one-to-one) case mappings, as case-insensitive file systems
//! compare it (see `collision_key`). Every collision is reported, whether
//! the units are independent or one depends on the other, and no unit wins.
//!
//! Each composed file also records the placeholder declarations in its
//! scope: those of the units that contribute it, its provider and its
//! amenders, and of the units each of them depends on, directly or
//! transitively (see `docs/domain/placeholders.md`).
//!
//! # Amendments
//!
//! Any member may amend files of the units it depends on, directly or
//! transitively: the unit being built from its working tree, and a resolved
//! unit from its published version. Its manifest declares each amended path
//! with the provider and the exact base version, and the edited copy lives
//! at `amendments/<path>` (see `docs/domain/amendments-and-composition.md`).
//! The copy takes the place of the provider's file, at the provider's path
//! and with the provider's executable bit. An amendment's base is layered:
//! the provider's file as the amenders below the amender, those in its
//! dependency closure, compose it. The composition records, for each
//! amendment, which other amenders of the file lie below its amender
//! ([`Amendment::below`]); layering the copies is the composer's business.
//! A member's copy is taken from the provider version its declaration
//! names; when the provider resolves to another, the composer applies the
//! copy's changes to the file at the resolved version ([`Amendment::base`]
//! says which version the copy was taken from), except for the unit being
//! built, whose declared base must be the version it is locked to. A
//! declaration whose copy is missing, a copy that is not declared, and a
//! copy of a file the provider does not have at the version resolved are
//! errors. Several members may amend one file of one provider when the
//! composer can combine their copies (JSON and JSONC by value, other text
//! by line): their copies are carried with their bases, and the combining
//! is the composer's business. When it cannot (a structured format without
//! a merge, or content that is not text), none is applied and the members
//! are reported together. Whether a base is the content the registry
//! records is verified by the build, not here.
//!
//! Every problem is reported, in the order [`compose`] documents.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use unicode_normalization::UnicodeNormalization;

use super::identity::UnitIdentity;
use super::manifest::{AmendmentDeclaration, Manifest};
use super::merge::MergeFormat;
use super::path::RelativePath;
use super::placeholder::{PlaceholderDeclaration, declarations};
use super::resolution::{Requirer, ResolvedGraph};
use super::text_merge::is_text;
use super::version::Version;

/// A unit's `src/` files, by path relative to `src/`.
pub type FileTree = BTreeMap<RelativePath, UnitFile>;

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
    /// A consumer project, whose explicit selections are the graph's root
    /// requirements.
    Project,
    /// A unit built from its working tree. Its manifest declares the graph's
    /// root requirements, and it is composed with its dependencies.
    Unit(UnitContent<'a>),
}

/// Composes `graph` for `root`.
///
/// `units` holds the content of every resolved unit at its resolved version;
/// entries for identities outside the graph are ignored. On failure every
/// problem is reported: each cause once, in this order, each kind sorted by
/// unit or path, and then by amender:
///
/// 1. content that does not match the resolved graph;
/// 2. provider files that several members amend, by provider and path;
/// 3. amendments that cannot be applied, and copies that are not declared,
///    by amender and then path;
/// 4. path collisions.
///
/// A declaration of a provider file that several members amend is not also
/// checked on its own. An amendment that cannot be applied leaves the
/// provider's file in place, so collisions are still found.
pub fn compose<'a>(
    graph: &ResolvedGraph,
    root: Root<'a>,
    units: &BTreeMap<UnitIdentity, UnitContent<'a>>,
) -> Result<Composition<'a>, Vec<CompositionError>> {
    let members = members(graph, root, units)?;
    let (files, errors) = compose_files(&members);
    if errors.is_empty() {
        Ok(Composition { files })
    } else {
        Err(errors)
    }
}

/// The result of composing a resolved graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composition<'a> {
    files: BTreeMap<RelativePath, ComposedFile<'a>>,
}

impl<'a> Composition<'a> {
    /// The composed file tree, by path.
    pub fn files(&self) -> &BTreeMap<RelativePath, ComposedFile<'a>> {
        &self.files
    }
}

/// A file in the composed tree and the unit that contributed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedFile<'a> {
    unit: &'a UnitIdentity,
    contents: &'a [u8],
    executable: bool,
    /// The copies that amend the file, by amender, in requirer order: the
    /// unit being built first, then by identity.
    amendments: Vec<Amendment<'a>>,
    /// The placeholder declarations in the file's scope.
    placeholders: Vec<PlaceholderDeclaration>,
}

/// One amender's copy of a provider's file, with the version the copy was
/// taken from. The provider may have resolved to another version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Amendment<'a> {
    amender: &'a UnitIdentity,
    copy: &'a UnitFile,
    base: &'a Version,
    below: BTreeSet<&'a UnitIdentity>,
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

    /// The other amenders of the same file in the amender's dependency
    /// closure: the amendments its copy was taken on top of. Amenders of
    /// the file outside it are its siblings, combined side by side.
    pub fn below(&self) -> &BTreeSet<&'a UnitIdentity> {
        &self.below
    }
}

impl<'a> ComposedFile<'a> {
    /// The unit whose `src/` provides the file.
    pub fn unit(&self) -> &'a UnitIdentity {
        self.unit
    }

    /// The provider's bytes. An amended file's copies are
    /// [`amendments`](Self::amendments).
    pub fn contents(&self) -> &'a [u8] {
        self.contents
    }

    /// Whether the file is executable. An amendment never changes it: the
    /// provider's bit stands.
    pub fn is_executable(&self) -> bool {
        self.executable
    }

    /// The copies that amend the file, with their amenders, in a fixed
    /// order. One copy takes the file's place; several are combined by the
    /// merge, by the composer.
    pub fn amendments(&self) -> &[Amendment<'a>] {
        &self.amendments
    }

    /// The placeholder declarations in the file's scope: those of its
    /// provider, of its amenders, and of the units each of them depends on,
    /// sorted by placeholder and then unit. Only these placeholders are
    /// reported in the file.
    pub fn placeholders(&self) -> &[PlaceholderDeclaration] {
        &self.placeholders
    }
}

/// A file a unit contributes, at its path in that unit's `src/`. Ordered by
/// path, then unit.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ContributedPath {
    path: RelativePath,
    unit: UnitIdentity,
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
}

impl fmt::Display for ContributedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} from {}", self.path, self.unit)
    }
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
    /// Several members amend the same file of the same provider, and the
    /// file is not one whose amendments can be combined (JSON, JSONC, and
    /// UTF-8 text, including YAML, TOML, and INI, are; XML is not, and
    /// neither is a file or copy that is not text). No amender takes
    /// precedence, so none is applied.
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
    /// The provider is not in the amending member's dependency closure, so
    /// the amender cannot have taken its copy from the provider's file.
    AmendmentUnreachable {
        /// The amending member.
        amender: Requirer,
        /// The amended path.
        path: RelativePath,
        /// The provider.
        unit: UnitIdentity,
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
    /// Files whose paths are equal or differ only in case or in Unicode
    /// normalization.
    PathCollision {
        /// Every colliding file, ordered by path and then unit.
        files: Vec<ContributedPath>,
    },
    /// A file's path is a directory of other files' paths, compared
    /// ignoring case and Unicode normalization.
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
                     text files, but not for XML files or a file that is not text, and no \
                     amender takes precedence unless each depends on the one before, so the \
                     file is not amended",
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
            Self::AmendmentUnreachable {
                amender,
                path,
                unit,
            } => match amender {
                Requirer::Root => write!(
                    f,
                    "amendments/{path} amends a file of {unit}, which the unit does not depend \
                     on, directly or through its dependencies"
                ),
                Requirer::Unit(..) => write!(
                    f,
                    "{amender} amends src/{path} of {unit}, which it does not depend on, \
                     directly or through its dependencies"
                ),
            },
            Self::AmendmentNotProvided {
                amender,
                path,
                unit,
                version,
            } => match amender {
                Requirer::Root => write!(
                    f,
                    "amendments/{path} amends a file that {unit} {version} does not provide: it \
                     has no src/{path}"
                ),
                Requirer::Unit(..) => write!(
                    f,
                    "amendments/{path} of {amender} amends a file that {unit} {version} does not \
                     provide: it has no src/{path}"
                ),
            },
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
            Self::PathCollision { files } => {
                f.write_str("path collision: ")?;
                write_list(f, files)?;
                let first = files.first().map(ContributedPath::path);
                let first_unit = files.first().map(ContributedPath::unit);
                let one_unit = files.iter().all(|file| Some(file.unit()) == first_unit);
                if files.iter().any(|file| Some(file.path()) != first) {
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
    /// How the unit is named as an amender.
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
) -> (
    Applied<'a>,
    BTreeMap<&'a UnitIdentity, BTreeSet<&'a UnitIdentity>>,
    Vec<CompositionError>,
) {
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
    let closure: BTreeMap<&'a UnitIdentity, BTreeSet<&'a UnitIdentity>> = members
        .keys()
        .map(|identity| {
            let below = dependency_closure(std::iter::once(*identity), |unit| {
                members
                    .get(unit)
                    .map(|member| member.manifest.dependencies())
            });
            (
                *identity,
                members
                    .keys()
                    .copied()
                    .filter(|unit| below.contains(*unit))
                    .collect(),
            )
        })
        .collect();
    let mut several = Vec::new();
    let mut contested = BTreeSet::new();
    for ((provider, path), requirers) in &amenders {
        let (Some(member), true) = (members.get(provider), requirers.len() > 1) else {
            continue;
        };
        // Several amendments of one file are combined by the composer
        // (OC-10): by value for JSON and JSONC, by line for any other file
        // whose provider file and copies are all text, and, whatever the
        // file, when the amenders form a chain, each above the one before,
        // where each copy is taken on top of the one below it. Anything
        // else could only be combined by letting one amender win, so none
        // applies.
        let chain = || {
            let mut identities: Vec<&UnitIdentity> = members
                .values()
                .filter(|member| requirers.contains(&&member.requirer))
                .map(|member| member.identity)
                .collect();
            identities.sort_by_key(|identity| closure.get(identity).map_or(0, BTreeSet::len));
            identities.windows(2).all(|pair| {
                closure
                    .get(pair[1])
                    .is_some_and(|below| below.contains(pair[0]))
            })
        };
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
        let combinable = match MergeFormat::of_path(path) {
            MergeFormat::Json => true,
            MergeFormat::Text => all_text(),
            MergeFormat::Unsupported => false,
        };
        if combinable || chain() {
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
            if let Some(problem) =
                amendment(members, &closure, member, path, declaration, &mut applied)
            {
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
    (applied, closure, several)
}

/// The identities reachable from `start` through `dependencies_of`,
/// `start` excluded unless a dependency leads back to it. An identity
/// without a known manifest ends the walk there.
pub fn dependency_closure<'a, I>(
    start: I,
    dependencies_of: impl Fn(&UnitIdentity) -> Option<&'a super::resolution::Dependencies>,
) -> BTreeSet<UnitIdentity>
where
    I: IntoIterator<Item = &'a UnitIdentity>,
{
    let mut pending: Vec<&'a UnitIdentity> = start
        .into_iter()
        .filter_map(&dependencies_of)
        .flat_map(|dependencies| dependencies.keys())
        .collect();
    let mut closure = BTreeSet::new();
    while let Some(identity) = pending.pop() {
        if !closure.insert(identity.clone()) {
            continue;
        }
        if let Some(dependencies) = dependencies_of(identity) {
            pending.extend(dependencies.keys());
        }
    }
    closure
}

/// Records the amendment of `path` that `member` declares, or why it cannot
/// be applied.
fn amendment<'a>(
    members: &BTreeMap<&'a UnitIdentity, Member<'a>>,
    closure: &BTreeMap<&'a UnitIdentity, BTreeSet<&'a UnitIdentity>>,
    member: &Member<'a>,
    path: &'a RelativePath,
    declaration: &'a AmendmentDeclaration,
    applied: &mut Applied<'a>,
) -> Option<CompositionError> {
    // The provider must be below the amender: a unit it depends on,
    // directly or through its dependencies, so a member of the graph.
    let reachable = members.get(declaration.unit()).filter(|provider| {
        closure
            .get(member.identity)
            .is_some_and(|below| below.contains(provider.identity))
    });
    let Some(provider) = reachable else {
        return Some(CompositionError::AmendmentUnreachable {
            amender: member.requirer.clone(),
            path: path.clone(),
            unit: declaration.unit().clone(),
        });
    };
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
        return Some(CompositionError::AmendmentNotProvided {
            amender: member.requirer.clone(),
            path: path.clone(),
            unit: provider.identity.clone(),
            version: resolved.clone(),
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
    let (applied, closure, mut errors) = amendments(members);
    // The declarations a file's contributor brings into its scope: its own
    // and those of every unit below it.
    let reach = |unit: &'a UnitIdentity| {
        std::iter::once(unit)
            .chain(closure.get(unit).into_iter().flatten().copied())
            .filter_map(|unit| members.get(unit).map(|member| member.manifest))
    };
    // Files by the collision key of their path.
    let mut folded: BTreeMap<String, Vec<ContributedPath>> = BTreeMap::new();
    let mut files = BTreeMap::new();
    for member in members.values() {
        for (path, file) in member.files {
            folded
                .entry(collision_key(path))
                .or_default()
                .push(ContributedPath {
                    path: path.clone(),
                    unit: member.identity.clone(),
                });
            let copies = applied.get(&(member.identity, path));
            let amendments = copies
                .map(|copies| {
                    copies
                        .iter()
                        .map(|(_, amender, copy, base)| Amendment {
                            amender,
                            copy,
                            base,
                            below: copies
                                .iter()
                                .map(|(_, other, _, _)| *other)
                                .filter(|other| {
                                    closure
                                        .get(*amender)
                                        .is_some_and(|below| below.contains(*other))
                                })
                                .collect(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let contributors = std::iter::once(member.identity).chain(
                copies
                    .into_iter()
                    .flatten()
                    .map(|(_, amender, _, _)| *amender),
            );
            let placeholders = declarations(contributors.flat_map(reach));
            files.insert(
                path.clone(),
                ComposedFile {
                    unit: member.identity,
                    contents: file.contents(),
                    executable: file.is_executable(),
                    amendments,
                    placeholders,
                },
            );
        }
    }

    let mut collisions: Vec<CompositionError> = folded
        .values()
        .filter(|group| group.len() > 1)
        .map(|group| {
            let mut files = group.clone();
            files.sort();
            CompositionError::PathCollision { files }
        })
        .collect();

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
        CompositionError, ContributedPath, FileTree, Root, UnitContent, UnitFile, compose,
    };
    use crate::domain::{
        AmendmentDeclaration, AvailableVersions, Dependencies, Description, Lock, Manifest,
        PlaceholderName, Placeholders, RelativePath, Requirer, UnitIdentity, Version, resolve,
    };

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    fn version(input: &str) -> Version {
        input.parse().unwrap()
    }

    fn path(input: &str) -> RelativePath {
        input.parse().unwrap()
    }

    fn unit(name: &str, resolved: &str) -> Requirer {
        Requirer::Unit(identity(name), version(resolved))
    }

    /// A unit for a test: its manifest fields and its `src/` files.
    #[derive(Clone)]
    struct Spec {
        name: &'static str,
        version: &'static str,
        dependencies: Vec<(&'static str, &'static str)>,
        /// Declared placeholders.
        placeholders: Vec<&'static str>,
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
                placeholders: Vec::new(),
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

        /// Declares `placeholder` for the consumer to replace.
        fn declares(mut self, placeholder: &'static str) -> Self {
            self.placeholders.push(placeholder);
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
            let placeholders = Placeholders::new(
                self.placeholders
                    .iter()
                    .map(|placeholder| {
                        (
                            PlaceholderName::parse(placeholder).unwrap(),
                            Description::new(&format!("{placeholder} of {}.", self.name)).unwrap(),
                        )
                    })
                    .collect(),
                Default::default(),
            )
            .unwrap();
            Manifest::new(
                identity(self.name),
                version(self.version),
                Description::new("A unit.").unwrap(),
                dependencies(&self.dependencies),
                placeholders,
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
        /// Path, unit, contents, and executable bit.
        files: Vec<(String, String, Vec<u8>, bool)>,
        /// The amended files: path and amending unit.
        amended: Vec<(String, String)>,
        /// The base version of each amendment: path and version.
        bases: Vec<(String, String)>,
        /// The placeholder declarations in each file's scope: path,
        /// placeholder, and declaring unit.
        placeholders: Vec<(String, String, String)>,
    }

    /// Resolves `published` for `root` and composes the result.
    fn run(root: &For, published: &[Spec]) -> Result<Composed, Vec<CompositionError>> {
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
        compose(resolution.graph(), root, &units).map(|composition| Composed {
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
            placeholders: composition
                .files()
                .iter()
                .flat_map(|(file, composed)| {
                    composed.placeholders().iter().map(|declaration| {
                        (
                            file.to_string(),
                            declaration.placeholder().to_string(),
                            declaration.unit().to_string(),
                        )
                    })
                })
                .collect(),
        })
    }

    fn contributed(file: &str, contributor: &str) -> ContributedPath {
        ContributedPath {
            path: path(file),
            unit: identity(contributor),
        }
    }

    fn project(selections: &[(&'static str, &'static str)]) -> For {
        For::Project(selections.to_vec())
    }

    fn node() -> Spec {
        Spec::new("@metamao/node", "1.0.0")
    }

    // Unit builds.

    #[test]
    fn a_unit_build_composes_the_built_unit_with_its_dependencies() {
        let built = Spec::new("@metamao/bun-app", "0.1.0")
            .depends("@metamao/node", "^1")
            .file("index.ts");
        let composed = run(&For::Build(built), &[node().file("package.json")]).unwrap();
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
        let composed = run(&project(&[("@metamao/app", "^1")]), &units).unwrap();
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
        let template = b"name: __TITLE__ {{ title }}\nrun: ${{ matrix.os }}\n".to_vec();
        let units = [Spec::new("@metamao/tools", "1.0.0")
            .declares("__TITLE__")
            .file_with("bin/run", binary.clone(), true)
            .file_with("ci.yml", template.clone(), false)];
        let composed = run(&project(&[("@metamao/tools", "^1")]), &units).unwrap();
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
        let errors = run(&For::Build(built), &[node().file("package.json")]).unwrap_err();
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
        let errors = run(&project(&[("@metamao/docs", "^1")]), &units).unwrap_err();
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

    fn provider() -> Vec<Spec> {
        vec![
            node()
                .file("package.json")
                .file_with("bin/run", b"#!/bin/sh".to_vec(), true),
            node().clone_as("1.1.0").file("package.json").file_with(
                "bin/run",
                b"#!/bin/sh".to_vec(),
                true,
            ),
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
            .amends("package.json", "@metamao/node", "1.0.0")
            .amends("bin/run", "@metamao/node", "1.0.0")
            .copy("package.json", "{ \"name\": \"acme\" }")
            .copy("bin/run", "#!/bin/sh\necho amended")
            .file("index.ts");
        let composed = run(&For::Build(built), &provider()).unwrap();
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
                    b"{ \"name\": \"acme\" }".as_slice(),
                    false
                ),
            ]
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
            // Not provided at all.
            .amends("missing.txt", "@metamao/node", "1.1.0")
            .copy("missing.txt", "")
            // Declared without a copy.
            .amends("package.json", "@metamao/node", "1.1.0")
            // A copy without a declaration.
            .copy("stray.txt", "")
            // The unit's own file at an amended path collides.
            .file("package.json");
        let errors = run(&For::Build(built), &provider()).unwrap_err();
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
                    path: path("missing.txt"),
                    unit: identity("@metamao/node"),
                    version: version("1.1.0"),
                },
                CompositionError::AmendmentMissingCopy {
                    amender: Requirer::Root,
                    path: path("package.json"),
                    provider: identity("@metamao/node"),
                },
                CompositionError::AmendmentUndeclared {
                    amender: Requirer::Root,
                    path: path("stray.txt"),
                },
                CompositionError::PathCollision {
                    files: vec![
                        contributed("package.json", "@metamao/app"),
                        contributed("package.json", "@metamao/node"),
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
            messages[1].ends_with("has no src/missing.txt"),
            "{}",
            messages[1]
        );
        assert!(messages[2].contains("does not exist"), "{}", messages[2]);
        assert!(messages[3].contains("not declared"), "{}", messages[3]);
    }

    /// `@metamao/amender` 1.0.0, a published unit amending two files of
    /// `@metamao/node` 1.0.0.
    fn amender() -> Spec {
        Spec::new("@metamao/amender", "1.0.0")
            .depends("@metamao/node", "=1.0.0")
            .amends("bin/run", "@metamao/node", "1.0.0")
            .copy("bin/run", "#!/bin/sh\necho amended by amender")
            .amends("package.json", "@metamao/node", "1.0.0")
            .copy("package.json", "{ \"name\": \"acme\" }")
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
        let composed = run(&For::Build(built), &published).unwrap();
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
                    b"{ \"name\": \"acme\" }".as_slice(),
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
        let composed = run(&For::Build(built), &published).unwrap();
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
        let errors = run(&For::Build(built), &provider()).unwrap_err();
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
        // Two published siblings amend `bin/run`, one of the copies not
        // being text, which is refused for every amender; two published
        // units amend `package.json`, a JSON file, whose
        // copies are carried for the merge; and a file only one member
        // amends still takes that copy.
        let other = Spec::new("@metamao/other", "1.0.0")
            .depends("@metamao/node", "=1.0.0")
            .amends("package.json", "@metamao/node", "1.0.0")
            .copy("package.json", "{}")
            .amends("README.md", "@metamao/node", "1.0.0")
            .copy("README.md", "amended readme")
            .file("other.txt");
        let other_binary = other
            .clone()
            .amends("bin/run", "@metamao/node", "1.0.0")
            .copy_bytes("bin/run", b"#!/bin/sh\0binary");
        let built = Spec::new("@metamao/app", "0.1.0")
            .depends("@metamao/amender", "^1")
            .depends("@metamao/other", "^1")
            .depends("@metamao/node", "=1.0.0")
            .file("index.ts");
        let mut published = provider();
        published[0] = published[0].clone().file("README.md");
        published.push(amender());
        let mut with_binary = published.clone();
        with_binary.push(other_binary);
        let errors = run(&For::Build(built.clone()), &with_binary).unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::SeveralAmenders {
                path: path("bin/run"),
                unit: identity("@metamao/node"),
                version: version("1.0.0"),
                amenders: vec![
                    Requirer::Unit(identity("@metamao/amender"), version("1.0.0")),
                    Requirer::Unit(identity("@metamao/other"), version("1.0.0")),
                ],
            }]
        );
        let message = errors[0].to_string();
        for part in [
            "src/bin/run of @metamao/node 1.0.0 is amended by @metamao/amender 1.0.0, \
             @metamao/other 1.0.0",
            "combined for JSON, JSONC, and UTF-8 text files, but not for XML files",
            "no amender takes precedence unless each depends on the one before",
        ] {
            assert!(message.contains(part), "{message}");
        }

        // The root, above the published amender, may amend the same file
        // with a copy that is not text: the amenders form a chain, and the
        // copy above is taken on top of the one below.
        published.push(other);
        let chained = built
            .clone()
            .amends("bin/run", "@metamao/node", "1.0.0")
            .copy_bytes("bin/run", b"#!/bin/sh\0binary");
        let composed = run(&For::Build(chained), &published).unwrap();
        assert_eq!(
            composed.amended,
            [
                ("README.md".to_owned(), "@metamao/other".to_owned()),
                ("bin/run".to_owned(), "@metamao/app".to_owned()),
                ("bin/run".to_owned(), "@metamao/amender".to_owned()),
                ("package.json".to_owned(), "@metamao/amender".to_owned()),
                ("package.json".to_owned(), "@metamao/other".to_owned()),
            ]
        );

        // Without the text file's amenders, the JSON file's copies are both
        // carried, in requirer order, with the provider's own contents.
        let built = built
            .amends("package.json", "@metamao/node", "1.0.0")
            .copy("package.json", "{ \"app\": true }");
        let composed = run(&For::Build(built), &published).unwrap();
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
        assert_eq!(package.2, b"@metamao/node package.json");
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
        let composed = run(&For::Build(built), &published).unwrap();
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
    fn one_unit_s_file_where_it_also_needs_a_directory_collides() {
        let units = [Spec::new("@metamao/docs", "1.0.0")
            .file("guide")
            .file("Guide/intro.md")];
        let errors = run(&project(&[("@metamao/docs", "^1")]), &units).unwrap_err();
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
        let errors = run(&project(&[("@metamao/docs", "^1")]), &units).unwrap_err();
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
        let errors = run(&project(&[("@metamao/docs", "^1")]), &units).unwrap_err();
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
            let errors = run(&project(&[("@metamao/docs", "^1")]), &units).unwrap_err();
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
            let composed = run(&project(&[("@metamao/docs", "^1")]), &units).unwrap();
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
        let composed = run(&project(&[("@metamao/docs", "^1")]), &units).unwrap();
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
            compose(resolution.graph(), Root::Project, &missing).unwrap_err(),
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
        let errors = compose(resolution.graph(), Root::Project, &wrong_version).unwrap_err();
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
            compose(resolution.graph(), built, &complete).unwrap_err(),
            [mismatch("@metamao/node", "1.0.0")]
        );
    }

    /// Units whose composition fails in every way at once.
    fn every_problem() -> (For, Vec<Spec>) {
        let units = vec![
            Spec::new("@metamao/api", "1.0.0")
                .depends("@metamao/node", "^1")
                .amends("README.md", "@metamao/node", "1.0.0")
                .copy("README.md", "api readme")
                .file("README.md")
                .file("docs"),
            Spec::new("@metamao/web", "1.0.0")
                .depends("@metamao/node", "^1")
                .amends("README.md", "@metamao/node", "1.0.0")
                .copy_bytes("README.md", b"web readme\0")
                .amends("missing.txt", "@metamao/node", "1.0.0")
                .copy("stray.txt", "stray")
                .file("readme.md")
                .file("docs/index.md"),
            node().file("README.md"),
        ];
        let root = project(&[("@metamao/api", "^1"), ("@metamao/web", "^1")]);
        (root, units)
    }

    #[test]
    fn every_problem_is_reported_in_a_stable_order() {
        let (root, units) = every_problem();
        let errors = run(&root, &units).unwrap_err();
        let kinds: Vec<String> = errors
            .iter()
            .map(|error| match error {
                CompositionError::ContentMismatch { unit, .. } => format!("mismatch {unit}"),
                CompositionError::SeveralAmenders { unit, path, .. } => {
                    format!("several {unit} {path}")
                }
                CompositionError::AmendmentBase { amender, path, .. }
                | CompositionError::AmendmentUnreachable { amender, path, .. }
                | CompositionError::AmendmentNotProvided { amender, path, .. }
                | CompositionError::AmendmentMissingCopy { amender, path, .. }
                | CompositionError::AmendmentUndeclared { amender, path, .. } => {
                    format!("amendment {amender} {path}")
                }
                CompositionError::PathCollision { files } => format!("collision {}", files[0]),
                CompositionError::NestedPathCollision { file, .. } => format!("nested {file}"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "several @metamao/node README.md",
                "amendment @metamao/web 1.0.0 missing.txt",
                "amendment @metamao/web 1.0.0 stray.txt",
                "collision README.md from @metamao/api",
                "nested docs from @metamao/api",
            ]
        );
    }

    /// A stack where two unrelated units declare `__NAME__`, a base
    /// declares `__PROJECT_NAME__`, and amendments bring declarations into
    /// a provider's file.
    fn placeholder_stack() -> (For, Vec<Spec>) {
        let units = vec![
            Spec::new("@acme/base", "1.0.0")
                .declares("__PROJECT_NAME__")
                .file("package.json")
                .file("README.md"),
            Spec::new("@acme/postgres", "1.0.0")
                .declares("__NAME__")
                .file("db/config.json"),
            Spec::new("@acme/docs", "1.0.0")
                .depends("@acme/base", "^1")
                .declares("__NAME__")
                .amends("README.md", "@acme/base", "1.0.0")
                .copy("README.md", "__PROJECT_NAME__ docs: __NAME__")
                .file("docs/site.json"),
            Spec::new("@acme/app", "1.0.0")
                .depends("@acme/docs", "^1")
                .depends("@acme/postgres", "^1")
                .declares("__APP_TITLE__")
                .file("index.html"),
        ];
        (project(&[("@acme/app", "^1")]), units)
    }

    #[test]
    fn a_file_s_scope_holds_its_contributors_declarations_and_their_graphs() {
        let (root, units) = placeholder_stack();
        let composed = run(&root, &units).unwrap();
        let scope = |file: &str| -> Vec<(&str, &str)> {
            composed
                .placeholders
                .iter()
                .filter(|(path, _, _)| path == file)
                .map(|(_, placeholder, unit)| (placeholder.as_str(), unit.as_str()))
                .collect()
        };
        // The base's own file sees the base alone; the amended one also the
        // amender and what it depends on.
        assert_eq!(scope("package.json"), [("__PROJECT_NAME__", "@acme/base")]);
        assert_eq!(
            scope("README.md"),
            [
                ("__NAME__", "@acme/docs"),
                ("__PROJECT_NAME__", "@acme/base")
            ]
        );
        // An unrelated unit's declaration of the same token stays out.
        assert_eq!(scope("db/config.json"), [("__NAME__", "@acme/postgres")]);
        assert_eq!(
            scope("index.html"),
            [
                ("__APP_TITLE__", "@acme/app"),
                ("__NAME__", "@acme/docs"),
                ("__NAME__", "@acme/postgres"),
                ("__PROJECT_NAME__", "@acme/base"),
            ]
        );
    }

    #[test]
    fn declaration_and_enumeration_order_never_change_the_outcome() {
        let succeeding = (
            project(&[("@metamao/bun-app", "^1"), ("@metamao/node", "^1")]),
            vec![
                Spec::new("@metamao/bun-app", "1.0.0")
                    .depends("@metamao/node", "^1")
                    .declares("__APP__")
                    .declares("__TITLE__")
                    .file("src/index.ts")
                    .file("bunfig.toml"),
                node()
                    .declares("__NAME__")
                    .file("package.json")
                    .file(".nvmrc"),
            ],
        );
        for (root, units) in [every_problem(), succeeding, placeholder_stack()] {
            let expected = run(&root, &units);
            let reorder = |spec: &Spec| {
                let mut spec = spec.clone();
                spec.dependencies.reverse();
                spec.placeholders.reverse();
                spec.files.reverse();
                spec.amendments.reverse();
                spec.copies.reverse();
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
                assert_eq!(run(&root, &shuffled), expected);
                assert_eq!(run(&reordered_root, &shuffled), expected);
            }
        }
    }
}
