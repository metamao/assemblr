//! `unit publish`: the version tag for the committed unit, and the registry
//! index updated with it (see `docs/domain/authoring-workflow.md` and
//! `docs/adr/0013-unit-check-and-publish.md`).
//!
//! A publication validates exactly what it publishes: the tree of the commit
//! at `HEAD`, read with the readers the registry and dependents use for
//! published versions, not the working tree. The working tree must be
//! committed and clean, so that what the author built is what is tagged.
//! Every check of `unit check` runs on that tree, and then the publication
//! rules: the lockfile is committed and current, no version of equal
//! precedence is published, and, when the registry registers this
//! repository, the registry would accept the new tag.
//!
//! Everything is decided before the tag is created. After it, only the
//! index update can fail, and that is reported as a partial outcome: the
//! tag stands, and `registry sync` recovers the index. A tag that already
//! points at `HEAD` is not created again, and the index update still runs,
//! which is how a failed one is retried.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use super::catalog::{in_unit, joint_lock};
use super::check::{CheckProblem, publication_checks};
use super::exclusive::BuildLock;
use super::{
    BuildProblem, Inputs, Joint, MANIFEST, ManifestFileError, PublishedContent, Sibling, Siblings,
};
use crate::adapters::git::{GitError, GitRepository, InvalidVersion, ReadFileError, ReadSrcError};
use crate::adapters::lockfile::{self, PublishedLockfileError};
use crate::adapters::registry::{
    Indexed, ReferenceError, Registry, RegistryFailure, TagProblem, local_reference,
    observe_tags as observe,
};
use crate::domain::{
    Excluded, ObjectId, ObservedTag, ObservedVersion, Provenance, RegistryError, SourceReference,
    UnitIdentity, UnitLock, Version,
};

