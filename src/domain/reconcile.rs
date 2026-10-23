//! Reconciling a consumer project with a new composition (see
//! `docs/domain/consumer-projects.md`, "Reconciliation").
//!
//! Three inputs meet at every path: the **base**, the composition last
//! materialized; the **current** content, what the project holds now, the
//! consumer's edits included; and the **incoming** composition. The
//! decision is per path and three-way: a side that did not change a file
//! yields to the side that did, equal changes are one change, and two
//! different changes are merged when the format allows it and are a
//! conflict otherwise. Nothing is resolved by preferring a side.
//!
//! The decisions here are data. Reading the project, merging texts, and
//! writing belong to the adapters.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::merge::{Combined, Conflict, Lists, Plan, Value, combine, derive, plan};
use super::path::RelativePath;
use super::snapshot::{Snapshot, SnapshotFile};

/// A side of the reconciliation that changes files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Contributor {
    /// The project's own content: the consumer's edits.
    Current,
    /// The incoming composition.
    Incoming,
}

impl fmt::Display for Contributor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Current => "the project",
            Self::Incoming => "the composition",
        })
    }
}

/// What the project holds at a path, as compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Current {
    /// A regular file, by its mode and content identity.
    File(SnapshotFile),
    /// A directory, a symbolic link, or another special file.
    Other,
}

/// Whether two different changes to one file can be merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mergeability {
    /// The file's format has a merge, and its base is available.
    Mergeable,
    /// The file's format has no merge.
    Format,
    /// The file's base is not stored, so its changes cannot be derived.
    BaseNotStored,
}

/// What the reconciliation does at one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Creates the incoming file; the base had nothing there.
    Create,
    /// Replaces the project's file with the incoming one, with this mode:
    /// the project did not change it.
    Write { executable: bool },
    /// Keeps the project's content and sets this mode: only the mode
    /// changed.
    Mode { executable: bool },
    /// Merges the incoming change into the project's file, which has this
    /// mode: both sides changed it, at different places.
    Merge { executable: bool },
    /// Removes the project's file: it left the composition, and the
    /// project did not change it.
    Remove,
    /// Leaves the path as it is: the project's content, or nothing.
    Keep,
}

/// Why one path cannot be reconciled without the consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disagreement {
    /// Both sides changed the content, and the changes cannot be merged.
    BothChanged(Mergeability),
    /// The composition removes a file the project edited.
    RemovedButEdited,
    /// The project deleted a file the composition changes.
    DeletedButChanged,
    /// The path holds something that is not a regular file.
    NotAFile,
}

/// The reconciliation of every path: what to do where the sides agree,
/// and where they do not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reconciliation {
    actions: BTreeMap<RelativePath, Action>,
    disagreements: BTreeMap<RelativePath, Disagreement>,
}

impl Reconciliation {
    /// The action at every path of the base or the incoming composition,
    /// in path order.
    pub fn actions(&self) -> &BTreeMap<RelativePath, Action> {
        &self.actions
    }

    /// The paths the consumer must settle, in path order.
    pub fn disagreements(&self) -> &BTreeMap<RelativePath, Disagreement> {
        &self.disagreements
    }
}

/// Decides every path of `base` and `incoming`. `current` holds what the
/// project has at those paths; an absent path is absent from the project.
/// `mergeable` says whether two changes to a path could be merged. A path
/// that the base lacks is created whatever the project holds there: that
/// content, which Assemblr did not materialize, is the caller's to refuse.
pub fn reconcile(
    base: &Snapshot,
    current: &BTreeMap<RelativePath, Current>,
    incoming: &Snapshot,
    mergeable: &dyn Fn(&RelativePath) -> Mergeability,
) -> Reconciliation {
    let paths: BTreeSet<&RelativePath> =
        base.files().keys().chain(incoming.files().keys()).collect();
    let mut out = Reconciliation::default();
    for path in paths {
        let decided = decide(
            base.files().get(path),
            current.get(path),
            incoming.files().get(path),
            || mergeable(path),
        );
        match decided {
            Ok(action) => {
                out.actions.insert(path.clone(), action);
            }
            Err(disagreement) => {
                out.disagreements.insert(path.clone(), disagreement);
            }
        }
    }
    out
}

