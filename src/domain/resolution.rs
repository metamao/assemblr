//! Dependency resolution: exactly one published version for every unit
//! identity that a set of root requirements reaches.
//!
//! [`resolve`] works on plain data: the root requirements, the published
//! versions available locally with each version's dependencies, and any
//! existing lock state. It either returns the resolved graph and how it
//! differs from the lock, or every problem it found.
//!
//! # Policy
//!
//! - A locked version that satisfies every requirement on its identity is
//!   tried first. It changes only when it no longer satisfies them or when
//!   its own dependencies cannot be resolved with it. A newer published
//!   version never changes it.
//! - Otherwise candidates are tried from the highest matching version down,
//!   the provisional Alpha default (OC-16).
//! - Requirements match with Cargo's semantics (OC-15, provisional). A
//!   pre-release matches only a requirement that names a pre-release of the
//!   same `major.minor.patch`, so it is never selected implicitly. Build
//!   metadata is part of a version's identity: `1.0.0+a` and `1.0.0+b` are
//!   distinct versions, ordered by [`Version`]'s ordering.
//! - Identities are decided in dependency order: a unit is decided before
//!   the units any of its published versions depend on, so each decision
//!   sees every requirement on it. Ties, and identities that could depend on
//!   each other, are decided in identity order. When raising one unit's
//!   version forces another's down, the unit decided first keeps the higher
//!   version.
//!
//! # Errors
//!
//! Only incompatible requirements make the search try other versions.
//! A missing identity or an unavailable locked version needs the user to act
//! on the registry, so it fails resolution instead of being avoided by
//! quietly choosing an older version of the unit that requires it. Cycles are
//! checked in the chosen graph.
//!
//! When resolution fails, the errors describe the preferred resolution: each
//! identity takes its first candidate, and every problem along the way is
//! reported, not only the first.
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
/// On failure every error is returned, in a deterministic order.
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