/// Publishes the version the committed manifest of the unit whose
/// repository's top level is `root` declares, resolving its dependencies
/// from `registry` and updating the registry's index when it registers this
/// repository.
///
/// At a catalog's top level, every unit of the catalog is published at the
/// catalog's version, with one tag (see [`publish_catalog`]). In a unit's
/// directory in a catalog, the publication is refused: a catalog's units
/// are published together.
pub fn publish_unit(root: &Path, registry: &Registry) -> Result<Published, PublishFailure> {
    match super::locate_unit(root) {
        super::Location::Unit => {}
        super::Location::Catalog => return publish_catalog(root, registry),
        super::Location::CatalogUnit { catalog, .. } => {
            return Err(PublishFailure::Rejected(vec![
                PublishProblem::InCatalogUnit { catalog },
            ]));
        }
    }
    let manifest_path = root.join(MANIFEST);
    if let Err(error) = fs::symlink_metadata(&manifest_path) {
        if error.kind() == io::ErrorKind::NotFound {
            return Err(PublishFailure::Rejected(vec![PublishProblem::Check(
                Box::new(CheckProblem::Build(BuildProblem::Manifest(
                    ManifestFileError::Missing {
                        path: manifest_path,
                    },
                ))),
            )]));
        }
    }
    let rejected = |problem| PublishFailure::Rejected(vec![problem]);
    let build = |problem| PublishProblem::Check(Box::new(CheckProblem::Build(problem)));
    let repository = GitRepository::open(root)
        .map_err(|error| rejected(build(BuildProblem::Repository(error))))?;
    // Under the lock that serializes builds: an update may be rewriting the
    // files the clean-state check compares with `HEAD`.
    let git_dir = repository
        .git_directory()
        .map_err(|error| rejected(build(BuildProblem::Repository(error))))?;
    let _guard = BuildLock::acquire(&git_dir)
        .map_err(|error| rejected(build(BuildProblem::BuildLock(error))))?;
    match super::update::interrupted(&git_dir) {
        Ok(None) => {}
        Ok(Some(journal)) => {
            return Err(rejected(build(BuildProblem::UpdateInterrupted { journal })));
        }
        Err(error) => return Err(rejected(build(BuildProblem::GitDirectory(error)))),
    }
    let Some(head) = repository
        .head_commit()
        .map_err(|error| rejected(build(BuildProblem::Repository(error))))?
    else {
        return Err(rejected(PublishProblem::NoCommit));
    };

    let mut problems = Vec::new();
    match repository.uncommitted_paths(root) {
        Ok(paths) if paths.is_empty() => {}
        Ok(paths) => problems.push(PublishProblem::Uncommitted { paths }),
        Err(error) => problems.push(build(BuildProblem::Repository(error))),
    }
    // The committed content, read as the registry and dependents read a
    // published version.
    let manifest = match repository.read_published(&[&head]) {
        Ok(mut read) => match read.pop() {
            Some(Ok(published)) => Some(published),
            Some(Err(invalid)) => {
                problems.push(PublishProblem::CommittedManifest(invalid));
                None
            }
            None => None,
        },
        Err(error) => {
            problems.push(build(BuildProblem::Repository(error)));
            None
        }
    };
    let src = match repository.read_src(&head) {
        Ok(src) => Some(src),
        Err(ReadSrcError::Git(error)) => {
            problems.push(build(BuildProblem::Repository(error)));
            None
        }
        Err(error) => {
            problems.push(build(BuildProblem::Src(error)));
            None
        }
    };
    let amendments = match repository.read_amendments(&head) {
        Ok(amendments) => Some(amendments),
        Err(ReadSrcError::Git(error)) => {
            problems.push(build(BuildProblem::Repository(error)));
            None
        }
        Err(error) => {
            problems.push(build(BuildProblem::Amendments(error)));
            None
        }
    };
    let previous = match lockfile::read_published(&repository, &head) {
        Ok(previous) => Some(previous),
        Err(PublishedLockfileError::Read(ReadFileError::Git(error))) => {
            problems.push(build(BuildProblem::Repository(error)));
            None
        }
        Err(error) => {
            problems.push(PublishProblem::CommittedLockfile(error));
            None
        }
    };
    let index = registry
        .index()
        .map_err(|error| problems.push(build(BuildProblem::Registry(error))))
        .ok();
    let (Some(published), Some(previous), Some(index)) = (manifest, previous, index) else {
        return Err(PublishFailure::Rejected(problems));
    };
    let manifest = published.manifest();
    let identity = manifest.name().clone();
    let version = manifest.version().clone();
    let provenance = published.provenance().clone();
    let mut checks = Vec::new();
    let publishable = publication_checks(
        &Inputs {
            manifest,
            src: src.as_ref(),
            amendments: amendments.as_ref(),
            previous: &previous.clone().unwrap_or_default(),
            catalog: None,
        },
        previous.is_some(),
        &index,
        &mut checks,
    );
    problems.extend(
        checks
            .into_iter()
            .map(|problem| PublishProblem::Check(Box::new(problem))),
    );

    let tag = format!("v{version}");
    let Some(already) = check_tag(&repository, &version, &head, &mut problems) else {
        return Err(PublishFailure::Rejected(problems));
    };

    // The registry. When it registers this repository, the new tag must be
    // one it will index; that is checked now, so that a tag is never
    // created that the registry then rejects.
    let registration = match local_reference(root) {
        Ok(source) => match index.identities_of(&source).as_slice() {
            [registered] if **registered == identity => Registration::Here { source },
            [registered, ..] => {
                problems.push(PublishProblem::RegisteredAs {
                    source: source.clone(),
                    registered: (*registered).clone(),
                });
                Registration::None
            }
            [] => match index.units().get(&identity) {
                Some(unit) => Registration::Elsewhere {
                    source: unit.source().clone(),
                },
                None => Registration::None,
            },
        },
        Err(error) => {
            problems.push(PublishProblem::Reference(error));
            Registration::None
        }
    };
    if let Registration::Here { source } = &registration {
        match observe(&repository, &index, source) {
            Ok(mut observed) => {
                if !already {
                    observed.push(ObservedTag::new(
                        tag.clone(),
                        version.clone(),
                        Ok(ObservedVersion::new(identity.clone(), provenance.clone())),
                    ));
                }
                if let Err(errors) = index.sync(BTreeMap::from([(source.clone(), observed)])) {
                    problems.push(PublishProblem::Registry(errors));
                }
            }
            Err(error) => problems.push(build(BuildProblem::Repository(error))),
        }
    }
    if publishable.is_none() || !problems.is_empty() {
        return Err(PublishFailure::Rejected(problems));
    }

    if !already {
        repository
            .create_annotated_tag(root, &tag, &format!("{identity} {version}"), &head)
            .map_err(PublishFailure::Tag)?;
    }
    let mut published = Published {
        unit: identity,
        units: Vec::new(),
        version,
        tag,
        provenance,
        already,
        index: IndexOutcome::NotRegistered,
        notes: publishable
            .map(|publishable| publishable.notes)
            .unwrap_or_default(),
    };
    let source = match registration {
        Registration::Here { source } => source,
        Registration::Elsewhere { source } => {
            published.index = IndexOutcome::RegisteredElsewhere { source };
            return Ok(published);
        }
        Registration::None => return Ok(published),
    };
    match registry.index_source(&source) {
        Ok(Indexed::Indexed {
            added, excluded, ..
        }) => {
            let indexed = added
                .get(&published.unit)
                .is_some_and(|versions| versions.contains(&published.version));
            published.index = if indexed {
                IndexOutcome::Indexed { excluded }
            } else {
                IndexOutcome::AlreadyIndexed { excluded }
            };
            Ok(published)
        }
        Ok(Indexed::NotRegistered) => Ok(published),
        Err(error) => Err(PublishFailure::IndexNotUpdated {
            published: Box::new(published),
            error,
        }),
    }
}