/// One side's change against the base, or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// Neither side changed it, or both made the same change.
    Agreed,
    /// Only the project changed it.
    ByCurrent,
    /// Only the composition changed it.
    ByIncoming,
    /// Each side changed it differently.
    Both,
}

fn three_way<T: PartialEq>(base: &T, current: &T, incoming: &T) -> Change {
    match (current == base, incoming == base) {
        (true, true) => Change::Agreed,
        (true, false) => Change::ByIncoming,
        (false, true) => Change::ByCurrent,
        (false, false) if current == incoming => Change::Agreed,
        (false, false) => Change::Both,
    }
}

fn decide(
    base: Option<&SnapshotFile>,
    current: Option<&Current>,
    incoming: Option<&SnapshotFile>,
    mergeable: impl FnOnce() -> Mergeability,
) -> Result<Action, Disagreement> {
    let Some(base) = base else {
        // Nothing was materialized here: the incoming file is new.
        return Ok(Action::Create);
    };
    let current = match current {
        Some(Current::File(file)) => Some(file),
        Some(Current::Other) => return Err(Disagreement::NotAFile),
        None => None,
    };
    let Some(incoming) = incoming else {
        // The file left the composition.
        return match current {
            None => Ok(Action::Keep),
            Some(current) if current == base => Ok(Action::Remove),
            Some(_) => Err(Disagreement::RemovedButEdited),
        };
    };
    if incoming == base {
        // The composition did not change it: the project's content stands,
        // present or deleted.
        return Ok(Action::Keep);
    }
    let Some(current) = current else {
        return Err(Disagreement::DeletedButChanged);
    };
    // A bit has one other value, so two sides that both change it agree:
    // the composition's change wins when it made one, else the project's
    // mode stands.
    let mode = if incoming.is_executable() == base.is_executable() {
        current.is_executable()
    } else {
        incoming.is_executable()
    };
    match three_way(base.blob(), current.blob(), incoming.blob()) {
        Change::Agreed | Change::ByCurrent => Ok(if mode == current.is_executable() {
            Action::Keep
        } else {
            Action::Mode { executable: mode }
        }),
        Change::ByIncoming => Ok(Action::Write { executable: mode }),
        Change::Both => match mergeable() {
            Mergeability::Mergeable => Ok(Action::Merge { executable: mode }),
            other => Err(Disagreement::BothChanged(other)),
        },
    }
}

