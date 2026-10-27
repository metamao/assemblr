//! Composing the files of a composition: each amended file's copies
//! layered on the provider's file (see
//! `crate::adapters::layered` and `docs/domain/amendments-and-composition.md`).
//!
//! An amender's base is the composition below it at its own locked
//! versions. When they are the composition's versions, the base is composed
//! here from the same content; when they differ, for a published amender
//! whose provider or lower amenders the composition resolved to other
//! versions, the base is reconstructed from the published content at the
//! amender's locked versions, read from the registered repositories, and
//! the copy's changes are then applied to the composition as it is now
//! (across provider versions). A lower amender's own base follows the same
//! rule inside that reconstruction, so a base is always what its amender
//! saw when the copy was taken.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use super::{BuildProblem, CrossVersionReason, Dependencies};
use crate::adapters::layered::{Layer, Unresolved, layer_json, layer_text};
use crate::adapters::merge::{MergeFailure, MergeNote, unshared_notes};
use crate::domain::{
    ComposedFile, Composition, FileTree, MergeFormat, RelativePath, UnitFile, UnitIdentity,
    UnitLock, Version, dependency_closure, is_text,
};

/// The composed files of a composition.
pub(crate) struct Layered {
    pub(crate) files: Vec<(RelativePath, UnitFile)>,
    /// What the composition reports without failing, each with the
    /// delivered path it concerns.
    pub(crate) notes: Vec<(RelativePath, String)>,
    /// The amendments applied to another version of their provider than
    /// the one they were taken from, each with its path.
    pub(crate) crossed: Vec<(RelativePath, String)>,
    /// The files with a conflict between siblings that no amender above
    /// both resolved. Such a file is not among `files`.
    pub(crate) unresolved: Vec<UnresolvedFile>,
}

/// A file left with conflict markers: a contest between sibling amenders
/// that no unit above both resolved.
#[derive(Debug, Clone)]
pub struct UnresolvedFile {
    /// The path in the provider's `src/`, which is also the delivered one.
    pub(crate) path: RelativePath,
    /// The provider.
    pub(crate) provider: UnitIdentity,
    /// Its resolved version, when it is locked.
    pub(crate) version: Option<Version>,
    /// The amenders, by identity.
    pub(crate) amenders: Vec<UnitIdentity>,
    /// The conflicts, as the merge reports them.
    pub(crate) conflicts: Vec<Unresolved>,
    /// The file as layered, each conflict between markers.
    pub(crate) bytes: Vec<u8>,
    pub(crate) executable: bool,
}

impl std::fmt::Display for UnresolvedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let amenders: Vec<String> = self.amenders.iter().map(ToString::to_string).collect();
        write!(
            f,
            "src/{} of {}{}: the amendments of {} conflict, and no unit above them resolves \
             the conflict:",
            self.path,
            self.provider,
            self.version
                .as_ref()
                .map(|version| format!(" {version}"))
                .unwrap_or_default(),
            amenders.join(" and ")
        )?;
        for conflict in &self.conflicts {
            write!(f, "\n  {conflict}")?;
        }
        Ok(())
    }
}

/// Layers every file of `composition`: each amended file's copies on the
/// provider's file. Every problem is reported, by path and then position,
/// and the files are given only when there was none. `built` is the unit
/// being built, whose amendments are not published versions.
pub(crate) fn layer_files(
    composition: &Composition<'_>,
    lock: &UnitLock,
    built: Option<&UnitIdentity>,
    dependencies: &Dependencies,
    problems: &mut Vec<BuildProblem>,
) -> Option<Layered> {
    let worlds = Worlds::new(lock, built, dependencies);
    let mut layered = Layered {
        files: Vec::new(),
        notes: Vec::new(),
        crossed: Vec::new(),
        unresolved: Vec::new(),
    };
    let mut failed = false;
    for (path, composed) in composition.files() {
        let mut sink = Vec::new();
        let outcome = worlds.layer(path, composed, &mut sink);
        problems.extend(sink);
        let Some(outcome) = outcome else {
            failed = true;
            continue;
        };
        layered
            .notes
            .extend(outcome.notes.into_iter().map(|note| (path.clone(), note)));
        layered
            .crossed
            .extend(outcome.crossed.into_iter().map(|note| (path.clone(), note)));
        if outcome.unresolved.is_empty() {
            layered.files.push((
                path.clone(),
                UnitFile::new(outcome.bytes, composed.is_executable()),
            ));
        } else {
            layered.unresolved.push(UnresolvedFile {
                path: path.clone(),
                provider: composed.unit().clone(),
                version: lock
                    .units()
                    .get(composed.unit())
                    .map(|locked| locked.version().clone()),
                amenders: composed
                    .amendments()
                    .iter()
                    .map(|amendment| amendment.amender().clone())
                    .collect(),
                conflicts: outcome.unresolved,
                bytes: outcome.bytes,
                executable: composed.is_executable(),
            });
        }
    }
    (!failed).then_some(layered)
}