/// Publishes every unit of the catalog whose top level is `root` at the
/// version its committed `catalog.jsonc` declares, with one tag,
/// `v<version>`, after validating each unit of the commit at `HEAD` with its
/// siblings as `unit check` does, under the catalog's one lockfile
/// (**Provisional (D-53)**). The registry's index is updated when it
/// registers this repository.
fn publish_catalog(root: &Path, registry: &Registry) -> Result<Published, PublishFailure> {
    let rejected = |problem| PublishFailure::Rejected(vec![problem]);
    let build = |problem| PublishProblem::Check(Box::new(CheckProblem::Build(problem)));
    let repository = GitRepository::open(root)
        .map_err(|error| rejected(build(BuildProblem::Repository(error))))?;
    let git_dir = repository
        .git_directory()
        .map_err(|error| rejected(build(BuildProblem::Repository(error))))?;
    let _guard = BuildLock::acquire(&git_dir)
        .map_err(|error| rejected(build(BuildProblem::BuildLock(error))))?;
    match super::update::interrupted(&git_dir) {
        Ok(None) => {}
        Ok(Some(journal)) => {
            return Err(rejected(build(BuildProblem::UpdateInterrupted { journal })));
        }
        Err(error) => return Err(rejected(build(BuildProblem::GitDirectory(error)))),
    }
    let Some(head) = repository
        .head_commit()
        .map_err(|error| rejected(build(BuildProblem::Repository(error))))?
    else {
        return Err(rejected(PublishProblem::NoCommit));
    };

    let mut problems = Vec::new();
    match repository.uncommitted_paths(root) {
        Ok(paths) if paths.is_empty() => {}
        Ok(paths) => problems.push(PublishProblem::Uncommitted { paths }),
        Err(error) => problems.push(build(BuildProblem::Repository(error))),
    }
    let catalog = match repository.read_catalog_at(&head) {
        Ok(Some(Ok(catalog))) => Some(catalog),
        Ok(Some(Err(invalid))) => {
            problems.push(PublishProblem::CommittedManifest(invalid));
            None
        }
        Ok(None) => {
            problems.push(PublishProblem::NotACatalog);
            None
        }
        Err(error) => {
            problems.push(build(BuildProblem::Repository(error)));
            None
        }
    };
    let previous = match lockfile::read_published(&repository, &head) {
        Ok(previous) => Some(previous),
        Err(PublishedLockfileError::Read(ReadFileError::Git(error))) => {
            problems.push(build(BuildProblem::Repository(error)));
            None
        }
        Err(error) => {
            problems.push(PublishProblem::CommittedLockfile(error));
            None
        }
    };
    let index = registry
        .index()
        .map_err(|error| problems.push(build(BuildProblem::Registry(error))))
        .ok();
    let (Some(catalog), Some(previous), Some(index)) = (catalog, previous, index) else {
        return Err(PublishFailure::Rejected(problems));
    };

    // Each unit's committed files, as dependents read them.
    let mut siblings = Siblings::default();
    let mut directories = BTreeMap::new();
    for unit in catalog.units() {
        let identity = unit.manifest().name().clone();
        let directory = identity.name().to_owned();
        let unit_repository = repository.clone().for_unit(&identity);
        let mut read =
            |files: Result<_, ReadSrcError>, kind: fn(ReadSrcError) -> BuildProblem| match files {
                Ok(files) => Some(files),
                Err(ReadSrcError::Git(error)) => {
                    problems.push(build(BuildProblem::Repository(error)));
                    None
                }
                Err(error) => {
                    problems.push(build(in_unit(&directory, kind(error))));
                    None
                }
            };
        let src = read(unit_repository.read_src(&head), BuildProblem::Src);
        let amendments = read(
            unit_repository.read_amendments(&head),
            BuildProblem::Amendments,
        );
        let (Some(src), Some(amendments)) = (src, amendments) else {
            continue;
        };
        directories.insert(identity.clone(), directory);
        siblings.units.insert(
            identity,
            Sibling {
                manifest: unit.manifest().clone(),
                content: PublishedContent {
                    files: src,
                    amendments,
                    lock: None,
                },
                provenance: unit.provenance().clone(),
            },
        );
    }
    if siblings.units.len() < catalog.units().len() {
        return Err(PublishFailure::Rejected(problems));
    }
    // Read for the catalog's top level, the lockfile holds every unit, at
    // this commit; the lockfile itself holds what they depend on in other
    // repositories.
    let previous = previous.map(|lock| {
        UnitLock::new(
            lock.units()
                .iter()
                .filter(|(identity, _)| !siblings.contains(identity))
                .map(|(identity, locked)| (identity.clone(), locked.clone()))
                .collect(),
        )
    });
    let joint = match joint_lock(&siblings, previous.as_ref(), &index) {
        Ok(joint) => joint,
        Err(found) => {
            problems.extend(found.into_iter().map(build));
            return Err(PublishFailure::Rejected(problems));
        }
    };
    for sibling in siblings.units.values_mut() {
        if !sibling.manifest.amendments().is_empty() {
            sibling.content.lock = Some(joint.lock.clone());
        }
    }
    match &previous {
        None => problems.push(PublishProblem::Check(Box::new(CheckProblem::NoLockfile))),
        Some(_) if !joint.changes.is_empty() => {
            problems.push(PublishProblem::Check(Box::new(
                CheckProblem::LockfileStale {
                    changes: joint.changes.clone(),
                },
            )));
        }
        Some(_) => {}
    }
    let mut notes = Vec::new();
    let mut publishable = true;
    for (identity, sibling) in &siblings.units {
        let directory = directories.get(identity).cloned().unwrap_or_default();
        let mut checks = Vec::new();
        let outcome = publication_checks(
            &Inputs {
                manifest: &sibling.manifest,
                src: Some(&sibling.content.files),
                amendments: Some(&sibling.content.amendments),
                previous: &joint.lock,
                catalog: Some(&Joint {
                    siblings: &siblings,
                    lock: &joint.lock,
                    changes: &BTreeMap::new(),
                }),
            },
            true,
            &index,
            &mut checks,
        );
        publishable = publishable && outcome.is_some();
        if let Some(outcome) = outcome {
            notes.extend(outcome.notes);
        }
        problems.extend(checks.into_iter().map(|problem| {
            PublishProblem::Check(Box::new(match problem {
                CheckProblem::Build(problem) => CheckProblem::Build(in_unit(&directory, problem)),
                problem => CheckProblem::InUnit {
                    directory: directory.clone(),
                    problem: Box::new(problem),
                },
            }))
        }));
    }

    let name = catalog.catalog().name().clone();
    let version = catalog.catalog().version().clone();
    let tag = format!("v{version}");
    let Some(already) = check_tag(&repository, &version, &head, &mut problems) else {
        return Err(PublishFailure::Rejected(problems));
    };
    let units: Vec<UnitIdentity> = catalog
        .units()
        .iter()
        .map(|unit| unit.manifest().name().clone())
        .collect();
    let registration = match local_reference(root) {
        Ok(source) if !index.identities_of(&source).is_empty() => Registration::Here { source },
        Ok(_) => match units.iter().find_map(|unit| index.units().get(unit)) {
            Some(unit) => Registration::Elsewhere {
                source: unit.source().clone(),
            },
            None => Registration::None,
        },
        Err(error) => {
            problems.push(PublishProblem::Reference(error));
            Registration::None
        }
    };
    if let Registration::Here { source } = &registration {
        match observe(&repository, &index, source) {
            Ok(mut observed) => {
                if !already {
                    observed.push(ObservedTag::of_catalog(
                        tag.clone(),
                        version.clone(),
                        Ok(catalog
                            .units()
                            .iter()
                            .map(|unit| {
                                ObservedVersion::new(
                                    unit.manifest().name().clone(),
                                    unit.provenance().clone(),
                                )
                            })
                            .collect()),
                    ));
                }
                if let Err(errors) = index.sync(BTreeMap::from([(source.clone(), observed)])) {
                    problems.push(PublishProblem::Registry(errors));
                }
            }
            Err(error) => problems.push(build(BuildProblem::Repository(error))),
        }
    }
    if !publishable || !problems.is_empty() {
        return Err(PublishFailure::Rejected(problems));
    }

    if !already {
        repository
            .create_annotated_tag(root, &tag, &format!("{name} {version}"), &head)
            .map_err(PublishFailure::Tag)?;
    }
    let provenance = catalog
        .units()
        .first()
        .map(|unit| unit.provenance().clone())
        .unwrap_or_else(|| Provenance::new(head.clone(), head.clone()));
    let mut published = Published {
        unit: name,
        units,
        version,
        tag,
        provenance,
        already,
        index: IndexOutcome::NotRegistered,
        notes,
    };
    let source = match registration {
        Registration::Here { source } => source,
        Registration::Elsewhere { source } => {
            published.index = IndexOutcome::RegisteredElsewhere { source };
            return Ok(published);
        }
        Registration::None => return Ok(published),
    };
    match registry.index_source(&source) {
        Ok(Indexed::Indexed {
            added, excluded, ..
        }) => {
            let indexed = published.units.iter().any(|unit| {
                added
                    .get(unit)
                    .is_some_and(|versions| versions.contains(&published.version))
            });
            published.index = if indexed {
                IndexOutcome::Indexed { excluded }
            } else {
                IndexOutcome::AlreadyIndexed { excluded }
            };
            Ok(published)
        }
        Ok(Indexed::NotRegistered) => Ok(published),
        Err(error) => Err(PublishFailure::IndexNotUpdated {
            published: Box::new(published),
            error,
        }),
    }
}