/// Merges `incoming`'s changes against `base` into `current`: the
/// operations each side derives from the base are combined, which detects
/// the changes that conflict, and the operations only the composition
/// makes are planned against the project's value, whose own changes stay
/// where they are. The plan's edits apply to the project's text.
pub fn merge_values(
    base: &Value,
    current: &Value,
    incoming: &Value,
) -> Result<Plan, Vec<Conflict<Contributor>>> {
    let sets = BTreeMap::from([
        (Contributor::Current, derive(base, current).operations),
        (Contributor::Incoming, derive(base, incoming).operations),
    ]);
    // Lists stay whole: a joined list would hold the project's items as
    // well as the composition's, and only the composition's are planned.
    let combined = combine(&sets, Lists::Whole)?;
    let only_incoming: Vec<Combined<Contributor>> = combined
        .into_iter()
        .filter(|entry| entry.amenders() == [Contributor::Incoming])
        .collect();
    plan(current, only_incoming)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]

    use super::*;
    use crate::domain::{ConflictKind, ValueChange};

    fn file(text: &str, executable: bool) -> SnapshotFile {
        SnapshotFile::new(text.as_bytes(), executable)
    }

    fn snapshot(files: &[(&str, &str, bool)]) -> Snapshot {
        Snapshot::new(
            files
                .iter()
                .map(|(path, text, executable)| {
                    (RelativePath::parse(path).unwrap(), file(text, *executable))
                })
                .collect(),
        )
    }

    fn current(files: &[(&str, &str, bool)]) -> BTreeMap<RelativePath, Current> {
        files
            .iter()
            .map(|(path, text, executable)| {
                (
                    RelativePath::parse(path).unwrap(),
                    Current::File(file(text, *executable)),
                )
            })
            .collect()
    }

    fn mergeable(_: &RelativePath) -> Mergeability {
        Mergeability::Mergeable
    }

    fn unmergeable(_: &RelativePath) -> Mergeability {
        Mergeability::Format
    }

    fn action(
        base: Option<(&str, bool)>,
        current: Option<(&str, bool)>,
        incoming: Option<(&str, bool)>,
    ) -> Result<Action, Disagreement> {
        fn entry(value: Option<(&str, bool)>) -> Vec<(&str, &str, bool)> {
            value
                .map(|(text, executable)| vec![("f", text, executable)])
                .unwrap_or_default()
        }
        let reconciliation = reconcile(
            &snapshot(&entry(base)),
            &self::current(&entry(current)),
            &snapshot(&entry(incoming)),
            &mergeable,
        );
        let path = RelativePath::parse("f").unwrap();
        match (
            reconciliation.actions.get(&path),
            reconciliation.disagreements.get(&path),
        ) {
            (Some(action), None) => Ok(action.clone()),
            (None, Some(disagreement)) => Err(disagreement.clone()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_side_that_did_not_change_a_file_yields_to_the_side_that_did() {
        let b = Some(("base", false));
        assert_eq!(action(None, None, Some(("new", false))), Ok(Action::Create));
        assert_eq!(
            action(b, b, Some(("incoming", false))),
            Ok(Action::Write { executable: false })
        );
        assert_eq!(action(b, Some(("edited", false)), b), Ok(Action::Keep));
        assert_eq!(action(b, b, b), Ok(Action::Keep));
        assert_eq!(
            action(b, Some(("same", false)), Some(("same", false))),
            Ok(Action::Keep)
        );
        assert_eq!(action(b, b, None), Ok(Action::Remove));
        assert_eq!(action(b, None, None), Ok(Action::Keep));
        assert_eq!(action(b, None, b), Ok(Action::Keep));
    }

    #[test]
    fn two_different_changes_are_merged_or_disagree() {
        let b = Some(("base", false));
        assert_eq!(
            action(b, Some(("mine", false)), Some(("theirs", false))),
            Ok(Action::Merge { executable: false })
        );
        let reconciliation = reconcile(
            &snapshot(&[("f", "base", false)]),
            &current(&[("f", "mine", false)]),
            &snapshot(&[("f", "theirs", false)]),
            &unmergeable,
        );
        assert_eq!(
            reconciliation.disagreements.values().collect::<Vec<_>>(),
            [&Disagreement::BothChanged(Mergeability::Format)]
        );
        assert_eq!(
            action(b, Some(("edited", false)), None),
            Err(Disagreement::RemovedButEdited)
        );
        assert_eq!(
            action(b, None, Some(("changed", false))),
            Err(Disagreement::DeletedButChanged)
        );
    }

    #[test]
    fn modes_are_reconciled_like_content() {
        let b = Some(("base", false));
        // Only the mode changes, on one side.
        assert_eq!(
            action(b, b, Some(("base", true))),
            Ok(Action::Mode { executable: true })
        );
        assert_eq!(action(b, Some(("base", true)), b), Ok(Action::Keep));
        // The project edits the content and the composition the mode.
        assert_eq!(
            action(b, Some(("mine", false)), Some(("base", true))),
            Ok(Action::Mode { executable: true })
        );
        // The composition changes the content and the project the mode.
        assert_eq!(
            action(b, Some(("base", true)), Some(("theirs", false))),
            Ok(Action::Write { executable: true })
        );
        // Both change the content, the project the mode too; and both
        // change the mode, which for a bit is the same change.
        assert_eq!(
            action(b, Some(("mine", true)), Some(("theirs", false))),
            Ok(Action::Merge { executable: true })
        );
        assert_eq!(
            action(b, Some(("mine", true)), Some(("theirs", true))),
            Ok(Action::Merge { executable: true })
        );
    }

    #[test]
    fn a_path_that_is_not_a_regular_file_disagrees() {
        let path = RelativePath::parse("f").unwrap();
        let reconciliation = reconcile(
            &snapshot(&[("f", "base", false)]),
            &BTreeMap::from([(path.clone(), Current::Other)]),
            &snapshot(&[("f", "theirs", false)]),
            &mergeable,
        );
        assert_eq!(
            reconciliation.disagreements.get(&path),
            Some(&Disagreement::NotAFile)
        );
    }

    #[test]
    fn the_whole_project_is_decided_in_path_order() {
        let reconciliation = reconcile(
            &snapshot(&[("b", "x", false), ("c", "x", false), ("d", "x", false)]),
            &current(&[("b", "x", false), ("c", "edited", false)]),
            &snapshot(&[("a", "new", false), ("b", "y", false), ("c", "x", false)]),
            &mergeable,
        );
        let actions: Vec<(&str, &Action)> = reconciliation
            .actions
            .iter()
            .map(|(path, action)| (path.as_str(), action))
            .collect();
        assert_eq!(
            actions,
            [
                ("a", &Action::Create),
                ("b", &Action::Write { executable: false }),
                ("c", &Action::Keep),
                ("d", &Action::Keep),
            ]
        );
        assert!(reconciliation.disagreements.is_empty());
    }

    fn n(number: i64) -> Value {
        Value::Number(number.to_string())
    }

    fn object(members: &[(&str, Value)]) -> Value {
        Value::Object(
            members
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        )
    }

    #[test]
    fn incoming_changes_are_planned_onto_the_projects_value() {
        let base = object(&[("a", n(1)), ("b", n(2)), ("c", n(3))]);
        let current = object(&[
            ("a", n(10)),
            ("b", n(2)),
            ("c", n(3)),
            ("mine", Value::Bool(true)),
        ]);
        let incoming = object(&[("a", n(1)), ("b", n(20)), ("d", n(4))]);
        let plan = merge_values(&base, &current, &incoming).unwrap();
        assert_eq!(
            plan.result,
            object(&[
                ("a", n(10)),
                ("b", n(20)),
                ("d", n(4)),
                ("mine", Value::Bool(true)),
            ])
        );
        // Only the composition's operations are edits: the project's edit
        // of `a` and its `mine` are already in its text.
        assert_eq!(plan.edits.len(), 3);
    }

    #[test]
    fn the_same_change_on_both_sides_is_one_change_and_different_ones_conflict() {
        let base = object(&[("a", n(1))]);
        let two = object(&[("a", n(2))]);
        let plan = merge_values(&base, &two, &two).unwrap();
        assert!(plan.edits.is_empty());
        assert_eq!(plan.result, two);

        let conflicts = merge_values(&base, &two, &object(&[("a", n(3))])).unwrap_err();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].path(), ["a"]);
        match conflicts[0].kind() {
            ConflictKind::Different { changes } => {
                assert_eq!(changes[0].0, [Contributor::Current]);
                assert_eq!(changes[1].0, [Contributor::Incoming]);
                assert!(matches!(changes[0].1, ValueChange::ValueChange { .. }));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            conflicts[0].to_string(),
            "/a: the project changes 1 to 2; the composition changes 1 to 3"
        );
    }

    #[test]
    fn appends_to_one_list_on_both_sides_still_conflict() {
        // Unlike a composition of units, a reconciliation compares lists
        // whole: joining them would drop the composition's items, since
        // only its own changes are planned onto the project's value.
        let list =
            |items: &[i64]| object(&[("a", Value::Array(items.iter().map(|i| n(*i)).collect()))]);
        let conflicts = merge_values(&list(&[1]), &list(&[1, 2]), &list(&[1, 3])).unwrap_err();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].path(), ["a"]);
    }
}