/// Why a set of requirements cannot be resolved.
///
/// Requirements inside an error are ordered by requirer, the root first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionError {
    /// No registered source provides a required identity.
    Missing {
        /// The identity no source provides.
        identity: UnitIdentity,
        /// The requirements on it.
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
    /// The chosen versions depend on each other in a cycle.
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
            Self::Unavailable { .. } => 1,
            Self::Incompatible { .. } => 2,
            Self::Cycle { .. } => 3,
            Self::StepLimitExceeded { .. } => 4,
        }
    }

    fn identity(&self) -> Option<&UnitIdentity> {
        match self {
            Self::Missing { identity, .. }
            | Self::Unavailable { identity, .. }
            | Self::Incompatible { identity, .. } => Some(identity),
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

fn write_list<T: fmt::Display>(f: &mut fmt::Formatter<'_>, items: &[T]) -> fmt::Result {
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            f.write_str(", ")?;
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
                write!(f, "no registered source provides {identity}, required as ")?;
                write_list(f, requirements)
            }
            Self::Unavailable {
                identity,
                version,
                requirements,
            } => {
                write!(
                    f,
                    "{identity} is locked to {version}, which is not available locally; \
                     synchronizing the registry may be needed (required as "
                )?;
                write_list(f, requirements)?;
                f.write_str(")")
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
                write_list(f, requirements)?;
                if available.is_empty() {
                    f.write_str("; no versions are available locally")
                } else {
                    f.write_str("; available versions: ")?;
                    write_list(f, available)
                }
            }
            Self::Cycle { path } => {
                f.write_str("dependency cycle: ")?;
                for (index, requirer) in path.iter().enumerate() {
                    if index > 0 {
                        f.write_str(" -> ")?;
                    }
                    write!(f, "{requirer}")?;
                }
                Ok(())
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

/// What the search must do next, given an assignment.
enum Step<'a> {
    /// Every required identity is decided and satisfied.
    Complete,
    /// Decide this identity, trying these candidates in order.
    Decide(&'a UnitIdentity, Vec<&'a Version>),
    /// The version chosen for this identity fails a requirement on it.
    Violated(&'a UnitIdentity),
    /// No available version satisfies every requirement on this identity.
    NoCandidate(&'a UnitIdentity),
    /// No source provides this identity.
    Missing(&'a UnitIdentity),
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
            Ok(assignment) => {
                let cycles = self.cycles(&assignment);
                if cycles.is_empty() {
                    return Ok(self.resolution(&assignment));
                }
                cycles
            }
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

    /// Searches for an assignment that satisfies every requirement.
    fn search(&self, step_limit: usize) -> Result<Assignment<'a>, Failure> {
        let mut frames: Vec<Frame<'a>> = Vec::new();
        let mut assignment = Assignment::new();
        let nothing_skipped = BTreeSet::new();
        for _ in 0..step_limit {
            let demands = self.demands(&assignment);
            let conflict = match self.next_step(&assignment, &demands, &nothing_skipped) {
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
                Step::NoCandidate(identity) => requirers(&demands, identity, |_| true),
                Step::Missing(identity) => {
                    return Err(Failure::Aborted(self.missing(identity, &demands)));
                }
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

    /// Follows the preferred resolution without backtracking, reporting
    /// every problem on the way.
    fn explain(&self) -> Vec<ResolutionError> {
        let mut assignment = Assignment::new();
        let mut skipped = BTreeSet::new();
        let mut redecided = BTreeSet::new();
        let mut errors = Vec::new();
        let mut unsettled = None;
        loop {
            let demands = self.demands(&assignment);
            let identity = match self.next_step(&assignment, &demands, &skipped) {
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
                Step::Unavailable(identity, version) => {
                    errors.push(self.unavailable(identity, version, &demands));
                    identity
                }
            };
            skipped.insert(identity);
        }
        errors.extend(self.cycles(&assignment));
        // The search proved that no resolution exists, so the preferred one
        // has a problem. If it still settled on versions for every identity,
        // the identity it could not settle is the best explanation left.
        if errors.is_empty() {
            if let Some(identity) = unsettled {
                errors.push(self.incompatible(identity, &self.demands(&assignment)));
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

    fn next_step(
        &self,
        assignment: &Assignment<'a>,
        demands: &Demands<'a>,
        skipped: &BTreeSet<&'a UnitIdentity>,
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

    /// Every cycle in the graph of the root and the assigned versions, found
    /// by a depth-first walk from the root in identity order. An edge counts
    /// only when the assigned version satisfies its requirement, so a
    /// failed resolution reports only cycles that its versions really form.
    fn cycles(&self, assignment: &Assignment<'a>) -> Vec<ResolutionError> {
        // `None` is the root.
        let successors = |node: Option<&'a UnitIdentity>| -> Vec<Option<&'a UnitIdentity>> {
            let dependencies: Vec<(&'a UnitIdentity, &'a VersionRequirement)> = match node {
                None => self.root.iter().collect(),
                Some(identity) => match assignment.get(identity) {
                    Some(&version) => self.dependencies(identity, version).collect(),
                    None => Vec::new(),
                },
            };
            let mut nodes: Vec<Option<&'a UnitIdentity>> = dependencies
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
        let requirer = |node: Option<&UnitIdentity>| match node {
            None => Requirer::Root,
            Some(identity) => match assignment.get(identity) {
                Some(&version) => Requirer::Unit(identity.clone(), version.clone()),
                None => Requirer::Root,
            },
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
                let mut path: Vec<Requirer> = stack[start..]
                    .iter()
                    .map(|(on_stack, _, _)| requirer(*on_stack))
                    .collect();
                path.push(requirer(next));
                cycles.push(ResolutionError::Cycle { path });
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
    use std::collections::BTreeMap;

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
    fn a_missing_dependency_is_not_avoided_by_an_older_requirer() {
        // @n/app 1.1.0 conflicts on @n/lib, and @n/app 1.0.0 needs a unit no
        // source provides. Both problems are reported.
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
        assert!(
            matches!(
                &errors[..],
                [
                    ResolutionError::Missing { identity: missing, .. },
                    ResolutionError::Incompatible { identity: incompatible, .. },
                ] if *missing == identity("@n/gone") && *incompatible == identity("@n/lib")
            ),
            "{errors:?}"
        );
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
    fn versions_differing_only_in_build_metadata_are_distinct() {
        let published: &[Published<'_>] = &[("@p/x", "1.0.0+a", &[]), ("@p/x", "1.0.0+b", &[])];
        let fresh = resolve_root(&[("@p/x", "^1")], published, &[]).unwrap();
        assert_eq!(versions(&fresh), pairs(&[("@p/x", "1.0.0+b")]));

        let locked = resolve_root(&[("@p/x", "^1")], published, &[("@p/x", "1.0.0+a")]).unwrap();
        assert_eq!(versions(&locked), pairs(&[("@p/x", "1.0.0+a")]));
        assert!(locked.changes().is_empty());
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
    ];

    const EVERY_PROBLEM_ROOT: &[(&str, &str)] = &[
        ("@e/app-a", "^1"),
        ("@e/app-b", "^1"),
        ("@e/ghost", "^1"),
        ("@e/locked", "^1"),
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
                "unavailable @e/locked",
                "incompatible @e/shared",
                "cycle of 3",
            ]
        );
    }

    /// Rebuilds `published` in a different order: reversed or rotated, with
    /// each version's dependencies reversed.
    fn reorder<'s>(published: &[Published<'s>], rotation: usize) -> AvailableVersions {
        let mut entries: Vec<(&str, &str, Dependencies)> = published
            .iter()
            .map(|(name, published_version, published_dependencies)| {
                let mut reversed = published_dependencies.to_vec();
                reversed.reverse();
                (*name, *published_version, dependencies(&reversed))
            })
            .collect();
        if rotation == 0 {
            entries.reverse();
        } else {
            let len = entries.len();
            entries.rotate_left(rotation % len);
        }
        let mut available = AvailableVersions::new();
        for (name, published_version, published_dependencies) in entries {
            available
                .entry(identity(name))
                .or_default()
                .insert(version(published_version), published_dependencies);
        }
        available
    }

    fn reordered_root(root: &[(&str, &str)], rotation: usize) -> Dependencies {
        let mut entries = root.to_vec();
        let len = entries.len();
        entries.rotate_left(rotation % len);
        entries.reverse();
        dependencies(&entries)
    }

    #[test]
    fn reordered_inputs_give_identical_results_and_errors() {
        let expected_resolution = resolve_root(SEVEN_UNIT_ROOT, SEVEN_UNITS, &[]);
        let expected_errors = resolve_root(EVERY_PROBLEM_ROOT, EVERY_PROBLEM, EVERY_PROBLEM_LOCK);
        assert!(expected_resolution.is_ok() && expected_errors.is_err());
        for rotation in 0..8 {
            let resolution = resolve(
                None,
                &reordered_root(SEVEN_UNIT_ROOT, rotation),
                &reorder(SEVEN_UNITS, rotation),
                &Lock::new(),
            );
            assert_eq!(resolution, expected_resolution, "rotation {rotation}");

            let mut locked = EVERY_PROBLEM_LOCK.to_vec();
            locked.reverse();
            let errors = resolve(
                None,
                &reordered_root(EVERY_PROBLEM_ROOT, rotation),
                &reorder(EVERY_PROBLEM, rotation),
                &lock(&locked),
            );
            assert_eq!(errors, expected_errors, "rotation {rotation}");
        }
    }
}