/// Checks that the tag of `version` can be created at `head`: a version is
/// published once, so a tag of this version at another commit, or one of
/// equal precedence, refuses the publication. Whether the tag already
/// points at `head`, or `None` when the tags cannot be read, which
/// `problems` reports.
fn check_tag(
    repository: &GitRepository,
    version: &Version,
    head: &ObjectId,
    problems: &mut Vec<PublishProblem>,
) -> Option<bool> {
    let tags = match repository.published_versions() {
        Ok(tags) => tags,
        Err(error) => {
            problems.push(PublishProblem::Check(Box::new(CheckProblem::Build(
                BuildProblem::Repository(error),
            ))));
            return None;
        }
    };
    let mut already = false;
    for existing in &tags {
        if existing.version().cmp_precedence(version) != std::cmp::Ordering::Equal {
            continue;
        }
        if existing.version() == version {
            if existing.commit() == Some(head) {
                already = true;
            } else {
                problems.push(PublishProblem::TagElsewhere {
                    tag: existing.name().to_owned(),
                    commit: existing.commit().cloned(),
                });
            }
        } else {
            problems.push(PublishProblem::EqualPrecedence {
                tag: existing.name().to_owned(),
                version: existing.version().clone(),
            });
        }
    }
    Some(already)
}

/// How the registry relates to the repository being published.
enum Registration {
    /// It registers this repository for the unit's identity.
    Here { source: SourceReference },
    /// It registers the identity from another reference.
    Elsewhere { source: SourceReference },
    /// It does not register the identity.
    None,
}