/// Layers the amended file at `path` alone: the provider's file and the
/// copies of every amender, with conflicts between siblings marked. This is
/// the base of a new amendment of the file by the unit being built, and the
/// copy the pending state creates.
pub(crate) fn layer_file(
    composition: &Composition<'_>,
    path: &RelativePath,
    lock: &UnitLock,
    built: Option<&UnitIdentity>,
    dependencies: &Dependencies,
    problems: &mut Vec<BuildProblem>,
) -> Option<Vec<u8>> {
    let composed = composition.files().get(path)?;
    let worlds = Worlds::new(lock, built, dependencies);
    worlds
        .layer(path, composed, problems)
        .map(|outcome| outcome.bytes)
}

/// The amended file: whose it is and where it lives.
#[derive(Clone, Copy)]
struct FileRef<'r> {
    provider: &'r UnitIdentity,
    path: &'r RelativePath,
}

impl<'r> FileRef<'r> {
    fn of(path: &'r RelativePath, composed: &ComposedFile<'r>) -> Self {
        Self {
            provider: composed.unit(),
            path,
        }
    }
}

/// What layering one file produced.
struct Outcome {
    bytes: Vec<u8>,
    notes: Vec<String>,
    crossed: Vec<String>,
    unresolved: Vec<Unresolved>,
}

/// Published content read while reconstructing bases, by version.
#[derive(Default)]
struct History {
    src: BTreeMap<(UnitIdentity, Version), FileTree>,
    amendments: BTreeMap<(UnitIdentity, Version), FileTree>,
    locks: BTreeMap<(UnitIdentity, Version), Option<UnitLock>>,
    /// Each reconstructed base, by amender, its version, and the path.
    bases: BTreeMap<(UnitIdentity, Version, RelativePath), Vec<u8>>,
}

/// The composition's world and the published worlds it reaches.
struct Worlds<'c> {
    lock: &'c UnitLock,
    built: Option<&'c UnitIdentity>,
    dependencies: &'c Dependencies,
    history: RefCell<History>,
}

/// One amender of a file in some world: its identity, its version there
/// when it is published, its copy, and the amenders of the file below it.
struct Amender<'w> {
    identity: &'w UnitIdentity,
    version: Option<Version>,
    copy: &'w [u8],
    /// The version its declaration names as the base.
    base: Version,
    below: BTreeSet<&'w UnitIdentity>,
}

impl<'c> Worlds<'c> {
    fn new(
        lock: &'c UnitLock,
        built: Option<&'c UnitIdentity>,
        dependencies: &'c Dependencies,
    ) -> Self {
        Self {
            lock,
            built,
            dependencies,
            history: RefCell::new(History::default()),
        }
    }

    /// The version of `amender` whose published content holds its copy, or
    /// `None` when the copy is the working tree's: the unit being built,
    /// or a unit of its catalog. A copy of the working tree is on the
    /// composition below it here, as `unit amend` takes it and `unit build`
    /// keeps it in a catalog by rebasing it onto its stored base's
    /// successor (see `bases`).
    fn published_version(&self, amender: &UnitIdentity) -> Option<Version> {
        (Some(amender) != self.built && !self.dependencies.is_sibling(amender))
            .then(|| self.lock.units().get(amender))
            .flatten()
            .map(|locked| locked.version().clone())
    }

    /// The published lockfile of `unit` at `version`, read once.
    fn lock_at(&self, unit: &UnitIdentity, version: &Version) -> Option<UnitLock> {
        let key = (unit.clone(), version.clone());
        if let Some(lock) = self.history.borrow().locks.get(&key) {
            return lock.clone();
        }
        let lock = self
            .dependencies
            .read_lock_at(unit, version)
            .and_then(|read| read.ok().flatten());
        self.history.borrow_mut().locks.insert(key, lock.clone());
        lock
    }

