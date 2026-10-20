//! Dependency resolution: exactly one published version for every unit
//! identity that a set of root requirements reaches.
//!
//! [`resolve`] works on plain data: the root requirements, the published
//! versions available locally with each version's dependencies, and any
//! existing lock state. It either returns the resolved graph and how it
//! differs from the lock, or the errors that explain why it failed.
//!
//! # Policy
//!
//! - A locked version that satisfies every requirement on its identity is
//!   tried first. It changes only when it no longer satisfies them or when
//!   its own dependencies cannot be resolved with it. A newer published
//!   version never changes it.
//! - Otherwise candidates are tried from the highest matching version down,
//!   the provisional Alpha default (OC-16).
//! - Requirements match with Cargo's semantics. A pre-release matches only a
//!   requirement that names a pre-release of the same `major.minor.patch`,
//!   so it is never selected implicitly.
//! - An identity whose published versions include two that differ only in
//!   build metadata is ambiguous and is never selected, because SemVer gives
//!   such versions equal precedence.
//! - Identities are decided in dependency order: a unit is decided before
//!   the units any of its published versions depend on, so each decision
//!   sees every requirement on it. Ties, and identities that could depend on
//!   each other, are decided in identity order. When raising one unit's
//!   version would force another's down, the unit decided first keeps the
//!   higher version. This is a deterministic tie-breaking rule, not a claim
//!   that the result is optimal by any measure.
//!
//! # Errors
//!
//! A candidate is abandoned for another when it conflicts with other
//! requirements, depends on an identity no source provides or whose versions
//! are ambiguous, or closes a dependency cycle. A resolution exists exactly
//! when some combination of available versions satisfies every requirement
//! without such problems, and within its step limit the search finds one
//! whenever it exists.
//!
//! The one exception is a locked version that satisfies every requirement
//! but is not available locally. It was part of the previous resolution, so
//! replacing it quietly would let fresh resolution override the lock.
//! Resolution fails instead and reports that synchronization may be needed.
//!
//! When resolution fails, the errors describe the **preferred** candidates:
//! each identity takes its locked or highest satisfying version, and every
//! problem on that path is reported. Other candidates were tried too, but the
//! errors do not enumerate why each of them failed.
//!
//! # Search and its worst case
//!
//! The search backtracks with conflict-directed backjumping: after a
//! conflict it returns to the most recent decision that caused it, skipping
//! unrelated decisions. A requirement that no version can satisfy whatever
//! the other choices therefore fails at once. The worst case is still
//! exponential, the product of the candidate counts of the identities
//! involved in conflicts, so the search gives up after a fixed number of
//! steps and reports [`ResolutionError::StepLimitExceeded`].

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::identity::UnitIdentity;
use super::version::{Version, VersionRequirement};

/// Direct dependencies and their version requirements, as declared by a
/// unit version or by the root.
pub type Dependencies = BTreeMap<UnitIdentity, VersionRequirement>;

/// The published versions available locally: for each identity, each
/// published version and that version's dependencies.
///
/// An identity that is absent has no registered source. An identity mapped
/// to no versions has a source with no published version available locally.
pub type AvailableVersions = BTreeMap<UnitIdentity, BTreeMap<Version, Dependencies>>;

/// Existing lock state: the exact version recorded for each identity.
pub type Lock = BTreeMap<UnitIdentity, Version>;

/// How many steps the search may take before giving up. Realistic
/// compositions need a few per unit.
const STEP_LIMIT: usize = 100_000;

/// Resolves `requirements` against the `available` versions, preferring the
/// versions in `lock`.
///
/// `root_identity` is the identity of the unit being built, or `None` for a
/// consumer project. The root is never resolved from `available`, even when
/// older published versions of it are listed there, and a dependency on it
/// is a cycle.
///
/// A candidate is passed over when it conflicts with other requirements,
/// depends on an identity that no source provides or whose versions are
/// ambiguous, or closes a cycle. Resolution fails when no combination of
/// available versions avoids all of these, or when a locked version that
/// satisfies every requirement is not available locally. On failure the
/// errors explain why the preferred candidates, the locked or highest
/// versions, fail, in a deterministic order (see [`ResolutionError`]).
pub fn resolve(
    root_identity: Option<&UnitIdentity>,
    requirements: &Dependencies,
    available: &AvailableVersions,
    lock: &Lock,
) -> Result<Resolution, Vec<ResolutionError>> {
    Resolver::new(root_identity, requirements, available, lock).run(STEP_LIMIT)
}

/// A successful resolution: the resolved graph and how it differs from the
/// lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    graph: ResolvedGraph,
    changes: BTreeMap<UnitIdentity, LockChange>,
}

impl Resolution {
    /// The resolved graph.
    pub fn graph(&self) -> &ResolvedGraph {
        &self.graph
    }

    /// Every difference from the lock, by identity, so each change can be
    /// explained. Empty when the lock is kept exactly.
    pub fn changes(&self) -> &BTreeMap<UnitIdentity, LockChange> {
        &self.changes
    }
}

/// The units a resolution selected, each at exactly one version, with the
/// dependency edges between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGraph {
    root: Dependencies,
    units: BTreeMap<UnitIdentity, ResolvedUnit>,
}

impl ResolvedGraph {
    /// The root requirements: the edges from the root to its direct
    /// dependencies.
    pub fn root(&self) -> &Dependencies {
        &self.root
    }

    /// Every resolved unit, by identity. A unit shared by several dependents
    /// appears once.
    pub fn units(&self) -> &BTreeMap<UnitIdentity, ResolvedUnit> {
        &self.units
    }
}

/// One unit in a resolved graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUnit {
    version: Version,
    dependencies: Dependencies,
}

impl ResolvedUnit {
    /// The exact version selected.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// The direct dependencies that this version declares. Each one is also
    /// a resolved unit.
    pub fn dependencies(&self) -> &Dependencies {
        &self.dependencies
    }
}

/// How a resolution differs from the lock for one identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockChange {
    /// The identity was not locked and is now resolved to this version.
    Added(Version),
    /// The locked version was replaced.
    Changed {
        /// The locked version.
        from: Version,
        /// The resolved version.
        to: Version,
    },
    /// The identity was locked to this version and is no longer required.
    Retired(Version),
}

/// Who declared a requirement.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Requirer {
    /// The root requirements: the unit being built or the consumer project.
    Root,
    /// A specific version of a unit.
    Unit(UnitIdentity, Version),
}

impl fmt::Display for Requirer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root => f.write_str("the root"),
            Self::Unit(identity, version) => write!(f, "{identity} {version}"),
        }
    }
}

/// A version requirement on an identity and who declared it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    requirer: Requirer,
    range: VersionRequirement,
}

impl Requirement {
    /// Who declared the requirement.
    pub fn requirer(&self) -> &Requirer {
        &self.requirer
    }

    /// The required version range.
    pub fn range(&self) -> &VersionRequirement {
        &self.range
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} from {}", self.range, self.requirer)
    }
}

/// A problem that prevents resolution.
///
/// Except for [`Unavailable`](Self::Unavailable) and
/// [`StepLimitExceeded`](Self::StepLimitExceeded), an error describes the
/// preferred candidates after the search found that no combination of
/// available versions works. Requirements inside an error are ordered by
/// requirer, the root first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionError {
    /// No registered source provides a required identity.
    Missing {
        /// The identity no source provides.
        identity: UnitIdentity,
        /// The requirements on it.
        requirements: Vec<Requirement>,
    },
    /// A required identity has published versions that differ only in build
    /// metadata, so none of them can be chosen.
    Ambiguous {
        /// The identity with ambiguous versions.
        identity: UnitIdentity,
        /// Every version that shares its precedence with another, lowest
        /// first.
        versions: Vec<Version>,
        /// The requirements on the identity.
        requirements: Vec<Requirement>,
    },
    /// A locked version that satisfies every requirement is not available
    /// locally. Synchronizing the registry may make it available.
    Unavailable {
        /// The locked identity.
        identity: UnitIdentity,
        /// The locked version.
        version: Version,
        /// The requirements on the identity.
        requirements: Vec<Requirement>,
    },
    /// No available version satisfies every requirement on an identity.
    Incompatible {
        /// The identity whose requirements conflict.
        identity: UnitIdentity,
        /// The requirements that no available version satisfies together.
        requirements: Vec<Requirement>,
        /// Every version of the identity available locally, lowest first.
        available: Vec<Version>,
    },
    /// Some available version satisfies every requirement on an identity,
    /// but no combination of versions satisfies them together with the
    /// rest of the graph. Reported only when the preferred candidates show
    /// no more specific problem.
    Unresolved {
        /// The identity the preferred candidates could not settle.
        identity: UnitIdentity,
        /// The requirements on it.
        requirements: Vec<Requirement>,
    },
    /// The preferred versions depend on each other in a cycle.
    Cycle {
        /// The cycle, starting and ending with the same requirer.
        path: Vec<Requirer>,
    },
    /// The search gave up before finding a resolution or proving that none
    /// exists.
    StepLimitExceeded {
        /// The number of steps taken.
        steps: usize,
    },
}

impl ResolutionError {
    fn kind_rank(&self) -> u8 {
        match self {
            Self::Missing { .. } => 0,
            Self::Ambiguous { .. } => 1,
            Self::Unavailable { .. } => 2,
            Self::Incompatible { .. } => 3,
            Self::Unresolved { .. } => 4,
            Self::Cycle { .. } => 5,
            Self::StepLimitExceeded { .. } => 6,
        }
    }

    fn identity(&self) -> Option<&UnitIdentity> {
        match self {
            Self::Missing { identity, .. }
            | Self::Ambiguous { identity, .. }
            | Self::Unavailable { identity, .. }
            | Self::Incompatible { identity, .. }
            | Self::Unresolved { identity, .. } => Some(identity),
            Self::Cycle { .. } | Self::StepLimitExceeded { .. } => None,
        }
    }