/// A published version.
#[derive(Debug)]
pub struct Published {
    unit: UnitIdentity,
    units: Vec<UnitIdentity>,
    version: Version,
    tag: String,
    provenance: Provenance,
    already: bool,
    index: IndexOutcome,
    notes: Vec<String>,
}

impl Published {
    /// The unit published, or the catalog's name for a catalog.
    pub fn unit(&self) -> &UnitIdentity {
        &self.unit
    }

    /// The units a catalog's publication published, in the order of their
    /// directories; empty for one unit.
    pub fn units(&self) -> &[UnitIdentity] {
        &self.units
    }

    /// The version published, as the committed manifest declares.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// The tag, `v<version>`.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// The tagged commit and its tree; for a catalog, the tree of its first
    /// unit's directory.
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    /// Whether the tag already pointed at the commit, so that none was
    /// created.
    pub fn already_published(&self) -> bool {
        self.already
    }

    /// What happened to the registry index.
    pub fn index(&self) -> &IndexOutcome {
        &self.index
    }

    /// What the composition reports without failing: parts of amendment
    /// copies the merge could not carry, each naming the file.
    pub fn notes(&self) -> &[String] {
        &self.notes
    }
}

/// What a publication did to the registry index.
#[derive(Debug)]
pub enum IndexOutcome {
    /// The version was indexed.
    Indexed {
        /// The repository's version tags excluded as invalid.
        excluded: Vec<Excluded<TagProblem>>,
    },
    /// The version was indexed already.
    AlreadyIndexed {
        /// The repository's version tags excluded as invalid.
        excluded: Vec<Excluded<TagProblem>>,
    },
    /// The registry registers the unit from another reference, so the
    /// version becomes discoverable when that source is synchronized.
    RegisteredElsewhere {
        /// The registered reference.
        source: SourceReference,
    },
    /// The registry does not register the unit.
    NotRegistered,
}