    /// The `src/` files of `unit` at `version`, read once.
    fn src_at(
        &self,
        unit: &UnitIdentity,
        version: &Version,
        sink: &mut Vec<BuildProblem>,
    ) -> Option<FileTree> {
        let key = (unit.clone(), version.clone());
        if let Some(tree) = self.history.borrow().src.get(&key) {
            return Some(tree.clone());
        }
        match self.dependencies.read_src_at(unit, version) {
            Some(Ok(tree)) => {
                self.history.borrow_mut().src.insert(key, tree.clone());
                Some(tree)
            }
            Some(Err(problem)) => {
                sink.push(problem);
                None
            }
            None => None,
        }
    }

    /// The `amendments/` copies of `unit` at `version`, read once.
    fn amendments_at(
        &self,
        unit: &UnitIdentity,
        version: &Version,
        sink: &mut Vec<BuildProblem>,
    ) -> Option<FileTree> {
        let key = (unit.clone(), version.clone());
        if let Some(tree) = self.history.borrow().amendments.get(&key) {
            return Some(tree.clone());
        }
        match self.dependencies.read_amendments_at(unit, version) {
            Some(Ok(tree)) => {
                self.history
                    .borrow_mut()
                    .amendments
                    .insert(key, tree.clone());
                Some(tree)
            }
            Some(Err(problem)) => {
                sink.push(problem);
                None
            }
            None => None,
        }
    }