    fn path(&self) -> &[Requirer] {
        match self {
            Self::Cycle { path } => path,
            _ => &[],
        }
    }

    /// Orders errors by kind, then by identity or cycle path. Two errors
    /// that compare equal describe the same problem.
    fn order(&self, other: &Self) -> Ordering {
        (self.kind_rank(), self.identity(), self.path()).cmp(&(
            other.kind_rank(),
            other.identity(),
            other.path(),
        ))
    }
}

fn write_list<T: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    items: &[T],
    separator: &str,
) -> fmt::Result {
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            f.write_str(separator)?;
        }
        write!(f, "{item}")?;
    }
    Ok(())
}

impl fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing {
                identity,
                requirements,
            } => {
                write!(f, "no registered source provides {identity} (required as ")?;
                write_list(f, requirements, ", ")?;
                f.write_str("); register a unit repository that provides it")
            }
            Self::Ambiguous {
                identity,
                versions,
                requirements,
            } => {
                write!(f, "the published versions of {identity} are ambiguous: ")?;
                write_list(f, versions, ", ")?;
                f.write_str(
                    " differ only in build metadata, which SemVer ignores when ordering \
                     versions, so none of them is selected (required as ",
                )?;
                write_list(f, requirements, ", ")?;
                f.write_str(")")
            }
            Self::Unavailable {
                identity,
                version,
                requirements,
            } => {
                write!(
                    f,
                    "{identity} is locked to {version}, which is not available locally \
                     (required as "
                )?;
                write_list(f, requirements, ", ")?;
                f.write_str("); synchronizing the registry may make it available")
            }
            Self::Incompatible {
                identity,
                requirements,
                available,
            } => {
                write!(
                    f,
                    "no available version of {identity} satisfies every requirement: "
                )?;
                write_list(f, requirements, ", ")?;
                if available.is_empty() {
                    f.write_str("; no versions are available locally")?;
                } else {
                    f.write_str("; available versions: ")?;
                    write_list(f, available, ", ")?;
                }
                f.write_str(
                    "; if a matching version has been published, synchronizing the \
                     registry may make it available",
                )
            }
            Self::Unresolved {
                identity,
                requirements,
            } => {
                write!(
                    f,
                    "no combination of available versions satisfies every requirement; \
                     the preferred versions could not settle {identity} (required as "
                )?;
                write_list(f, requirements, ", ")?;
                f.write_str(")")
            }
            Self::Cycle { path } => {
                f.write_str("dependency cycle: ")?;
                write_list(f, path, " -> ")
            }
            Self::StepLimitExceeded { steps } => write!(
                f,
                "dependency resolution gave up after {steps} steps; \
                 narrowing the version requirements reduces the combinations to explore"
            ),
        }
    }
}

impl std::error::Error for ResolutionError {}

/// The version chosen so far for each decided identity.
type Assignment<'a> = BTreeMap<&'a UnitIdentity, &'a Version>;

/// A requirement borrowed from the inputs. `requirer` is `None` for the
/// root.
#[derive(Clone, Copy)]
struct Demand<'a> {
    requirer: Option<(&'a UnitIdentity, &'a Version)>,
    range: &'a VersionRequirement,
}

/// Every requirement on each identity, given an assignment.
type Demands<'a> = BTreeMap<&'a UnitIdentity, Vec<Demand<'a>>>;

/// A node of the dependency graph: `None` is the root.
type Node<'a> = Option<&'a UnitIdentity>;