/// Why a publication failed.
#[derive(Debug)]
pub enum PublishFailure {
    /// The publication was rejected before anything was done. Every
    /// problem found is listed.
    Rejected(Vec<PublishProblem>),
    /// The tag could not be created. Nothing was changed.
    Tag(GitError),
    /// The tag was created, but the registry index could not be updated.
    /// `registry sync` indexes the version.
    IndexNotUpdated {
        /// The publication, whose tag stands.
        published: Box<Published>,
        /// Why the index was not updated.
        error: RegistryFailure,
    },
}

impl fmt::Display for PublishFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(problems) => {
                for (index, problem) in problems.iter().enumerate() {
                    if index > 0 {
                        f.write_str("\n")?;
                    }
                    problem.fmt(f)?;
                }
                Ok(())
            }
            Self::Tag(error) => write!(f, "could not create the tag: {error}"),
            Self::IndexNotUpdated { published, error } => write!(
                f,
                "published {} {} as tag {}, but the registry index was not updated: {error}",
                published.unit, published.version, published.tag
            ),
        }
    }
}

impl std::error::Error for PublishFailure {}

/// A problem that rejects a publication.
#[derive(Debug)]
pub enum PublishProblem {
    /// A problem `unit check` reports, on the committed content.
    Check(Box<CheckProblem>),
    /// `HEAD` names no commit.
    NoCommit,
    /// The working tree is not committed and clean.
    Uncommitted {
        /// The paths with uncommitted changes or untracked, unignored
        /// content, sorted.
        paths: Vec<String>,
    },
    /// The commit at `HEAD` has no valid manifest.
    CommittedManifest(InvalidVersion),
    /// The commit at `HEAD` has a lockfile that cannot be read.
    CommittedLockfile(PublishedLockfileError),
    /// The version's tag exists and points at another commit.
    TagElsewhere {
        /// The tag.
        tag: String,
        /// The commit it points at, or `None` for another kind of object.
        commit: Option<ObjectId>,
    },
    /// A published version has the same precedence as the version to
    /// publish, differing only in build metadata.
    EqualPrecedence {
        /// The tag.
        tag: String,
        /// Its version.
        version: Version,
    },
    /// The path cannot be a registry reference.
    Reference(ReferenceError),
    /// The registry registers this repository for another identity.
    RegisteredAs {
        /// This repository's reference.
        source: SourceReference,
        /// The identity it is registered for.
        registered: UnitIdentity,
    },
    /// The registry would not index the new tag.
    Registry(Vec<RegistryError<TagProblem>>),
    /// The directory is a unit's in a catalog, whose units are published
    /// together, from the catalog's top level.
    InCatalogUnit {
        /// The catalog's top level.
        catalog: std::path::PathBuf,
    },
    /// The commit at `HEAD` has no `catalog.jsonc`, which the working tree
    /// has.
    NotACatalog,
}