    /// Whether the base of `amender` at `version`, a published version, is
    /// the composition below it in the world `against` locks: its lockfile
    /// locks the provider, the amenders of the file below it there, and
    /// the amenders of the file in its own world, at the versions `against`
    /// records. A lower unit that amended the file then and no longer does
    /// is a difference like a changed one.
    fn agrees_at<'u>(
        &self,
        amender: &UnitIdentity,
        version: &Version,
        against: &UnitLock,
        file: FileRef<'_>,
        below: impl IntoIterator<Item = &'u UnitIdentity>,
    ) -> bool {
        let Some(world) = self.lock_at(amender, version) else {
            return false;
        };
        let Some(lower) = self.amenders_in(amender, version, &world, file) else {
            return false;
        };
        let mut units: BTreeSet<UnitIdentity> = BTreeSet::new();
        units.insert(file.provider.clone());
        units.extend(below.into_iter().cloned());
        units.extend(lower.iter().map(|(unit, _)| (*unit).clone()));
        Self::agrees(Some(&world), against, &units)
    }

    /// The amenders of `file` in `world`, the published lockfile of
    /// `amender` at `version`: the units it locks, among those the
    /// amender's dependencies reach there, whose manifest at the locked
    /// version amends the file. `None` when the amender's manifest is not
    /// known.
    fn amenders_in<'w>(
        &self,
        amender: &UnitIdentity,
        version: &Version,
        world: &'w UnitLock,
        file: FileRef<'_>,
    ) -> Option<Vec<(&'w UnitIdentity, &'w Version)>> {
        let manifest = self.dependencies.manifest(amender, version)?;
        let manifest_at = |unit: &UnitIdentity| {
            world
                .units()
                .get(unit)
                .and_then(|locked| self.dependencies.manifest(unit, locked.version()))
        };
        let reach = dependency_closure(std::iter::once(amender), |unit| {
            if unit == amender {
                Some(manifest.dependencies())
            } else {
                manifest_at(unit).map(|manifest| manifest.dependencies())
            }
        });
        Some(
            world
                .units()
                .iter()
                .filter(|(unit, _)| reach.contains(*unit))
                .filter(|(unit, locked)| {
                    self.dependencies
                        .manifest(unit, locked.version())
                        .and_then(|manifest| manifest.amendments().get(file.path))
                        .is_some_and(|declaration| declaration.unit() == file.provider)
                })
                .map(|(unit, locked)| (unit, locked.version()))
                .collect(),
        )
    }

    /// What a unit's own single copy of a JSON file loses if a unit above
    /// merges it: members and values are carried, comments and order are
    /// not. Said to the author now, while the copy stands whole.
    fn unshared_notes(
        &self,
        file: FileRef<'_>,
        amender: &Amender<'_>,
        provider_text: &[u8],
        copy: &[u8],
    ) -> Vec<String> {
        let mut notes = Vec::new();
        if Some(amender.identity) != self.built
            || MergeFormat::of_path(file.path) != MergeFormat::Json
        {
            return notes;
        }
        let (Ok(base), Ok(copy)) = (
            std::str::from_utf8(provider_text),
            std::str::from_utf8(copy),
        ) else {
            return notes;
        };
        for note in unshared_notes(base, amender.identity, copy) {
            let MergeNote::NotCarried { what, .. } = note else {
                continue;
            };
            notes.push(format!(
                "amendments/{} of {}: if another unit amends src/{} of {} too, {what} of this \
                 copy is not carried; the merge carries members and values, not comments or \
                 order",
                file.path, amender.identity, file.path, file.provider
            ));
        }
        notes
    }

    /// Whether `world`, an amender's published lockfile, locks `units` at
    /// the versions `against` records. A unit `world` does not lock is a
    /// difference.
    fn agrees<'u>(
        world: Option<&UnitLock>,
        against: &UnitLock,
        units: impl IntoIterator<Item = &'u UnitIdentity>,
    ) -> bool {
        let Some(world) = world else {
            return false;
        };
        units.into_iter().all(
            |unit| match (world.units().get(unit), against.units().get(unit)) {
                (Some(theirs), Some(ours)) => theirs.version() == ours.version(),
                _ => false,
            },
        )
    }

    /// Layers the amendments of `composed`, delivered at `path`, in the
    /// composition's world.
    fn layer(
        &self,
        path: &RelativePath,
        composed: &ComposedFile<'_>,
        sink: &mut Vec<BuildProblem>,
    ) -> Option<Outcome> {
        let file = FileRef::of(path, composed);
        let provider = composed.unit();
        let resolved = self
            .lock
            .units()
            .get(provider)
            .map(|locked| locked.version().clone());
        let amenders: Vec<Amender<'_>> = composed
            .amendments()
            .iter()
            .map(|amendment| Amender {
                identity: amendment.amender(),
                version: self.published_version(amendment.amender()),
                copy: amendment.copy().contents(),
                base: amendment.base().clone(),
                below: amendment.below().clone(),
            })
            .collect();
        let mut crossed = Vec::new();
        for amender in &amenders {
            if resolved
                .as_ref()
                .is_some_and(|resolved| *resolved != amender.base)
            {
                crossed.push(format!(
                    "{path}: the amendment of {} was taken from {provider} {} and is applied to \
                     {provider} {}",
                    amender.identity,
                    amender.base,
                    resolved
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default()
                ));
            }
        }
        // Which amenders' bases must be reconstructed: those whose
        // published lockfile does not lock the provider and the amenders
        // below them at the composition's versions.
        let mut reconstructed: BTreeMap<&UnitIdentity, Vec<u8>> = BTreeMap::new();
        for amender in &amenders {
            let Some(version) = &amender.version else {
                // The unit being built is locked to what it amends, by
                // construction of the lock state.
                continue;
            };
            if self.agrees_at(
                amender.identity,
                version,
                self.lock,
                file,
                amender.below.iter().copied(),
            ) {
                continue;
            }
            let base = self.base_of(amender.identity, version, file, resolved.as_ref(), sink)?;
            reconstructed.insert(amender.identity, base);
        }
        let layered =
            self.layer_with(file, composed.contents(), &amenders, &reconstructed, sink)?;
        Some(Outcome {
            bytes: layered.bytes,
            notes: layered.notes,
            crossed,
            unresolved: layered.unresolved,
        })
    }

    /// Layers `amenders` on the provider's file of `composed`, each on its
    /// base: `reconstructed` where given, the composition below it here
    /// otherwise.
    fn layer_with(
        &self,
        file: FileRef<'_>,
        provider_bytes: &[u8],
        amenders: &[Amender<'_>],
        reconstructed: &BTreeMap<&UnitIdentity, Vec<u8>>,
        sink: &mut Vec<BuildProblem>,
    ) -> Option<Outcome> {
        let (provider, path) = (file.provider, file.path);
        let version = self
            .lock
            .units()
            .get(provider)
            .map(|locked| locked.version().clone());
        let provider_text = provider_bytes;
        if amenders.is_empty() {
            return Some(Outcome {
                bytes: provider_text.to_vec(),
                notes: Vec::new(),
                crossed: Vec::new(),
                unresolved: Vec::new(),
            });
        }
        let copies: Vec<&[u8]> = amenders.iter().map(|amender| amender.copy).collect();
        // One amender whose base is the file itself, or a base reconstructed
        // at other versions that is the file all the same: its copy takes
        // the file's place, byte for byte, whatever the format.
        if let ([amender], [copy]) = (amenders, copies.as_slice()) {
            let unchanged = reconstructed
                .get(amender.identity)
                .is_none_or(|base| base.as_slice() == provider_text);
            if unchanged {
                return Some(Outcome {
                    notes: self.unshared_notes(file, amender, provider_text, copy),
                    bytes: copy.to_vec(),
                    crossed: Vec::new(),
                    unresolved: Vec::new(),
                });
            }
        }
        // Amenders in a chain, each above the one before, with each copy
        // taken on top of the composition below it: the top copy is the
        // file, whatever the format.
        let mut ordered: Vec<(&Amender<'_>, &&[u8])> = amenders.iter().zip(&copies).collect();
        ordered.sort_by_key(|(amender, _)| amender.below.len());
        let chain = ordered
            .windows(2)
            .all(|pair| pair[1].0.below.contains(pair[0].0.identity))
            && reconstructed.is_empty();
        if chain {
            if let Some((_, top)) = ordered.last() {
                return Some(Outcome {
                    bytes: top.to_vec(),
                    notes: Vec::new(),
                    crossed: Vec::new(),
                    unresolved: Vec::new(),
                });
            }
        }
        let merge = |failure: Option<MergeFailure>| BuildProblem::Merge {
            unit: provider.clone(),
            version: version.clone(),
            path: file.path.clone(),
            amenders: amenders
                .iter()
                .map(|amender| amender.identity.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            crossed: amenders
                .iter()
                .filter(|amender| {
                    version
                        .as_ref()
                        .is_some_and(|version| *version != amender.base)
                })
                .map(|amender| (amender.identity.clone(), amender.base.clone()))
                .collect(),
            failure,
        };
        let format = MergeFormat::of_path(path);
        let all_text = is_text(provider_text)
            && copies.iter().all(|copy| is_text(copy))
            && reconstructed.values().all(|base| is_text(base));
        if format == MergeFormat::Unsupported || !all_text {
            // A format without a merge, or content that is not text, can
            // be layered only when one amender takes the file whole, above.
            // Across versions it is what a cross-version refusal reports.
            match amenders {
                [amender] => sink.push(BuildProblem::CrossVersion {
                    unit: provider.clone(),
                    path: file.path.clone(),
                    amender: amender.identity.clone(),
                    published: amender.version.clone(),
                    base: amender.base.clone(),
                    resolved: version.clone().unwrap_or_else(|| amender.base.clone()),
                    reason: CrossVersionReason::NoMerge,
                }),
                _ => sink.push(merge(None)),
            }
            return None;
        }
        let texts: Vec<(&str, Option<&str>)> = amenders
            .iter()
            .zip(&copies)
            .map(|(amender, copy)| {
                let copy = std::str::from_utf8(copy).ok();
                let base = reconstructed
                    .get(amender.identity)
                    .and_then(|base| std::str::from_utf8(base).ok());
                (copy, base)
            })
            .filter_map(|(copy, base)| copy.map(|copy| (copy, base)))
            .collect();
        let Ok(provider_str) = std::str::from_utf8(provider_text) else {
            sink.push(merge(None));
            return None;
        };
        if texts.len() != amenders.len() {
            sink.push(merge(None));
            return None;
        }
        let layers: Vec<Layer<'_>> = amenders
            .iter()
            .zip(&texts)
            .map(|(amender, (copy, base))| Layer {
                amender: amender.identity,
                copy,
                below: amender.below.clone(),
                base: *base,
            })
            .collect();
        let layered = match format {
            MergeFormat::Json => layer_json(provider_str, &layers),
            MergeFormat::Text => layer_text(provider, provider_str, &layers),
            MergeFormat::Unsupported => unreachable!("refused above"),
        };
        match layered {
            Ok(layered) => Some(Outcome {
                notes: layered
                    .merged
                    .notes()
                    .iter()
                    .map(|note| format!("src/{} of {provider}: {note}", file.path))
                    .collect(),
                bytes: layered.merged.into_text().into_bytes(),
                crossed: Vec::new(),
                unresolved: layered.unresolved,
            }),
            Err(failure) => {
                sink.push(merge(Some(failure)));
                None
            }
        }
    }

    /// The base of the amendment of `composed` by `amender` at `version`,
    /// a published version, reconstructed at the versions its lockfile
    /// records: the provider's file at the declared base, composed with the
    /// amenders of the file below `amender` there, each at the version
    /// locked, on its own base by the same rule.
    fn base_of(
        &self,
        amender: &UnitIdentity,
        version: &Version,
        file: FileRef<'_>,
        resolved: Option<&Version>,
        sink: &mut Vec<BuildProblem>,
    ) -> Option<Vec<u8>> {
        let (provider, source) = (file.provider, file.path);
        let key = (amender.clone(), version.clone(), source.clone());
        if let Some(base) = self.history.borrow().bases.get(&key) {
            return Some(base.clone());
        }
        let manifest = self.dependencies.manifest(amender, version)?;
        let declared = manifest.amendments().get(source)?.base().clone();
        let cross = |reason| BuildProblem::CrossVersion {
            unit: provider.clone(),
            path: source.clone(),
            amender: amender.clone(),
            published: Some(version.clone()),
            base: declared.clone(),
            resolved: resolved.cloned().unwrap_or_else(|| declared.clone()),
            reason,
        };
        let Some(world) = self.lock_at(amender, version) else {
            sink.push(cross(CrossVersionReason::BaseUnavailable));
            return None;
        };
        let Some(provider_file) = self
            .src_at(provider, &declared, sink)
            .and_then(|tree| tree.get(source).cloned())
        else {
            sink.push(cross(CrossVersionReason::BaseUnavailable));
            return None;
        };
        // The amenders of the file in the amender's world.
        let manifest_at = |unit: &UnitIdentity| {
            world
                .units()
                .get(unit)
                .and_then(|locked| self.dependencies.manifest(unit, locked.version()))
        };
        let candidates = self.amenders_in(amender, version, &world, file)?;
        let mut lower: Vec<Amender<'_>> = Vec::new();
        let mut copies: Vec<FileTree> = Vec::new();
        for (unit, locked) in &candidates {
            match self.amendments_at(unit, locked, sink) {
                Some(tree) if tree.contains_key(source) => copies.push(tree),
                _ => {
                    sink.push(cross(CrossVersionReason::BaseUnavailable));
                    return None;
                }
            }
        }
        for ((unit, locked), tree) in candidates.iter().zip(&copies) {
            let Some(copy) = tree.get(source) else {
                continue;
            };
            let below_unit = dependency_closure(std::iter::once(*unit), |unit| {
                manifest_at(unit).map(|manifest| manifest.dependencies())
            });
            let declaration = self
                .dependencies
                .manifest(unit, locked)
                .and_then(|manifest| manifest.amendments().get(source))?;
            lower.push(Amender {
                identity: unit,
                version: Some((*locked).clone()),
                copy: copy.contents(),
                base: declaration.base().clone(),
                below: candidates
                    .iter()
                    .map(|(other, _)| *other)
                    .filter(|other| below_unit.contains(*other))
                    .collect(),
            });
        }
        let mut reconstructed: BTreeMap<&UnitIdentity, Vec<u8>> = BTreeMap::new();
        for amender_below in &lower {
            let Some(version_below) = &amender_below.version else {
                continue;
            };
            if self.agrees_at(
                amender_below.identity,
                version_below,
                &world,
                file,
                amender_below.below.iter().copied(),
            ) {
                continue;
            }
            let base = self.base_of(
                amender_below.identity,
                version_below,
                file,
                Some(&declared),
                sink,
            )?;
            reconstructed.insert(amender_below.identity, base);
        }
        let outcome =
            self.layer_with(file, provider_file.contents(), &lower, &reconstructed, sink)?;
        self.history
            .borrow_mut()
            .bases
            .insert(key, outcome.bytes.clone());
        Some(outcome.bytes)
    }
}