/// What the search must do next, given an assignment.
enum Step<'a> {
    /// Every required identity is decided and satisfied.
    Complete,
    /// Decide this identity, trying these candidates in order.
    Decide(&'a UnitIdentity, Vec<&'a Version>),
    /// The version chosen for this identity fails a requirement on it.
    Violated(&'a UnitIdentity),
    /// The chosen versions form this cycle, which starts and ends at the
    /// same node.
    Cycle(Vec<Node<'a>>),
    /// No available version satisfies every requirement on this identity.
    NoCandidate(&'a UnitIdentity),
    /// No source provides this identity.
    Missing(&'a UnitIdentity),
    /// This identity's published versions are ambiguous.
    Ambiguous(&'a UnitIdentity),
    /// This locked version satisfies every requirement but is not available.
    Unavailable(&'a UnitIdentity, &'a Version),
}

/// One decision in the search.
struct Frame<'a> {
    identity: &'a UnitIdentity,
    candidates: Vec<&'a Version>,
    current: usize,
    /// The decided units whose requirements limited `candidates`.
    requirers: BTreeSet<&'a UnitIdentity>,
    /// The earlier decisions blamed for the failures of earlier candidates.
    conflicts: BTreeSet<&'a UnitIdentity>,
}

enum Failure {
    /// Every combination of candidates was tried.
    Exhausted,
    /// A problem that backtracking must not avoid.
    Aborted(ResolutionError),
    StepLimit(usize),
}

struct Resolver<'a> {
    root_identity: Option<&'a UnitIdentity>,
    root: &'a Dependencies,
    available: &'a AvailableVersions,
    lock: &'a Lock,
    /// The position of each reachable identity in the decision order.
    order: BTreeMap<&'a UnitIdentity, usize>,
}

impl<'a> Resolver<'a> {
    fn new(
        root_identity: Option<&'a UnitIdentity>,
        root: &'a Dependencies,
        available: &'a AvailableVersions,
        lock: &'a Lock,
    ) -> Self {
        let order = decision_order(root_identity, root, available);
        Self {
            root_identity,
            root,
            available,
            lock,
            order,
        }
    }

    fn run(&self, step_limit: usize) -> Result<Resolution, Vec<ResolutionError>> {
        let mut errors = match self.search(step_limit) {
            Ok(assignment) => return Ok(self.resolution(&assignment)),
            Err(Failure::StepLimit(steps)) => {
                return Err(vec![ResolutionError::StepLimitExceeded { steps }]);
            }
            Err(Failure::Exhausted) => self.explain(),
            Err(Failure::Aborted(error)) => {
                let mut errors = self.explain();
                errors.push(error);
                errors
            }
        };
        // A stable sort keeps the explanation's version of a problem that
        // the search also reported.
        errors.sort_by(ResolutionError::order);
        errors.dedup_by(|later, earlier| later.order(earlier) == Ordering::Equal);
        Err(errors)
    }

    /// Searches for an assignment that satisfies every requirement without
    /// a cycle.
    fn search(&self, step_limit: usize) -> Result<Assignment<'a>, Failure> {
        let mut frames: Vec<Frame<'a>> = Vec::new();
        let mut assignment = Assignment::new();
        let nothing_skipped = BTreeSet::new();
        for _ in 0..step_limit {
            let demands = self.demands(&assignment);
            let conflict = match self.next_step(&assignment, &demands, &nothing_skipped, true) {
                Step::Complete => return Ok(assignment),
                Step::Decide(identity, candidates) => {
                    if let Some(&first) = candidates.first() {
                        assignment.insert(identity, first);
                        frames.push(Frame {
                            identity,
                            candidates,
                            current: 0,
                            requirers: requirers(&demands, identity, |_| true),
                            conflicts: BTreeSet::new(),
                        });
                    }
                    continue;
                }
                Step::Violated(identity) => {
                    let mut conflict = match assignment.get(identity) {
                        Some(&version) => {
                            requirers(&demands, identity, |range| !range.matches(version))
                        }
                        None => BTreeSet::new(),
                    };
                    conflict.insert(identity);
                    conflict
                }
                // The versions on the cycle create its edges, whatever is
                // decided later.
                Step::Cycle(path) => path.into_iter().flatten().collect(),
                Step::NoCandidate(identity)
                | Step::Missing(identity)
                | Step::Ambiguous(identity) => requirers(&demands, identity, |_| true),
                Step::Unavailable(identity, version) => {
                    return Err(Failure::Aborted(
                        self.unavailable(identity, version, &demands),
                    ));
                }
            };
            if !backjump(&mut frames, &mut assignment, conflict) {
                return Err(Failure::Exhausted);
            }
        }
        Err(Failure::StepLimit(step_limit))
    }

    /// Follows the preferred candidates without backtracking, reporting
    /// every problem on the way.
    fn explain(&self) -> Vec<ResolutionError> {
        let mut assignment = Assignment::new();
        let mut skipped = BTreeSet::new();
        let mut redecided = BTreeSet::new();
        let mut errors = Vec::new();
        let mut unsettled = None;
        loop {
            let demands = self.demands(&assignment);
            let identity = match self.next_step(&assignment, &demands, &skipped, false) {
                Step::Complete => break,
                Step::Decide(identity, candidates) => {
                    if let Some(&first) = candidates.first() {
                        assignment.insert(identity, first);
                    }
                    continue;
                }
                // Only identities that could depend on each other are
                // decided before every requirement on them is known. Such an
                // identity is decided again, once, with all of them.
                Step::Violated(identity) => match self.choose(identity, &demands) {
                    Step::Decide(_, candidates) if redecided.insert(identity) => {
                        if let Some(&first) = candidates.first() {
                            assignment.insert(identity, first);
                        }
                        self.prune(&mut assignment);
                        continue;
                    }
                    Step::Decide(..) => {
                        unsettled.get_or_insert(identity);
                        identity
                    }
                    Step::Unavailable(identity, version) => {
                        errors.push(self.unavailable(identity, version, &demands));
                        identity
                    }
                    _ => {
                        errors.push(self.incompatible(identity, &demands));
                        identity
                    }
                },
                Step::NoCandidate(identity) => {
                    errors.push(self.incompatible(identity, &demands));
                    identity
                }
                Step::Missing(identity) => {
                    errors.push(self.missing(identity, &demands));
                    identity
                }
                Step::Ambiguous(identity) => {
                    errors.push(self.ambiguous(identity, &demands));
                    identity
                }
                Step::Unavailable(identity, version) => {
                    errors.push(self.unavailable(identity, version, &demands));
                    identity
                }
                // Cycles are collected once the walk is over.
                Step::Cycle(_) => break,
            };
            skipped.insert(identity);
        }
        errors.extend(
            self.cycles(&assignment)
                .into_iter()
                .map(|path| self.cycle(&path, &assignment)),
        );
        // The search proved that no combination works, so the preferred
        // candidates have a problem. If they still settled on a version for
        // every identity but one, that identity is the explanation left.
        if errors.is_empty() {
            if let Some(identity) = unsettled {
                let demands = self.demands(&assignment);
                errors.push(ResolutionError::Unresolved {
                    identity: identity.clone(),
                    requirements: requirements(&demands, identity),
                });
            }
        }
        errors
    }

    /// Removes the assigned units that nothing in the assignment or the
    /// root requires any more.
    fn prune(&self, assignment: &mut Assignment<'a>) {
        let mut reachable = BTreeSet::new();
        let mut pending: Vec<&'a UnitIdentity> = self.root.keys().collect();
        while let Some(identity) = pending.pop() {
            if let Some(&version) = assignment.get(identity) {
                if reachable.insert(identity) {
                    pending.extend(
                        self.dependencies(identity, version)
                            .map(|(dependency, _)| dependency),
                    );
                }
            }
        }
        assignment.retain(|identity, _| reachable.contains(identity));
    }

    /// Every requirement on each identity, from the root and from the
    /// versions in `assignment`.
    fn demands(&self, assignment: &Assignment<'a>) -> Demands<'a> {
        let mut demands: Demands<'a> = BTreeMap::new();
        for (identity, range) in self.root {
            demands.entry(identity).or_default().push(Demand {
                requirer: None,
                range,
            });
        }
        for (&identity, &version) in assignment {
            for (dependency, range) in self.dependencies(identity, version) {
                demands.entry(dependency).or_default().push(Demand {
                    requirer: Some((identity, version)),
                    range,
                });
            }
        }
        demands
    }

    fn dependencies(
        &self,
        identity: &UnitIdentity,
        version: &Version,
    ) -> impl Iterator<Item = (&'a UnitIdentity, &'a VersionRequirement)> + use<'a> {
        self.available
            .get(identity)
            .and_then(|versions| versions.get(version))
            .into_iter()
            .flatten()
    }

    /// The next step: a violated requirement first, then a cycle when
    /// `detect_cycles` is set, then the next identity to decide.
    fn next_step(
        &self,
        assignment: &Assignment<'a>,
        demands: &Demands<'a>,
        skipped: &BTreeSet<&'a UnitIdentity>,
        detect_cycles: bool,
    ) -> Step<'a> {
        for (&identity, identity_demands) in demands {
            if skipped.contains(identity) {
                continue;
            }
            if let Some(&version) = assignment.get(identity) {
                if identity_demands
                    .iter()
                    .any(|demand| !demand.range.matches(version))
                {
                    return Step::Violated(identity);
                }
            }
        }
        if detect_cycles {
            if let Some(path) = self.cycles(assignment).into_iter().next() {
                return Step::Cycle(path);
            }
        }

        let next = demands
            .keys()
            .copied()
            .filter(|identity| {
                Some(*identity) != self.root_identity
                    && !assignment.contains_key(identity)
                    && !skipped.contains(identity)
            })
            .min_by_key(|identity| {
                (
                    self.order.get(identity).copied().unwrap_or(usize::MAX),
                    *identity,
                )
            });
        let Some(identity) = next else {
            return Step::Complete;
        };
        self.choose(identity, demands)
    }

    /// The candidates for `identity` given every requirement on it: the
    /// satisfying locked version first, then the other satisfying versions
    /// from the highest down.
    fn choose(&self, identity: &'a UnitIdentity, demands: &Demands<'a>) -> Step<'a> {
        let Some(versions) = self.available.get(identity) else {
            return Step::Missing(identity);
        };
        if !ambiguous_versions(versions).is_empty() {
            return Step::Ambiguous(identity);
        }
        let identity_demands = demands.get(identity).map_or(&[][..], Vec::as_slice);
        let satisfies = |version: &Version| {
            identity_demands
                .iter()
                .all(|demand| demand.range.matches(version))
        };

        let mut locked = None;
        if let Some(version) = self.lock.get(identity) {
            if satisfies(version) {
                match versions.get_key_value(version) {
                    Some((version, _)) => locked = Some(version),
                    None => return Step::Unavailable(identity, version),
                }
            }
        }
        let candidates: Vec<&'a Version> = locked
            .into_iter()
            .chain(
                versions
                    .keys()
                    .rev()
                    .filter(|version| Some(*version) != locked && satisfies(version)),
            )
            .collect();
        if candidates.is_empty() {
            Step::NoCandidate(identity)
        } else {
            Step::Decide(identity, candidates)
        }
    }

    fn missing(&self, identity: &UnitIdentity, demands: &Demands<'a>) -> ResolutionError {
        ResolutionError::Missing {
            identity: identity.clone(),
            requirements: requirements(demands, identity),
        }
    }

    fn ambiguous(&self, identity: &UnitIdentity, demands: &Demands<'a>) -> ResolutionError {
        ResolutionError::Ambiguous {
            identity: identity.clone(),
            versions: self
                .available
                .get(identity)
                .map(|versions| ambiguous_versions(versions).into_iter().cloned().collect())
                .unwrap_or_default(),
            requirements: requirements(demands, identity),
        }
    }

    fn unavailable(
        &self,
        identity: &UnitIdentity,
        version: &Version,
        demands: &Demands<'a>,
    ) -> ResolutionError {
        ResolutionError::Unavailable {
            identity: identity.clone(),
            version: version.clone(),
            requirements: requirements(demands, identity),
        }
    }

    fn incompatible(&self, identity: &UnitIdentity, demands: &Demands<'a>) -> ResolutionError {
        ResolutionError::Incompatible {
            identity: identity.clone(),
            requirements: requirements(demands, identity),
            available: self
                .available
                .get(identity)
                .map(|versions| versions.keys().cloned().collect())
                .unwrap_or_default(),
        }
    }

    fn cycle(&self, path: &[Node<'a>], assignment: &Assignment<'a>) -> ResolutionError {
        let path = path
            .iter()
            .map(
                |node| match node.and_then(|identity| assignment.get_key_value(identity)) {
                    Some((&identity, &version)) => {
                        Requirer::Unit(identity.clone(), version.clone())
                    }
                    None => Requirer::Root,
                },
            )
            .collect();
        ResolutionError::Cycle { path }
    }

    /// Every cycle in the graph of the root and the assigned versions, found
    /// by a depth-first walk from the root in identity order. An edge counts
    /// only when the assigned version satisfies its requirement, so only
    /// cycles that the versions really form are found.
    fn cycles(&self, assignment: &Assignment<'a>) -> Vec<Vec<Node<'a>>> {
        let successors = |node: Node<'a>| -> Vec<Node<'a>> {
            let dependencies: Vec<(&'a UnitIdentity, &'a VersionRequirement)> = match node {
                None => self.root.iter().collect(),
                Some(identity) => match assignment.get(identity) {
                    Some(&version) => self.dependencies(identity, version).collect(),
                    None => Vec::new(),
                },
            };
            let mut nodes: Vec<Node<'a>> = dependencies
                .into_iter()
                .filter_map(|(dependency, range)| {
                    if Some(dependency) == self.root_identity {
                        Some(None)
                    } else {
                        let &version = assignment.get(dependency)?;
                        range.matches(version).then_some(Some(dependency))
                    }
                })
                .collect();
            nodes.sort();
            nodes
        };

        let mut finished = BTreeSet::new();
        let mut stack = vec![(None, successors(None), 0)];
        let mut cycles = Vec::new();
        while let Some((node, next_nodes, position)) = stack.last_mut() {
            let Some(&next) = next_nodes.get(*position) else {
                finished.insert(*node);
                stack.pop();
                continue;
            };
            *position += 1;
            if let Some(start) = stack.iter().position(|(on_stack, _, _)| *on_stack == next) {
                let mut path: Vec<Node<'a>> = stack[start..]
                    .iter()
                    .map(|(on_stack, _, _)| *on_stack)
                    .collect();
                path.push(next);
                cycles.push(path);
            } else if !finished.contains(&next) {
                stack.push((next, successors(next), 0));
            }
        }
        cycles
    }

    fn resolution(&self, assignment: &Assignment<'a>) -> Resolution {
        let units: BTreeMap<UnitIdentity, ResolvedUnit> = assignment
            .iter()
            .map(|(&identity, &version)| {
                let dependencies = self
                    .dependencies(identity, version)
                    .map(|(dependency, range)| (dependency.clone(), range.clone()))
                    .collect();
                (
                    identity.clone(),
                    ResolvedUnit {
                        version: version.clone(),
                        dependencies,
                    },
                )
            })
            .collect();

        let mut changes = BTreeMap::new();
        for (identity, unit) in &units {
            match self.lock.get(identity) {
                None => {
                    changes.insert(identity.clone(), LockChange::Added(unit.version.clone()));
                }
                Some(locked) if *locked != unit.version => {
                    changes.insert(
                        identity.clone(),
                        LockChange::Changed {
                            from: locked.clone(),
                            to: unit.version.clone(),
                        },
                    );
                }
                Some(_) => {}
            }
        }
        for (identity, locked) in self.lock {
            if !units.contains_key(identity) {
                changes.insert(identity.clone(), LockChange::Retired(locked.clone()));
            }
        }

        Resolution {
            graph: ResolvedGraph {
                root: self.root.clone(),
                units,
            },
            changes,
        }
    }
}

/// The versions that share their SemVer precedence with another version in
/// `versions`, lowest first. Empty when the set is unambiguous.
fn ambiguous_versions(versions: &BTreeMap<Version, Dependencies>) -> Vec<&Version> {
    let ordered: Vec<&Version> = versions.keys().collect();
    // Versions of equal precedence are adjacent, because `Version` orders by
    // precedence first and build metadata last.
    ordered
        .iter()
        .enumerate()
        .filter(|(index, version)| {
            let equal = |other: Option<&&Version>| {
                other.is_some_and(|other| other.cmp_precedence(version) == Ordering::Equal)
            };
            equal(index.checked_sub(1).and_then(|before| ordered.get(before)))
                || equal(ordered.get(index + 1))
        })
        .map(|(_, version)| *version)
        .collect()
}

/// Returns to the most recent decision in `conflict` and moves it to its
/// next candidate. Decisions made after it are undone, because they did not
/// cause the conflict. When a decision runs out of candidates, its own
/// conflicts and requirers are blamed in turn. Returns `false` when no
/// decision is left to change.
fn backjump<'a>(
    frames: &mut Vec<Frame<'a>>,
    assignment: &mut Assignment<'a>,
    mut conflict: BTreeSet<&'a UnitIdentity>,
) -> bool {
    loop {
        let Some(level) = frames
            .iter()
            .rposition(|frame| conflict.contains(frame.identity))
        else {
            return false;
        };
        for frame in frames.drain(level + 1..) {
            assignment.remove(frame.identity);
        }
        let Some(frame) = frames.last_mut() else {
            return false;
        };
        conflict.remove(frame.identity);
        frame.conflicts.append(&mut conflict);
        if let Some(&next) = frame.candidates.get(frame.current + 1) {
            frame.current += 1;
            assignment.insert(frame.identity, next);
            return true;
        }
        let Some(frame) = frames.pop() else {
            return false;
        };
        assignment.remove(frame.identity);
        conflict = frame.conflicts;
        conflict.extend(frame.requirers);
    }
}