impl fmt::Display for PublishProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Check(problem) => problem.fmt(f),
            Self::NoCommit => f.write_str(
                "the repository has no commit; a published version is a tag on a commit, so \
                 commit the unit first",
            ),
            Self::Uncommitted { paths } => {
                write!(
                    f,
                    "the working tree is not committed and clean; a publication tags exactly \
                     what is committed, and {} path(s) differ from it or are untracked:",
                    paths.len()
                )?;
                for path in paths {
                    write!(f, "\n  {path}")?;
                }
                Ok(())
            }
            Self::CommittedManifest(invalid) => {
                write!(f, "the commit at `HEAD` cannot be published: {invalid}")
            }
            Self::CommittedLockfile(error) => {
                write!(f, "in the commit at `HEAD`, {error}")
            }
            Self::TagElsewhere {
                tag,
                commit: Some(commit),
            } => write!(
                f,
                "tag {tag} already exists and points at commit {commit}, not at `HEAD`; a version \
                 is published once, so publish the change as a new version"
            ),
            Self::TagElsewhere { tag, commit: None } => write!(
                f,
                "tag {tag} already exists and does not point at a commit; a version is published \
                 once, so publish the change as a new version"
            ),
            Self::EqualPrecedence { tag, version } => write!(
                f,
                "tag {tag} publishes version {version}, which has the same precedence; versions \
                 that differ only in build metadata cannot both be published"
            ),
            Self::Reference(error) => error.fmt(f),
            Self::RegisteredAs { source, registered } => write!(
                f,
                "the registry registers {source} as {registered}, not as the identity the \
                 manifest declares; publishing would make the registration invalid"
            ),
            Self::Registry(errors) => {
                f.write_str("the registry would not index the new version:")?;
                for error in errors {
                    write!(f, "\n  {error}")?;
                }
                Ok(())
            }
            Self::InCatalogUnit { catalog } => write!(
                f,
                "this unit is in the catalog at {}, whose units are published together at the \
                 catalog's version",
                catalog.display()
            ),
            Self::NotACatalog => f.write_str(
                "the commit at `HEAD` has no catalog.jsonc; commit it with the catalog's units",
            ),
        }
    }
}

impl std::error::Error for PublishProblem {}