/// The decided units that declare requirements on `identity` whose range
/// passes `filter`.
fn requirers<'a>(
    demands: &Demands<'a>,
    identity: &UnitIdentity,
    filter: impl Fn(&VersionRequirement) -> bool,
) -> BTreeSet<&'a UnitIdentity> {
    demands
        .get(identity)
        .into_iter()
        .flatten()
        .filter(|demand| filter(demand.range))
        .filter_map(|demand| demand.requirer.map(|(requirer, _)| requirer))
        .collect()
}

/// The requirements on `identity`, ordered by requirer.
fn requirements(demands: &Demands<'_>, identity: &UnitIdentity) -> Vec<Requirement> {
    let mut requirements: Vec<Requirement> = demands
        .get(identity)
        .into_iter()
        .flatten()
        .map(|demand| Requirement {
            requirer: match demand.requirer {
                None => Requirer::Root,
                Some((identity, version)) => Requirer::Unit(identity.clone(), version.clone()),
            },
            range: demand.range.clone(),
        })
        .collect();
    requirements.sort_by(|a, b| a.requirer.cmp(&b.requirer));
    requirements
}

/// Orders the identities reachable from the root so that a unit comes
/// before every unit any of its published versions depends on.
///
/// Identities that could depend on each other have no such order; the
/// smallest of them is placed first and the walk continues. Ties go to the
/// smallest identity.
fn decision_order<'a>(
    root_identity: Option<&'a UnitIdentity>,
    root: &'a Dependencies,
    available: &'a AvailableVersions,
) -> BTreeMap<&'a UnitIdentity, usize> {
    let all_dependencies = |identity: &UnitIdentity| {
        available
            .get(identity)
            .into_iter()
            .flat_map(|versions| versions.values())
            .flat_map(|dependencies| dependencies.keys())
    };

    let mut reachable = BTreeSet::new();
    let mut pending: Vec<&'a UnitIdentity> = root.keys().collect();
    while let Some(identity) = pending.pop() {
        if Some(identity) != root_identity && reachable.insert(identity) {
            pending.extend(all_dependencies(identity));
        }
    }

    let mut successors: BTreeMap<&'a UnitIdentity, BTreeSet<&'a UnitIdentity>> = BTreeMap::new();
    let mut in_degree: BTreeMap<&'a UnitIdentity, usize> =
        reachable.iter().map(|&identity| (identity, 0)).collect();
    for &identity in &reachable {
        let targets = successors.entry(identity).or_default();
        for dependency in all_dependencies(identity) {
            if dependency != identity
                && reachable.contains(dependency)
                && targets.insert(dependency)
            {
                *in_degree.entry(dependency).or_default() += 1;
            }
        }
    }

    let mut remaining = reachable;
    let mut ready: BTreeSet<&'a UnitIdentity> = remaining
        .iter()
        .copied()
        .filter(|identity| in_degree.get(identity) == Some(&0))
        .collect();
    let mut order = BTreeMap::new();
    while let Some(identity) = ready.pop_first().or_else(|| remaining.first().copied()) {
        remaining.remove(identity);
        order.insert(identity, order.len());
        for &dependency in successors.get(identity).into_iter().flatten() {
            if remaining.contains(dependency) {
                if let Some(degree) = in_degree.get_mut(dependency) {
                    *degree = degree.saturating_sub(1);
                    if *degree == 0 {
                        ready.insert(dependency);
                    }
                }
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::{
        AvailableVersions, Dependencies, Lock, LockChange, Requirer, Resolution, ResolutionError,
        Resolver, resolve,
    };
    use crate::domain::{UnitIdentity, Version};

    /// A published unit version: identity, version, and dependencies.
    type Published<'s> = (&'s str, &'s str, &'s [(&'s str, &'s str)]);

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    fn version(input: &str) -> Version {
        input.parse().unwrap()
    }

    fn dependencies(entries: &[(&str, &str)]) -> Dependencies {
        entries
            .iter()
            .map(|(name, range)| (identity(name), range.parse().unwrap()))
            .collect()
    }

    fn lock(entries: &[(&str, &str)]) -> Lock {
        entries
            .iter()
            .map(|(name, locked)| (identity(name), version(locked)))
            .collect()
    }

    fn available(published: &[Published<'_>]) -> AvailableVersions {
        let mut available = AvailableVersions::new();
        for (name, published_version, published_dependencies) in published {
            available.entry(identity(name)).or_default().insert(
                version(published_version),
                dependencies(published_dependencies),
            );
        }
        available
    }

    fn resolve_root(
        root: &[(&str, &str)],
        published: &[Published<'_>],
        locked: &[(&str, &str)],
    ) -> Result<Resolution, Vec<ResolutionError>> {
        resolve(
            None,
            &dependencies(root),
            &available(published),
            &lock(locked),
        )
    }

    fn versions(resolution: &Resolution) -> Vec<(String, String)> {
        resolution
            .graph()
            .units()
            .iter()
            .map(|(name, unit)| (name.to_string(), unit.version().to_string()))
            .collect()
    }

    fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected
            .iter()
            .map(|(name, resolved)| (name.to_string(), resolved.to_string()))
            .collect()
    }

    fn unit(name: &str, resolved: &str) -> Requirer {
        Requirer::Unit(identity(name), version(resolved))
    }

    const SEVEN_UNITS: &[Published<'static>] = &[
        ("@metamao/docker", "1.0.0", &[]),
        ("@metamao/docker", "1.1.0", &[]),
        (
            "@metamao/docker-bun",
            "1.0.0",
            &[("@metamao/docker", "^1.0")],
        ),
        (
            "@metamao/docker-bun",
            "1.1.0",
            &[("@metamao/docker", "^1.1")],
        ),
        (
            "@metamao/angular-bun",
            "1.0.0",
            &[("@metamao/docker-bun", "^1.1")],
        ),
        (
            "@metamao/cloudflare-bun",
            "1.0.0",
            &[("@metamao/docker-bun", "^1.0")],
        ),
        (
            "@metamao/angular-ssr-trpc",
            "1.0.0",
            &[
                ("@metamao/angular-bun", "^1"),
                ("@metamao/cloudflare-bun", "^1"),
            ],
        ),
        (
            "@metamao/angular-i18n",
            "1.0.0",
            &[("@metamao/angular-ssr-trpc", "^1")],
        ),
        (
            "@metamao/neonpg-drizzle",
            "1.0.0",
            &[("@metamao/cloudflare-bun", "^1")],
        ),
        ("@metamao/unrelated", "1.0.0", &[]),
    ];

    const SEVEN_UNIT_ROOT: &[(&str, &str)] = &[
        ("@metamao/angular-i18n", "^1"),
        ("@metamao/neonpg-drizzle", "^1"),
    ];

    #[test]
    fn the_seven_unit_example_resolves_shared_units_once() {
        let resolution = resolve_root(SEVEN_UNIT_ROOT, SEVEN_UNITS, &[]).unwrap();
        assert_eq!(
            versions(&resolution),
            pairs(&[
                ("@metamao/angular-bun", "1.0.0"),
                ("@metamao/angular-i18n", "1.0.0"),
                ("@metamao/angular-ssr-trpc", "1.0.0"),
                ("@metamao/cloudflare-bun", "1.0.0"),
                ("@metamao/docker", "1.1.0"),
                ("@metamao/docker-bun", "1.1.0"),
                ("@metamao/neonpg-drizzle", "1.0.0"),
            ])
        );

        let graph = resolution.graph();
        assert_eq!(graph.root(), &dependencies(SEVEN_UNIT_ROOT));
        let dependents: Vec<String> = graph
            .units()
            .iter()
            .filter(|(_, unit)| {
                unit.dependencies()
                    .contains_key(&identity("@metamao/docker-bun"))
            })
            .map(|(name, _)| name.to_string())
            .collect();
        assert_eq!(
            dependents,
            ["@metamao/angular-bun", "@metamao/cloudflare-bun"]
        );
        assert!(
            resolution
                .changes()
                .values()
                .all(|change| matches!(change, LockChange::Added(_)))
        );
        assert_eq!(resolution.changes().len(), 7);
    }

    #[test]
    fn the_highest_matching_version_is_preferred() {
        let resolution = resolve_root(
            &[("@a/x", "^1.1")],
            &[
                ("@a/x", "1.0.0", &[]),
                ("@a/x", "1.1.0", &[]),
                ("@a/x", "1.4.2", &[]),
                ("@a/x", "2.0.0", &[]),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(versions(&resolution), pairs(&[("@a/x", "1.4.2")]));
    }

    #[test]
    fn a_satisfying_lock_is_kept_when_newer_versions_exist() {
        let published: &[Published<'_>] = &[
            ("@a/x", "1.0.0", &[("@a/y", "^1")]),
            ("@a/x", "1.5.0", &[("@a/y", "^1")]),
            ("@a/y", "1.0.0", &[]),
            ("@a/y", "1.9.0", &[]),
        ];
        let locked = [("@a/x", "1.0.0"), ("@a/y", "1.0.0")];
        let resolution = resolve_root(&[("@a/x", "^1")], published, &locked).unwrap();
        assert_eq!(versions(&resolution), pairs(&locked));
        assert!(resolution.changes().is_empty());
    }

    #[test]
    fn a_lock_changes_only_where_new_requirements_need_it() {
        let published: &[Published<'_>] = &[
            ("@a/kept", "1.0.0", &[]),
            ("@a/kept", "1.1.0", &[]),
            ("@a/moved", "1.0.0", &[]),
            ("@a/moved", "1.1.0", &[]),
            ("@a/moved", "1.2.0", &[]),
            ("@a/new", "1.0.0", &[("@a/moved", "^1.1")]),
            ("@a/old", "1.0.0", &[]),
        ];
        let resolution = resolve_root(
            &[("@a/kept", "^1"), ("@a/moved", "^1"), ("@a/new", "^1")],
            published,
            &[
                ("@a/kept", "1.0.0"),
                ("@a/moved", "1.0.0"),
                ("@a/old", "1.0.0"),
                ("@z/absent", "3.0.0"),
            ],
        )
        .unwrap();
        assert_eq!(
            versions(&resolution),
            pairs(&[
                ("@a/kept", "1.0.0"),
                ("@a/moved", "1.2.0"),
                ("@a/new", "1.0.0")
            ])
        );
        let expected: BTreeMap<UnitIdentity, LockChange> = [
            (
                identity("@a/moved"),
                LockChange::Changed {
                    from: version("1.0.0"),
                    to: version("1.2.0"),
                },
            ),
            (identity("@a/new"), LockChange::Added(version("1.0.0"))),
            (identity("@a/old"), LockChange::Retired(version("1.0.0"))),
            (identity("@z/absent"), LockChange::Retired(version("3.0.0"))),
        ]
        .into();
        assert_eq!(resolution.changes(), &expected);
    }

    #[test]
    fn a_satisfying_lock_changes_when_its_dependencies_conflict() {
        let resolution = resolve_root(
            &[("@a/app", "^1"), ("@a/lib", "^2")],
            &[
                ("@a/app", "1.0.0", &[("@a/lib", "^1")]),
                ("@a/app", "1.1.0", &[("@a/lib", "^2")]),
                ("@a/lib", "1.0.0", &[]),
                ("@a/lib", "2.0.0", &[]),
            ],
            &[("@a/app", "1.0.0"), ("@a/lib", "1.0.0")],
        )
        .unwrap();
        let expected: BTreeMap<UnitIdentity, LockChange> = [
            (
                identity("@a/app"),
                LockChange::Changed {
                    from: version("1.0.0"),
                    to: version("1.1.0"),
                },
            ),
            (
                identity("@a/lib"),
                LockChange::Changed {
                    from: version("1.0.0"),
                    to: version("2.0.0"),
                },
            ),
        ]
        .into();
        assert_eq!(resolution.changes(), &expected);
    }

    #[test]
    fn a_requirer_steps_down_when_its_highest_version_conflicts() {
        let resolution = resolve_root(
            &[("@a/app", "^1"), ("@a/lib", "^1")],
            &[
                ("@a/app", "1.0.0", &[("@a/lib", "^1")]),
                ("@a/app", "1.1.0", &[("@a/lib", "^2")]),
                ("@a/lib", "1.0.0", &[]),
                ("@a/lib", "2.0.0", &[]),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(
            versions(&resolution),
            pairs(&[("@a/app", "1.0.0"), ("@a/lib", "1.0.0")])
        );
    }

    #[test]
    fn backjumping_skips_decisions_unrelated_to_a_conflict() {
        let mut view = available(&[
            ("@b/a", "1.0.0", &[("@b/z", "^1")]),
            ("@b/a", "2.0.0", &[("@b/z", "^2")]),
            ("@b/z", "1.0.0", &[]),
        ]);
        let mut root = dependencies(&[("@b/a", "*")]);
        for index in 0..12 {
            let name = identity(&format!("@b/m{index:02}"));
            for minor in 0..4 {
                view.entry(name.clone())
                    .or_default()
                    .insert(version(&format!("1.{minor}.0")), Dependencies::new());
            }
            root.insert(name, "*".parse().unwrap());
        }

        // Revisiting every combination of the twelve unrelated units would
        // take millions of steps.
        let resolution = Resolver::new(None, &root, &view, &Lock::new())
            .run(100)
            .unwrap();
        let units = resolution.graph().units();
        assert_eq!(units[&identity("@b/a")].version(), &version("1.0.0"));
        assert_eq!(units[&identity("@b/m07")].version(), &version("1.3.0"));

        // A requirement no version satisfies fails without exploring them.
        let mut impossible = root.clone();
        impossible.insert(identity("@b/z"), "^3".parse().unwrap());
        let errors = Resolver::new(None, &impossible, &view, &Lock::new())
            .run(100)
            .unwrap_err();
        assert!(
            matches!(&errors[..], [ResolutionError::Incompatible { identity: name, .. }] if *name == identity("@b/z")),
            "{errors:?}"
        );
    }

    #[test]
    fn the_search_gives_up_after_its_step_limit() {
        let root = dependencies(&[("@a/app", "^1"), ("@a/lib", "^1")]);
        let view = available(&[
            ("@a/app", "1.0.0", &[("@a/lib", "^1")]),
            ("@a/app", "1.1.0", &[("@a/lib", "^2")]),
            ("@a/lib", "1.0.0", &[]),
        ]);
        let errors = Resolver::new(None, &root, &view, &Lock::new())
            .run(2)
            .unwrap_err();
        assert_eq!(errors, [ResolutionError::StepLimitExceeded { steps: 2 }]);
        let message = errors[0].to_string();
        assert!(message.contains("gave up after 2 steps"), "{message}");
    }

    #[test]
    fn compatible_diamond_ranges_share_one_version() {
        let resolution = resolve_root(
            &[("@d/left", "^1"), ("@d/right", "^1")],
            &[
                ("@d/left", "1.0.0", &[("@d/base", "^1.0")]),
                ("@d/right", "1.0.0", &[("@d/base", ">=1.2, <1.6")]),
                ("@d/base", "1.0.0", &[]),
                ("@d/base", "1.2.0", &[]),
                ("@d/base", "1.5.0", &[]),
                ("@d/base", "1.7.0", &[]),
                ("@d/base", "2.0.0", &[]),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(
            versions(&resolution),
            pairs(&[
                ("@d/base", "1.5.0"),
                ("@d/left", "1.0.0"),
                ("@d/right", "1.0.0")
            ])
        );
    }

    #[test]
    fn incompatible_diamond_ranges_name_both_requirers() {
        let errors = resolve_root(
            &[("@d/left", "^1"), ("@d/right", "^1")],
            &[
                ("@d/left", "1.0.0", &[("@d/base", "^1")]),
                ("@d/right", "1.0.0", &[("@d/base", "^2")]),
                ("@d/base", "1.0.0", &[]),
                ("@d/base", "2.0.0", &[]),
            ],
            &[],
        )
        .unwrap_err();
        let [
            ResolutionError::Incompatible {
                identity: name,
                requirements,
                available,
            },
        ] = &errors[..]
        else {
            panic!("expected one incompatibility, got {errors:?}");
        };
        assert_eq!(name, &identity("@d/base"));
        let described: Vec<(Requirer, String)> = requirements
            .iter()
            .map(|requirement| {
                (
                    requirement.requirer().clone(),
                    requirement.range().to_string(),
                )
            })
            .collect();
        assert_eq!(
            described,
            [
                (unit("@d/left", "1.0.0"), "^1".to_owned()),
                (unit("@d/right", "1.0.0"), "^2".to_owned()),
            ]
        );
        assert_eq!(available, &[version("1.0.0"), version("2.0.0")]);

        let message = errors[0].to_string();
        for expected in [
            "no available version of @d/base",
            "^1 from @d/left 1.0.0",
            "^2 from @d/right 1.0.0",
            "available versions: 1.0.0, 2.0.0",
        ] {
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn a_range_no_available_version_satisfies_is_incompatible() {
        let errors = resolve_root(
            &[("@a/x", "^2")],
            &[("@a/x", "1.0.0", &[]), ("@a/x", "1.9.0", &[])],
            &[],
        )
        .unwrap_err();
        let message = errors[0].to_string();
        assert!(message.contains("^2 from the root"), "{message}");
        assert!(
            message.contains("available versions: 1.0.0, 1.9.0"),
            "{message}"
        );
    }

    #[test]
    fn an_identity_without_published_versions_is_incompatible() {
        let root = dependencies(&[("@a/empty", "^1")]);
        let mut view = AvailableVersions::new();
        view.insert(identity("@a/empty"), BTreeMap::new());
        let errors = resolve(None, &root, &view, &Lock::new()).unwrap_err();
        assert_eq!(
            errors,
            [ResolutionError::Incompatible {
                identity: identity("@a/empty"),
                requirements: vec![super::Requirement {
                    requirer: Requirer::Root,
                    range: "^1".parse().unwrap(),
                }],
                available: Vec::new(),
            }]
        );
        let message = errors[0].to_string();
        assert!(
            message.contains("no versions are available locally"),
            "{message}"
        );
    }

    #[test]
    fn missing_identities_are_all_reported_with_their_requirers() {
        let errors = resolve_root(
            &[("@m/app", "^1"), ("@m/gone", "^1.2"), ("@m/nowhere", "*")],
            &[("@m/app", "1.0.0", &[("@m/gone", "^1")])],
            &[],
        )
        .unwrap_err();
        let summary: Vec<(String, Vec<Requirer>)> = errors
            .iter()
            .map(|error| match error {
                ResolutionError::Missing {
                    identity,
                    requirements,
                } => (
                    identity.to_string(),
                    requirements
                        .iter()
                        .map(|requirement| requirement.requirer().clone())
                        .collect(),
                ),
                other => panic!("expected only missing identities, got {other:?}"),
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "@m/gone".to_owned(),
                    vec![Requirer::Root, unit("@m/app", "1.0.0")]
                ),
                ("@m/nowhere".to_owned(), vec![Requirer::Root]),
            ]
        );
        let message = errors[0].to_string();
        assert!(
            message.contains("no registered source provides @m/gone"),
            "{message}"
        );
        assert!(message.contains("^1 from @m/app 1.0.0"), "{message}");
    }

    #[test]
    fn a_satisfying_locked_version_that_is_not_available_needs_synchronization() {
        let published: &[Published<'_>] = &[("@a/x", "1.1.0", &[])];
        let errors = resolve_root(&[("@a/x", "^1")], published, &[("@a/x", "1.0.0")]).unwrap_err();
        assert_eq!(
            errors,
            [ResolutionError::Unavailable {
                identity: identity("@a/x"),
                version: version("1.0.0"),
                requirements: vec![super::Requirement {
                    requirer: Requirer::Root,
                    range: "^1".parse().unwrap(),
                }],
            }]
        );
        let message = errors[0].to_string();
        assert!(message.contains("locked to 1.0.0"), "{message}");
        assert!(message.contains("synchronizing the registry"), "{message}");

        // A locked transitive dependency is not replaced by another version.
        let errors = resolve_root(
            &[("@a/app", "^1")],
            &[
                ("@a/app", "1.0.0", &[("@a/lib", "^1")]),
                ("@a/lib", "1.1.0", &[]),
            ],
            &[("@a/app", "1.0.0"), ("@a/lib", "1.0.0")],
        )
        .unwrap_err();
        assert!(
            matches!(
                &errors[..],
                [ResolutionError::Unavailable { identity: name, .. }] if *name == identity("@a/lib")
            ),
            "{errors:?}"
        );

        // A lock the requirements no longer allow is replaced instead.
        let resolution =
            resolve_root(&[("@a/x", "^1.1")], published, &[("@a/x", "1.0.0")]).unwrap();
        assert_eq!(
            resolution.changes()[&identity("@a/x")],
            LockChange::Changed {
                from: version("1.0.0"),
                to: version("1.1.0"),
            }
        );
    }

    #[test]
    fn an_older_version_avoids_a_missing_dependency() {
        let resolution = resolve_root(
            &[("@n/app", "^1")],
            &[
                ("@n/app", "1.1.0", &[]),
                ("@n/app", "1.2.0", &[("@n/gone", "^1")]),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(versions(&resolution), pairs(&[("@n/app", "1.1.0")]));
    }

    #[test]
    fn failures_are_explained_by_the_preferred_candidates() {
        // @n/app 1.1.0 conflicts on @n/lib, and @n/app 1.0.0 needs a unit no
        // source provides. The errors describe the preferred @n/app 1.1.0.
        let errors = resolve_root(
            &[("@n/app", "^1"), ("@n/lib", "^1")],
            &[
                ("@n/app", "1.0.0", &[("@n/gone", "^1")]),
                ("@n/app", "1.1.0", &[("@n/lib", "^2")]),
                ("@n/lib", "1.0.0", &[]),
            ],
            &[],
        )
        .unwrap_err();
        let [
            ResolutionError::Incompatible {
                identity: name,
                requirements,
                ..
            },
        ] = &errors[..]
        else {
            panic!("expected one incompatibility, got {errors:?}");
        };
        assert_eq!(name, &identity("@n/lib"));
        assert_eq!(requirements[1].requirer(), &unit("@n/app", "1.1.0"));
    }

    #[test]
    fn an_older_version_avoids_a_cycle() {
        let resolution = resolve_root(
            &[("@c/app", "^1")],
            &[
                ("@c/app", "1.0.0", &[]),
                ("@c/app", "1.1.0", &[("@c/lib", "^1")]),
                ("@c/lib", "1.0.0", &[("@c/app", "^1")]),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(versions(&resolution), pairs(&[("@c/app", "1.0.0")]));

        let root_identity = identity("@c/root");
        let view = available(&[
            ("@c/lib", "1.0.0", &[]),
            ("@c/lib", "1.1.0", &[("@c/root", "*")]),
        ]);
        let resolution = resolve(
            Some(&root_identity),
            &dependencies(&[("@c/lib", "^1")]),
            &view,
            &Lock::new(),
        )
        .unwrap();
        assert_eq!(versions(&resolution), pairs(&[("@c/lib", "1.0.0")]));
    }

    #[test]
    fn cycles_report_their_path() {
        let errors = resolve_root(
            &[("@c/app", "^1")],
            &[
                ("@c/app", "1.0.0", &[("@c/one", "^1")]),
                ("@c/one", "1.0.0", &[("@c/two", "^1")]),
                ("@c/two", "1.0.0", &[("@c/one", "^1")]),
            ],
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [ResolutionError::Cycle {
                path: vec![
                    unit("@c/one", "1.0.0"),
                    unit("@c/two", "1.0.0"),
                    unit("@c/one", "1.0.0"),
                ],
            }]
        );
        let message = errors[0].to_string();
        assert!(
            message.contains("@c/one 1.0.0 -> @c/two 1.0.0 -> @c/one 1.0.0"),
            "{message}"
        );
    }

    #[test]
    fn a_published_version_that_depends_on_itself_is_a_cycle() {
        let errors = resolve_root(
            &[("@c/self", "^1")],
            &[("@c/self", "1.0.0", &[("@c/self", "^1")])],
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [ResolutionError::Cycle {
                path: vec![unit("@c/self", "1.0.0"), unit("@c/self", "1.0.0")],
            }]
        );
    }

    #[test]
    fn a_dependency_on_the_root_unit_is_a_cycle() {
        let root_identity = identity("@r/app");
        let view = available(&[
            ("@r/app", "0.9.0", &[]),
            ("@r/lib", "1.0.0", &[("@r/app", "^0.9")]),
        ]);
        let errors = resolve(
            Some(&root_identity),
            &dependencies(&[("@r/lib", "^1")]),
            &view,
            &Lock::new(),
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [ResolutionError::Cycle {
                path: vec![Requirer::Root, unit("@r/lib", "1.0.0"), Requirer::Root],
            }]
        );
        let message = errors[0].to_string();
        assert!(
            message.contains("the root -> @r/lib 1.0.0 -> the root"),
            "{message}"
        );
    }

    #[test]
    fn the_root_unit_is_never_resolved_from_its_published_versions() {
        let root_identity = identity("@r/app");
        let view = available(&[("@r/app", "0.9.0", &[]), ("@r/lib", "1.0.0", &[])]);
        let resolution = resolve(
            Some(&root_identity),
            &dependencies(&[("@r/lib", "^1")]),
            &view,
            &lock(&[("@r/app", "0.9.0")]),
        )
        .unwrap();
        assert_eq!(versions(&resolution), pairs(&[("@r/lib", "1.0.0")]));
        assert_eq!(
            resolution.changes()[&root_identity],
            LockChange::Retired(version("0.9.0"))
        );
    }

    #[test]
    fn pre_releases_are_never_selected_implicitly() {
        let published: &[Published<'_>] = &[
            ("@p/x", "1.0.0", &[]),
            ("@p/x", "1.1.0-beta.1", &[]),
            ("@p/x", "1.1.0-beta.2", &[]),
            ("@p/x", "2.0.0-rc.1", &[]),
        ];
        let resolved = |range: &str| {
            let resolution = resolve_root(&[("@p/x", range)], published, &[]).unwrap();
            resolution.graph().units()[&identity("@p/x")]
                .version()
                .to_string()
        };
        assert_eq!(resolved("^1"), "1.0.0");
        assert_eq!(resolved("*"), "1.0.0");
        assert_eq!(resolved("^1.1.0-beta.1"), "1.1.0-beta.2");
        assert_eq!(resolved("=1.1.0-beta.1"), "1.1.0-beta.1");
        assert_eq!(resolved(">=2.0.0-rc.1"), "2.0.0-rc.1");

        // Only a pre-release is available, and the range does not name one.
        let errors = resolve_root(&[("@p/x", "^2")], published, &[]).unwrap_err();
        assert!(
            matches!(&errors[..], [ResolutionError::Incompatible { .. }]),
            "{errors:?}"
        );
    }

    #[test]
    fn an_explicit_pre_release_range_still_prefers_a_higher_release() {
        let resolution = resolve_root(
            &[("@p/x", "^1.1.0-beta.1")],
            &[
                ("@p/x", "1.1.0-beta.1", &[]),
                ("@p/x", "1.1.0", &[]),
                ("@p/x", "1.2.0", &[]),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(versions(&resolution), pairs(&[("@p/x", "1.2.0")]));
    }

    #[test]
    fn a_locked_pre_release_the_requirements_no_longer_name_is_replaced() {
        let published: &[Published<'_>] = &[("@p/x", "1.0.0", &[]), ("@p/x", "1.1.0-beta.1", &[])];
        let kept = resolve_root(
            &[("@p/x", "^1.1.0-beta.1")],
            published,
            &[("@p/x", "1.1.0-beta.1")],
        )
        .unwrap();
        assert!(kept.changes().is_empty());

        let replaced =
            resolve_root(&[("@p/x", "^1")], published, &[("@p/x", "1.1.0-beta.1")]).unwrap();
        assert_eq!(
            replaced.changes()[&identity("@p/x")],
            LockChange::Changed {
                from: version("1.1.0-beta.1"),
                to: version("1.0.0"),
            }
        );
    }

    #[test]
    fn versions_differing_only_in_build_metadata_are_never_chosen_between() {
        let published: &[Published<'_>] = &[
            ("@p/x", "1.0.0+a", &[]),
            ("@p/x", "1.0.0+b", &[]),
            ("@p/x", "1.1.0", &[]),
        ];
        for locked in [&[][..], &[("@p/x", "1.0.0+a")]] {
            let errors = resolve_root(&[("@p/x", "^1")], published, locked).unwrap_err();
            assert_eq!(
                errors,
                [ResolutionError::Ambiguous {
                    identity: identity("@p/x"),
                    versions: vec![version("1.0.0+a"), version("1.0.0+b")],
                    requirements: vec![super::Requirement {
                        requirer: Requirer::Root,
                        range: "^1".parse().unwrap(),
                    }],
                }]
            );
            let message = errors[0].to_string();
            assert!(
                message.contains("1.0.0+a, 1.0.0+b differ only in build metadata"),
                "{message}"
            );
        }

        // A version that differs from every other in more than build
        // metadata is not ambiguous.
        let resolution = resolve_root(
            &[("@p/x", "^1")],
            &[("@p/x", "1.0.0+a", &[]), ("@p/x", "1.1.0+a", &[])],
            &[],
        )
        .unwrap();
        assert_eq!(versions(&resolution), pairs(&[("@p/x", "1.1.0+a")]));
    }

    #[test]
    fn an_older_version_avoids_an_ambiguous_dependency() {
        let resolution = resolve_root(
            &[("@p/app", "^1")],
            &[
                ("@p/app", "1.0.0", &[]),
                ("@p/app", "1.1.0", &[("@p/x", "^1")]),
                ("@p/x", "1.0.0+a", &[]),
                ("@p/x", "1.0.0+b", &[]),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(versions(&resolution), pairs(&[("@p/app", "1.0.0")]));
    }

    #[test]
    fn the_preferred_resolution_is_explained_when_identities_could_depend_on_each_other() {
        // Some version of each identity depends on the other, so neither
        // can be decided after every requirement on it is known. @q/x 2.0.0
        // conflicts with @q/y, and @q/x 1.0.0 needs an unavailable @q/y.
        let errors = resolve_root(
            &[("@q/x", "*"), ("@q/y", "*")],
            &[
                ("@q/x", "1.0.0", &[("@q/y", "^2")]),
                ("@q/x", "2.0.0", &[]),
                ("@q/y", "1.0.0", &[("@q/x", "^1")]),
            ],
            &[],
        )
        .unwrap_err();
        let [
            ResolutionError::Incompatible {
                identity: name,
                requirements,
                ..
            },
        ] = &errors[..]
        else {
            panic!("expected one incompatibility, got {errors:?}");
        };
        assert_eq!(name, &identity("@q/y"));
        let requirers: Vec<&Requirer> = requirements
            .iter()
            .map(|requirement| requirement.requirer())
            .collect();
        assert_eq!(requirers, [&Requirer::Root, &unit("@q/x", "1.0.0")]);
    }

    /// Published versions with every kind of problem at once.
    const EVERY_PROBLEM: &[Published<'static>] = &[
        (
            "@e/app-a",
            "1.0.0",
            &[("@e/shared", "^1"), ("@e/loop-a", "^1")],
        ),
        ("@e/app-b", "1.0.0", &[("@e/shared", "^2")]),
        ("@e/shared", "1.0.0", &[]),
        ("@e/shared", "2.0.0", &[]),
        ("@e/loop-a", "1.0.0", &[("@e/loop-b", "^1")]),
        ("@e/loop-b", "1.0.0", &[("@e/loop-a", "^1")]),
        ("@e/locked", "1.1.0", &[]),
        ("@e/twin", "1.0.0+a", &[]),
        ("@e/twin", "1.0.0+b", &[]),
    ];

    const EVERY_PROBLEM_ROOT: &[(&str, &str)] = &[
        ("@e/app-a", "^1"),
        ("@e/app-b", "^1"),
        ("@e/ghost", "^1"),
        ("@e/locked", "^1"),
        ("@e/twin", "^1"),
    ];

    const EVERY_PROBLEM_LOCK: &[(&str, &str)] = &[("@e/locked", "1.0.0")];

    #[test]
    fn every_problem_is_reported_in_a_stable_order() {
        let errors =
            resolve_root(EVERY_PROBLEM_ROOT, EVERY_PROBLEM, EVERY_PROBLEM_LOCK).unwrap_err();
        let kinds: Vec<String> = errors
            .iter()
            .map(|error| match error {
                ResolutionError::Missing { identity, .. } => format!("missing {identity}"),
                ResolutionError::Ambiguous { identity, .. } => format!("ambiguous {identity}"),
                ResolutionError::Unresolved { identity, .. } => format!("unresolved {identity}"),
                ResolutionError::Unavailable { identity, .. } => {
                    format!("unavailable {identity}")
                }
                ResolutionError::Incompatible { identity, .. } => {
                    format!("incompatible {identity}")
                }
                ResolutionError::Cycle { path } => format!("cycle of {}", path.len()),
                ResolutionError::StepLimitExceeded { .. } => "step limit".to_owned(),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "missing @e/ghost",
                "ambiguous @e/twin",
                "unavailable @e/locked",
                "incompatible @e/shared",
                "cycle of 3",
            ]
        );
    }

    /// Root requirements, published versions, and a lock.
    type OrderingCase = (
        &'static [(&'static str, &'static str)],
        &'static [Published<'static>],
        &'static [(&'static str, &'static str)],
    );

    /// Every ordering of `items`, by repeated insertion.
    fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
        let Some((first, rest)) = items.split_first() else {
            return vec![Vec::new()];
        };
        let mut all = Vec::new();
        for permutation in permutations(rest) {
            for position in 0..=permutation.len() {
                let mut ordering = permutation.clone();
                ordering.insert(position, first.clone());
                all.push(ordering);
            }
        }
        all
    }

    /// Builds the available versions by inserting `published` in the order
    /// given, with each version's dependencies inserted in reverse.
    fn insert_in_order(published: &[Published<'_>]) -> AvailableVersions {
        let mut available = AvailableVersions::new();
        for (name, published_version, published_dependencies) in published {
            let mut reversed = published_dependencies.to_vec();
            reversed.reverse();
            available
                .entry(identity(name))
                .or_default()
                .insert(version(published_version), dependencies(&reversed));
        }
        available
    }

    #[test]
    fn declaration_and_enumeration_order_never_change_the_outcome() {
        let cases: [OrderingCase; 2] = [
            (
                SEVEN_UNIT_ROOT,
                SEVEN_UNITS,
                &[("@metamao/docker", "1.0.0")],
            ),
            (EVERY_PROBLEM_ROOT, EVERY_PROBLEM, EVERY_PROBLEM_LOCK),
        ];
        for (root, published, locked) in cases {
            let expected = resolve_root(root, published, locked);
            let mut enumerations = vec![published.to_vec()];
            for rotation in 1..published.len() {
                let mut rotated = published.to_vec();
                rotated.rotate_left(rotation);
                enumerations.push(rotated);
            }
            let mut reversed = published.to_vec();
            reversed.reverse();
            enumerations.push(reversed);

            for declared in permutations(root) {
                for enumeration in &enumerations {
                    let mut lock_order = locked.to_vec();
                    lock_order.reverse();
                    let outcome = resolve(
                        None,
                        &dependencies(&declared),
                        &insert_in_order(enumeration),
                        &lock(&lock_order),
                    );
                    assert_eq!(outcome, expected, "declared {declared:?}");
                }
            }
        }
    }

    /// A deterministic xorshift generator, so the oracle cases are
    /// reproducible without a random-number dependency.
    struct Generator(u64);

    impl Generator {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }

        fn chance(&mut self, percent: u64) -> bool {
            self.next() % 100 < percent
        }
    }

    const ORACLE_UNITS: [&str; 4] = ["@o/a", "@o/b", "@o/c", "@o/d"];
    const ORACLE_VERSIONS: [&str; 3] = ["1.0.0", "1.1.0", "2.0.0"];
    const ORACLE_RANGES: [&str; 7] = ["^1", "^2", "=1.1.0", ">=1.1.0", "<1.1.0", "*", "~1.0"];
    const ORACLE_MISSING: &str = "@o/missing";
    const ORACLE_ROOT: &str = "@o/root";

    /// A bounded resolution problem for the oracle.
    struct Case {
        root_identity: Option<UnitIdentity>,
        root: Dependencies,
        available: AvailableVersions,
    }

    fn generate(generator: &mut Generator) -> Case {
        let root_identity = generator.chance(30).then(|| identity(ORACLE_ROOT));
        let mut targets: Vec<&str> = ORACLE_UNITS.to_vec();
        targets.push(ORACLE_MISSING);
        if root_identity.is_some() {
            targets.push(ORACLE_ROOT);
        }
        let range = |generator: &mut Generator| {
            ORACLE_RANGES[generator.below(ORACLE_RANGES.len())]
                .parse()
                .unwrap()
        };

        let mut available = AvailableVersions::new();
        for name in ORACLE_UNITS {
            // Some identities have no registered source, and some have a
            // source with no versions.
            if generator.chance(8) {
                continue;
            }
            let versions = available.entry(identity(name)).or_default();
            for published_version in ORACLE_VERSIONS {
                if !generator.chance(65) {
                    continue;
                }
                let mut published_dependencies = Dependencies::new();
                for target in &targets {
                    if generator.chance(14) {
                        published_dependencies.insert(identity(target), range(generator));
                    }
                }
                versions.insert(version(published_version), published_dependencies);
            }
        }
        if root_identity.is_some() && generator.chance(50) {
            available
                .entry(identity(ORACLE_ROOT))
                .or_default()
                .insert(version("1.0.0"), Dependencies::new());
        }

        let mut root = Dependencies::new();
        for _ in 0..=generator.below(3) {
            let target = if generator.chance(8) {
                ORACLE_MISSING
            } else {
                ORACLE_UNITS[generator.below(ORACLE_UNITS.len())]
            };
            root.insert(identity(target), range(generator));
        }
        Case {
            root_identity,
            root,
            available,
        }
    }

    /// Whether `chosen` is a valid resolution of `case`, checked directly
    /// from the definition: every requirement is chosen and satisfied,
    /// nothing depends on the root, everything chosen is reachable, and the
    /// graph has no cycle.
    fn is_valid(case: &Case, chosen: &BTreeMap<UnitIdentity, Version>) -> bool {
        let satisfied = |requirements: &Dependencies| {
            requirements.iter().all(|(dependency, range)| {
                Some(dependency) != case.root_identity.as_ref()
                    && chosen
                        .get(dependency)
                        .is_some_and(|chosen_version| range.matches(chosen_version))
            })
        };
        let declared = |name: &UnitIdentity, chosen_version: &Version| {
            case.available
                .get(name)
                .and_then(|versions| versions.get(chosen_version))
        };
        if !satisfied(&case.root) {
            return false;
        }
        for (name, chosen_version) in chosen {
            match declared(name, chosen_version) {
                Some(requirements) if Some(name) != case.root_identity.as_ref() => {
                    if !satisfied(requirements) {
                        return false;
                    }
                }
                _ => return false,
            }
        }

        let mut reached = BTreeSet::new();
        let mut pending: Vec<&UnitIdentity> = case.root.keys().collect();
        while let Some(name) = pending.pop() {
            if reached.insert(name) {
                if let Some(requirements) = chosen.get(name).and_then(|v| declared(name, v)) {
                    pending.extend(requirements.keys());
                }
            }
        }
        if chosen.keys().any(|name| !reached.contains(name)) {
            return false;
        }

        // Kahn's algorithm consumes every node exactly when there is no
        // cycle.
        let mut in_degree: BTreeMap<&UnitIdentity, usize> =
            chosen.keys().map(|name| (name, 0)).collect();
        for (name, chosen_version) in chosen {
            for dependency in declared(name, chosen_version)
                .into_iter()
                .flat_map(|d| d.keys())
            {
                if let Some(degree) = in_degree.get_mut(dependency) {
                    *degree += 1;
                }
            }
        }
        let mut ready: Vec<&UnitIdentity> = in_degree
            .iter()
            .filter(|(_, degree)| **degree == 0)
            .map(|(name, _)| *name)
            .collect();
        let mut consumed = 0;
        while let Some(name) = ready.pop() {
            consumed += 1;
            let requirements = chosen.get(name).and_then(|v| declared(name, v));
            for dependency in requirements.into_iter().flat_map(|d| d.keys()) {
                if let Some(degree) = in_degree.get_mut(dependency) {
                    *degree -= 1;
                    if *degree == 0 {
                        ready.push(dependency);
                    }
                }
            }
        }
        consumed == chosen.len()
    }

    /// Whether any valid resolution exists, by trying every assignment of
    /// "absent" or one available version to every identity.
    fn exhaustively_resolvable(case: &Case) -> bool {
        let options: Vec<(&UnitIdentity, Vec<Option<&Version>>)> = case
            .available
            .iter()
            .filter(|(name, _)| Some(*name) != case.root_identity.as_ref())
            .map(|(name, versions)| {
                let mut choices = vec![None];
                choices.extend(versions.keys().map(Some));
                (name, choices)
            })
            .collect();
        let combinations: usize = options.iter().map(|(_, choices)| choices.len()).product();
        (0..combinations).any(|mut index| {
            let mut chosen = BTreeMap::new();
            for (name, choices) in &options {
                if let Some(chosen_version) = choices[index % choices.len()] {
                    chosen.insert((*name).clone(), chosen_version.clone());
                }
                index /= choices.len();
            }
            is_valid(case, &chosen)
        })
    }

    /// Checks that an error states something true about `case`.
    fn assert_truthful(case: &Case, error: &ResolutionError) {
        let jointly_satisfiable = |name: &UnitIdentity, requirements: &[super::Requirement]| {
            case.available.get(name).is_some_and(|versions| {
                versions.keys().any(|candidate| {
                    requirements
                        .iter()
                        .all(|requirement| requirement.range().matches(candidate))
                })
            })
        };
        match error {
            ResolutionError::Missing { identity: name, .. } => {
                assert!(!case.available.contains_key(name), "{error}");
            }
            ResolutionError::Incompatible {
                identity: name,
                requirements,
                ..
            } => assert!(!jointly_satisfiable(name, requirements), "{error}"),
            ResolutionError::Unresolved {
                identity: name,
                requirements,
            } => assert!(jointly_satisfiable(name, requirements), "{error}"),
            ResolutionError::Cycle { path } => {
                assert!(path.len() >= 2 && path.first() == path.last(), "{error}");
            }
            ResolutionError::Ambiguous { .. }
            | ResolutionError::Unavailable { .. }
            | ResolutionError::StepLimitExceeded { .. } => {
                panic!("generated cases cannot produce {error}")
            }
        }
    }

    #[test]
    fn the_resolver_agrees_with_exhaustive_search_on_bounded_graphs() {
        let mut generator = Generator(0x9e37_79b9_7f4a_7c15);
        let (mut resolved, mut failed) = (0, 0);
        // Solvable cases whose preferred candidates hit each kind of problem,
        // so the search had to find an alternative.
        let (mut avoided_missing, mut avoided_cycle, mut avoided_conflict) = (0, 0, 0);
        for case_number in 0..3000 {
            let case = generate(&mut generator);
            let outcome = resolve(
                case.root_identity.as_ref(),
                &case.root,
                &case.available,
                &Lock::new(),
            );
            let expected = exhaustively_resolvable(&case);
            assert_eq!(
                outcome.is_ok(),
                expected,
                "case {case_number}: root {:?}, root identity {:?}, available {:?}, outcome {outcome:?}",
                case.root,
                case.root_identity,
                case.available
            );
            match &outcome {
                Ok(resolution) => {
                    resolved += 1;
                    let preferred = Resolver::new(
                        case.root_identity.as_ref(),
                        &case.root,
                        &case.available,
                        &Lock::new(),
                    )
                    .explain();
                    for problem in &preferred {
                        match problem {
                            ResolutionError::Missing { .. } => avoided_missing += 1,
                            ResolutionError::Cycle { .. } => avoided_cycle += 1,
                            _ => avoided_conflict += 1,
                        }
                    }
                    let chosen: BTreeMap<UnitIdentity, Version> = resolution
                        .graph()
                        .units()
                        .iter()
                        .map(|(name, unit)| (name.clone(), unit.version().clone()))
                        .collect();
                    assert!(is_valid(&case, &chosen), "case {case_number}");
                    for (name, unit) in resolution.graph().units() {
                        assert_eq!(
                            Some(unit.dependencies()),
                            case.available[name].get(unit.version()),
                            "case {case_number}"
                        );
                    }
                    // The result, used as the lock, is kept exactly.
                    let relocked = resolve(
                        case.root_identity.as_ref(),
                        &case.root,
                        &case.available,
                        &chosen,
                    )
                    .unwrap();
                    assert_eq!(relocked.graph(), resolution.graph(), "case {case_number}");
                    assert!(relocked.changes().is_empty(), "case {case_number}");
                }
                Err(errors) => {
                    failed += 1;
                    assert!(!errors.is_empty(), "case {case_number}");
                    for error in errors {
                        assert_truthful(&case, error);
                    }
                }
            }
        }
        // Both outcomes, and every kind of avoided problem, are well
        // represented.
        let counts = format!(
            "{resolved} resolved, {failed} failed; avoided {avoided_missing} missing, \
             {avoided_cycle} cycles, {avoided_conflict} conflicts"
        );
        assert!(resolved > 500 && failed > 500, "{counts}");
        assert!(
            avoided_missing > 20 && avoided_cycle > 20 && avoided_conflict > 20,
            "{counts}"
        );
    }
}
