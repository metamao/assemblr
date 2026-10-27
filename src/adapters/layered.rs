//! Layered composition of the amendments of one file (see
//! `docs/domain/amendments-and-composition.md`, "The base").
//!
//! An amendment's base is the provider's file as the amenders below its
//! amender compose it: the units in the amender's dependency closure that
//! amend the same file. Amenders that are not below one another are
//! siblings and combine side by side, as the flat merge always did. The
//! layering is recursive on the set of amenders: the maximal ones, those
//! below no other in the set, are combined on the composition of the rest.
//! Where siblings conflict, the contested value or lines are left between
//! conflict markers in that composition, so that a unit above both sees
//! the conflict in its base and resolves it by replacing the marker. A
//! marker that no unit above resolves is reported with the conflict it
//! stands for, and the caller decides what that means: an error for a
//! consumer composition, and the pending state for a unit build.
//!
//! Every input is text: the provider's file, each copy, and, for an amender
//! whose locked versions differ from the composition's, its base as
//! reconstructed at those versions.

use std::collections::{BTreeMap, BTreeSet};

use super::jsonc_edit::{apply, parse_value};
use super::merge::{
    MergeFailure, MergeNote, Merged, RebaseSide, TextConflictReport, conflict_reports, not_carried,
    round_trip,
};
use crate::domain::{
    Conflict, ConflictKind, Edit, InsertTie, Lists, Operation, TextMerge, TextSegment,
    UnitIdentity, Value, combine, derive, describe_change, describe_lines, merge_text, plan,
};

/// The member that marks a contested JSON value: an object with this one
/// member, whose value names each side's value by its amender.
pub(crate) const JSON_MARK: &str = "<<<<<<< conflict";
/// The lines that mark contested text, each followed by a side's identity
/// or, for the base section, by `base`.
const OPEN_MARK: &str = "<<<<<<< ";
const BASE_MARK: &str = "||||||| base";
const SEPARATOR: &str = "=======";
const CLOSE_MARK: &str = ">>>>>>> ";

/// One amender's part in the layering of a file.
#[derive(Debug, Clone)]
pub(crate) struct Layer<'a> {
    /// The amender.
    pub(crate) amender: &'a UnitIdentity,
    /// Its copy, as text.
    pub(crate) copy: &'a str,
    /// The other amenders of the file below it.
    pub(crate) below: BTreeSet<&'a UnitIdentity>,
    /// Its base, when it is not the composition of `below` here: the
    /// amender's locked versions differ from the composition's, and the
    /// caller reconstructed the base at them.
    pub(crate) base: Option<&'a str>,
}

/// A layered composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layered {
    pub(crate) merged: Merged,
    /// Conflicts between siblings that no amender above both resolved. Each
    /// is still marked in the text.
    pub(crate) unresolved: Vec<Unresolved>,
}

/// A conflict left marked in a layered composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Unresolved {
    /// A contested value, with the conflict as the value merge reports it.
    Value(Conflict),
    /// Contested lines, with the conflict as the text merge reports it.
    Lines(TextConflictReport),
}

impl std::fmt::Display for Unresolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Value(conflict) => conflict.fmt(f),
            Self::Lines(conflict) => conflict.fmt(f),
        }
    }
}

/// Whether `bytes` hold a conflict marker: a line that opens or closes a
/// text conflict, or the member that marks a JSON value. Only the lines
/// Assemblr writes count, opened or closed by a unit's identity or by the
/// `current` and `incoming` sides of a rebase or a reconciliation, so a
/// file that shows another tool's markers, such as `<<<<<<< HEAD` in a
/// code block, is not pending. The base section and the separator are
/// not looked for: a line of `=` signs underlines a Markdown heading.
pub(crate) fn has_markers(bytes: &[u8]) -> bool {
    let json_mark = format!("\"{JSON_MARK}\"");
    let marks = [
        format!("{OPEN_MARK}@"),
        format!("{OPEN_MARK}current"),
        format!("{CLOSE_MARK}@"),
        format!("{CLOSE_MARK}incoming"),
        json_mark,
    ];
    bytes.split(|byte| *byte == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let trimmed = line.trim_ascii_start();
        marks
            .iter()
            .any(|mark| trimmed.starts_with(mark.as_bytes()))
    })
}

/// The amenders in `set` that are below no other amender in `set`, and
/// the rest.
fn split<'a>(layers: &[Layer<'a>], set: &BTreeSet<usize>) -> (BTreeSet<usize>, BTreeSet<usize>) {
    let maximal: BTreeSet<usize> = set
        .iter()
        .copied()
        .filter(|candidate| {
            !set.iter().any(|other| {
                other != candidate && layers[*other].below.contains(layers[*candidate].amender)
            })
        })
        .collect();
    let inner = set.difference(&maximal).copied().collect();
    (maximal, inner)
}

/// The indexes of the amenders of `layers` that are below `layer`.
fn indexes_below(layers: &[Layer<'_>], layer: &Layer<'_>) -> BTreeSet<usize> {
    layers
        .iter()
        .enumerate()
        .filter(|(_, other)| layer.below.contains(other.amender))
        .map(|(index, _)| index)
        .collect()
}

/// Composes `layers`, the amendments of a JSON or JSONC file, onto
/// `provider`, the provider's file.
pub(crate) fn layer_json(provider: &str, layers: &[Layer<'_>]) -> Result<Layered, MergeFailure> {
    let mut notes = Vec::new();
    let all: BTreeSet<usize> = (0..layers.len()).collect();
    let (text, value, marked) = json_set(provider, layers, &all, &mut notes)?;
    let unresolved = marked
        .into_iter()
        .filter(|(path, _)| is_json_marker(value.at(path)))
        .map(|(_, conflict)| Unresolved::Value(conflict))
        .collect();
    notes.sort();
    notes.dedup();
    Ok(Layered {
        merged: Merged::new(text, notes),
        unresolved,
    })
}

/// A contested value as it is marked: an object with [`JSON_MARK`] alone.
fn is_json_marker(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Object(members)) => members.len() == 1 && members[0].0 == JSON_MARK,
        _ => false,
    }
}

type MarkedValues = Vec<(Vec<String>, Conflict)>;

/// The composition of the amenders `set` of a JSON file: its text, its
/// value, and the conflicts marked in it so far.
fn json_set(
    provider: &str,
    layers: &[Layer<'_>],
    set: &BTreeSet<usize>,
    notes: &mut Vec<MergeNote>,
) -> Result<(String, Value, MarkedValues), MergeFailure> {
    let read = |amender: Option<&UnitIdentity>, text: &str| {
        parse_value(text).map_err(|error| MergeFailure::Unreadable {
            amender: amender.cloned(),
            error,
        })
    };
    if set.is_empty() {
        let value = read(None, provider)?;
        return Ok((provider.to_owned(), value, Vec::new()));
    }
    let (maximal, inner) = split(layers, set);
    let (common, common_value, mut marked) = json_set(provider, layers, &inner, notes)?;
    // Each maximal amender's base: given, or composed here from what is
    // below it.
    let mut bases: BTreeMap<usize, String> = BTreeMap::new();
    for index in &maximal {
        let layer = &layers[*index];
        let base = match layer.base {
            Some(base) => base.to_owned(),
            None if layer.below.is_empty() => provider.to_owned(),
            None => json_set(provider, layers, &indexes_below(layers, layer), notes)?.0,
        };
        bases.insert(*index, base);
    }
    // One amender whose base is the composition below it: its copy takes
    // the file's place, byte for byte, as a single copy always did.
    if let [index] = *maximal.iter().copied().collect::<Vec<_>>().as_slice() {
        if bases[&index] == common {
            let layer = &layers[index];
            let value = read(Some(layer.amender), layer.copy)?;
            return Ok((layer.copy.to_owned(), value, marked));
        }
    }
    let mut sets: BTreeMap<UnitIdentity, Vec<Operation>> = BTreeMap::new();
    let mut copies: BTreeMap<&UnitIdentity, (Value, Value)> = BTreeMap::new();
    for index in &maximal {
        let layer = &layers[*index];
        let base_value = read(Some(layer.amender), &bases[index])?;
        let copy_value = read(Some(layer.amender), layer.copy)?;
        let derived = derive(&base_value, &copy_value);
        notes.extend(not_carried(
            layer.amender,
            &bases[index],
            layer.copy,
            &derived,
        ));
        round_trip(layer.amender, &base_value, &copy_value, &derived.operations)?;
        sets.insert(layer.amender.clone(), derived.operations);
        copies.insert(layer.amender, (base_value, copy_value));
    }
    let conflicts = match combine(&sets, Lists::Union) {
        Ok(combined) => match plan(&common_value, combined) {
            Ok(planned) => {
                let text = apply(&common, &planned.edits).map_err(MergeFailure::Edit)?;
                projection(&text, &planned.result)?;
                return Ok((text, planned.result, marked));
            }
            Err(conflicts) => conflicts,
        },
        Err(conflicts) => conflicts,
    };
    // A value the provider's file does not hold as an amender expects is
    // not a contest between siblings: the amender's copy is stale. Only
    // contests are marked.
    if conflicts
        .iter()
        .any(|conflict| matches!(conflict.kind(), ConflictKind::Unexpected { .. }))
    {
        return Err(MergeFailure::Conflicts(conflicts));
    }
    let paths: Vec<Vec<String>> = conflicts
        .iter()
        .map(|conflict| conflict.path().to_vec())
        .collect();
    let touches = |operation: &Operation| {
        paths
            .iter()
            .any(|path| operation.path().starts_with(path) || path.starts_with(operation.path()))
    };
    let kept: BTreeMap<UnitIdentity, Vec<Operation>> = sets
        .iter()
        .map(|(amender, operations)| {
            let kept = operations
                .iter()
                .filter(|operation| !touches(operation))
                .cloned()
                .collect();
            (amender.clone(), kept)
        })
        .collect();
    let planned = combine(&kept, Lists::Union)
        .and_then(|combined| plan(&common_value, combined))
        .map_err(MergeFailure::Conflicts)?;
    let text = apply(&common, &planned.edits).map_err(MergeFailure::Edit)?;
    projection(&text, &planned.result)?;
    let mut value = planned.result;
    let mut edits = Vec::new();
    for conflict in conflicts {
        let path = conflict.path().to_vec();
        let sides: Vec<(String, Value)> = maximal
            .iter()
            .filter_map(|index| {
                let layer = &layers[*index];
                let (base, copy) = copies.get(layer.amender)?;
                // A side that removed the value is `null`, so that the
                // author sees every side of the contest.
                let held = copy.at(&path);
                (base.at(&path) != held).then(|| {
                    (
                        layer.amender.to_string(),
                        held.cloned().unwrap_or(Value::Null),
                    )
                })
            })
            .collect();
        let marker = Value::Object(vec![(JSON_MARK.to_owned(), Value::Object(sides))]);
        edits.extend(place(&mut value, &path, marker));
        marked.push((path, conflict));
    }
    let text = apply(&text, &edits).map_err(MergeFailure::Edit)?;
    projection(&text, &value)?;
    Ok((text, value, marked))
}

/// The edits that put `marker` at `path` of `value`, creating the objects
/// on the way, and `value` with them applied.
fn place(value: &mut Value, path: &[String], marker: Value) -> Vec<Edit> {
    let mut edits = Vec::new();
    for depth in 1..=path.len() {
        let (parent, key) = (&path[..depth - 1], &path[depth - 1]);
        let last = depth == path.len();
        let Some(Value::Object(members)) = value.at(parent) else {
            break;
        };
        let new = if last {
            marker.clone()
        } else {
            Value::Object(Vec::new())
        };
        if let Some(position) = members.iter().position(|(member, _)| member == key) {
            if !last {
                continue;
            }
            edits.push(Edit::Set {
                path: path.to_vec(),
                value: new.clone(),
            });
            if let Some(Value::Object(members)) = value_at_mut(value, parent) {
                members[position].1 = new;
            }
        } else {
            let after = members.last().map(|(member, _)| member.clone());
            edits.push(Edit::Insert {
                parent: parent.to_vec(),
                key: key.clone(),
                value: new.clone(),
                after,
            });
            if let Some(Value::Object(members)) = value_at_mut(value, parent) {
                members.push((key.clone(), new));
            }
        }
    }
    edits
}

fn value_at_mut<'v>(value: &'v mut Value, path: &[String]) -> Option<&'v mut Value> {
    path.iter().try_fold(value, |value, key| match value {
        Value::Object(members) => members
            .iter_mut()
            .find(|(member, _)| member == key)
            .map(|(_, value)| value),
        _ => None,
    })
}

/// The projection check: the edited text must hold exactly `planned`.
fn projection(text: &str, planned: &Value) -> Result<(), MergeFailure> {
    let written = parse_value(text).map_err(|error| MergeFailure::Projection {
        detail: error.to_string(),
    })?;
    if written != *planned {
        return Err(MergeFailure::Projection {
            detail: format!(
                "the edited text holds {}, not the planned {}",
                written.brief(),
                planned.brief()
            ),
        });
    }
    Ok(())
}

/// Composes `layers`, the amendments of a text file, onto `provider`, the
/// file of `provider_unit`, by line.
pub(crate) fn layer_text(
    provider_unit: &UnitIdentity,
    provider: &str,
    layers: &[Layer<'_>],
) -> Result<Layered, MergeFailure> {
    let mut notes = Vec::new();
    let all: BTreeSet<usize> = (0..layers.len()).collect();
    let (text, marked) = text_set(provider_unit, provider, layers, &all, &mut notes)?;
    // A conflict is unresolved while its marker block stands in the text.
    // Two conflicts can have the same block, so each occurrence in the text
    // accounts for one of them.
    let mut remaining: BTreeMap<&str, usize> = BTreeMap::new();
    let unresolved = marked
        .iter()
        .filter(|(block, _)| {
            let left = remaining
                .entry(block.as_str())
                .or_insert_with(|| text.matches(block.as_str()).count());
            let stands = *left > 0;
            *left = left.saturating_sub(1);
            stands
        })
        .map(|(_, conflict)| Unresolved::Lines(conflict.clone()))
        .collect();
    notes.sort();
    notes.dedup();
    Ok(Layered {
        merged: Merged::new(text, notes),
        unresolved,
    })
}

/// Each conflict marked so far, with the whole block of its marker.
type MarkedLines = Vec<(String, TextConflictReport)>;

fn text_set(
    provider_unit: &UnitIdentity,
    provider: &str,
    layers: &[Layer<'_>],
    set: &BTreeSet<usize>,
    notes: &mut Vec<MergeNote>,
) -> Result<(String, MarkedLines), MergeFailure> {
    if set.is_empty() {
        return Ok((provider.to_owned(), Vec::new()));
    }
    let (maximal, inner) = split(layers, set);
    let (common, mut marked) = text_set(provider_unit, provider, layers, &inner, notes)?;
    let mut bases: BTreeMap<usize, String> = BTreeMap::new();
    for index in &maximal {
        let layer = &layers[*index];
        let base = match layer.base {
            Some(base) => base.to_owned(),
            None if layer.below.is_empty() => provider.to_owned(),
            None => {
                text_set(
                    provider_unit,
                    provider,
                    layers,
                    &indexes_below(layers, layer),
                    notes,
                )?
                .0
            }
        };
        bases.insert(*index, base);
    }
    if let [index] = *maximal.iter().copied().collect::<Vec<_>>().as_slice() {
        if bases[&index] == common {
            return Ok((layers[index].copy.to_owned(), marked));
        }
    }
    let mut moved: Vec<(UnitIdentity, String)> = Vec::new();
    for index in &maximal {
        let layer = &layers[*index];
        let base = bases[index].as_str();
        // An amender's changes alone, replayed on its base, must give its
        // copy back, byte for byte.
        let alone = merge_text(
            base,
            &[(layer.amender.clone(), layer.copy)],
            InsertTie::Chain,
        );
        if alone.text().as_deref() != Some(layer.copy) {
            return Err(MergeFailure::RoundTrip {
                amender: layer.amender.clone(),
                detail: "its line changes replayed on its base do not give the copy".to_owned(),
            });
        }
        if base == common {
            moved.push((layer.amender.clone(), layer.copy.to_owned()));
            continue;
        }
        // The copy is moved onto the composition below it: what remains is
        // the amender's own contribution to it.
        let sides = [
            (RebaseSide::Amender(layer.amender.clone()), layer.copy),
            (RebaseSide::Provider(provider_unit.clone()), common.as_str()),
        ];
        let rebased = merge_text(base, &sides, InsertTie::Chain);
        match rebased.text() {
            Some(text) => moved.push((layer.amender.clone(), text)),
            None => {
                return Err(MergeFailure::Rebase {
                    amender: layer.amender.clone(),
                    conflicts: conflict_reports(&rebased, RebaseSide::unit),
                });
            }
        }
    }
    let sides: Vec<(UnitIdentity, &str)> = moved
        .iter()
        .map(|(amender, text)| (amender.clone(), text.as_str()))
        .collect();
    let merge = merge_text(&common, &sides, InsertTie::Chain);
    notes.extend(merge.chained().iter().map(|chained| MergeNote::Chained {
        amenders: chained.sides.clone(),
        after_line: chained.after_line,
    }));
    let (text, conflicts) = write_marked(&common, &merge, BASE_MARK, Ord::cmp, |side| {
        side.to_string()
    });
    for (block, base_lines, sides) in conflicts {
        let mut report_sides: Vec<(UnitIdentity, String)> = sides;
        report_sides.sort_by(|left, right| left.0.cmp(&right.0));
        marked.push((
            block,
            TextConflictReport {
                base_lines,
                sides: report_sides,
            },
        ));
    }
    Ok((text, marked))
}

/// Each conflict a marked text holds: the whole block of its marker,
/// the base lines it covers, and what each side that changed them holds
/// there, briefly, in the merge's order of the sides.
type MarkedConflicts<C> = Vec<(String, std::ops::Range<usize>, Vec<(C, String)>)>;

/// The merged text with each conflict left between markers: the first
/// side that changed the lines, the base section under `base_mark`, then
/// each other side. Two sides are a diff3 conflict as editors show it;
/// with more, each separator names its side. The marker lines end as the
/// merged text's lines do.
fn write_marked<C: Clone + Ord>(
    common: &str,
    merge: &TextMerge<'_, C>,
    base_mark: &str,
    order: impl Fn(&C, &C) -> std::cmp::Ordering,
    label: impl Fn(&C) -> String,
) -> (String, MarkedConflicts<C>) {
    let eol = if common.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut text = String::new();
    let mut marked = Vec::new();
    for segment in merge.segments() {
        match segment {
            TextSegment::Text(lines) => lines.iter().for_each(|line| text.push_str(line)),
            TextSegment::Conflict(conflict) => {
                let mut changed: Vec<(&C, &[&str])> = conflict
                    .sides
                    .iter()
                    .zip(&conflict.changed)
                    .filter(|(_, changed)| **changed)
                    .map(|((side, lines), _)| (side, lines.as_slice()))
                    .collect();
                changed.sort_by(|left, right| order(left.0, right.0));
                let start = text.len();
                let mut section = |header: String, lines: &[&str]| {
                    text.push_str(&header);
                    text.push_str(eol);
                    lines.iter().for_each(|line| text.push_str(line));
                    if lines.last().is_some_and(|line| !line.ends_with('\n')) {
                        text.push_str(eol);
                    }
                };
                let Some(((first, first_lines), rest)) = changed.split_first() else {
                    continue;
                };
                section(format!("{OPEN_MARK}{}", label(first)), first_lines);
                section(base_mark.to_owned(), &conflict.base);
                for (side, lines) in rest {
                    let separator = if rest.len() == 1 {
                        SEPARATOR.to_owned()
                    } else {
                        format!("{SEPARATOR} {}", label(side))
                    };
                    section(separator, lines);
                }
                let last = rest.last().map(|(side, _)| *side).unwrap_or(first);
                text.push_str(&format!("{CLOSE_MARK}{}{eol}", label(last)));
                let sides = changed
                    .iter()
                    .map(|(side, lines)| ((*side).clone(), describe_change(&conflict.base, lines)))
                    .collect();
                marked.push((text[start..].to_owned(), conflict.base_lines.clone(), sides));
            }
        }
    }
    (text, marked)
}

/// The two sides of a copy's rebase by `unit update`: the copy as it is,
/// and the base after the update. The base's side orders first, so that
/// insertions both make at one position keep the base's lines before the
/// copy's, as composition does when it moves a copy onto a newer base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Rebase {
    Incoming,
    Current,
}

impl Rebase {
    /// The side's name in a marker.
    fn marker(self) -> &'static str {
        match self {
            Self::Incoming => "incoming (the base after the update)",
            Self::Current => "current (your copy)",
        }
    }

    /// The side's key in a JSON marker.
    fn key(self) -> &'static str {
        match self {
            Self::Incoming => "incoming",
            Self::Current => "current",
        }
    }
}

impl std::fmt::Display for Rebase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Incoming => "the update",
            Self::Current => "the copy",
        })
    }
}

/// The base section of a rebase marker.
const REBASE_BASE_MARK: &str = "||||||| base (before the update)";

/// A copy rebased onto its base after an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rebased {
    /// The copy with the update's changes merged in, each conflict between
    /// markers.
    pub(crate) text: String,
    /// Each conflict left marked, as the merge reports it.
    pub(crate) conflicts: Vec<String>,
}

/// Rebases `current`, a copy of the JSON or JSONC file `base`, onto
/// `incoming`, the base after an update: the changes the update makes
/// against the base are edited into the copy, whose own changes and layout
/// stay. A value both sides change differently is left in the copy as a
/// marker object naming each side's value, and reported.
pub(crate) fn rebase_json(
    base: &str,
    current: &str,
    incoming: &str,
) -> Result<Rebased, MergeFailure> {
    let read = |text: &str| {
        parse_value(text).map_err(|error| MergeFailure::Unreadable {
            amender: None,
            error,
        })
    };
    let (base_value, current_value, incoming_value) =
        (read(base)?, read(current)?, read(incoming)?);
    let sets = BTreeMap::from([
        (
            Rebase::Incoming,
            derive(&base_value, &incoming_value).operations,
        ),
        (
            Rebase::Current,
            derive(&base_value, &current_value).operations,
        ),
    ]);
    // Lists stay whole, as a consumer's reconciliation keeps them: only the
    // update's operations are planned, onto the copy's value.
    let only_incoming = |combined: Vec<crate::domain::Combined<Rebase>>| {
        combined
            .into_iter()
            .filter(|entry| entry.amenders() == [Rebase::Incoming])
            .collect::<Vec<_>>()
    };
    let conflicts = match combine(&sets, Lists::Whole) {
        Ok(combined) => match plan(&current_value, only_incoming(combined)) {
            Ok(planned) => {
                let text = apply(current, &planned.edits).map_err(MergeFailure::Edit)?;
                projection(&text, &planned.result)?;
                return Ok(Rebased {
                    text,
                    conflicts: Vec::new(),
                });
            }
            Err(conflicts) => conflicts,
        },
        Err(conflicts) => conflicts,
    };
    let paths: Vec<Vec<String>> = conflicts
        .iter()
        .map(|conflict| conflict.path().to_vec())
        .collect();
    let touches = |operation: &Operation| {
        paths
            .iter()
            .any(|path| operation.path().starts_with(path) || path.starts_with(operation.path()))
    };
    let kept: BTreeMap<Rebase, Vec<Operation>> = sets
        .iter()
        .map(|(side, operations)| {
            let kept = operations
                .iter()
                .filter(|operation| !touches(operation))
                .cloned()
                .collect();
            (*side, kept)
        })
        .collect();
    let planned = combine(&kept, Lists::Whole)
        .map(only_incoming)
        .and_then(|combined| plan(&current_value, combined))
        .map_err(|conflicts| {
            // Nothing outside the marked paths can conflict: the operations
            // that touch them were left out.
            let listed: Vec<String> = conflicts.iter().map(ToString::to_string).collect();
            MergeFailure::Projection {
                detail: format!(
                    "the changes outside the marked conflicts still conflict: {}",
                    listed.join("; ")
                ),
            }
        })?;
    let text = apply(current, &planned.edits).map_err(MergeFailure::Edit)?;
    projection(&text, &planned.result)?;
    let mut value = planned.result;
    let mut edits = Vec::new();
    let mut reported = Vec::new();
    for conflict in conflicts {
        let path = conflict.path().to_vec();
        let sides: Vec<(String, Value)> = [
            (Rebase::Current, &current_value),
            (Rebase::Incoming, &incoming_value),
        ]
        .into_iter()
        .filter(|(_, held)| held.at(&path) != base_value.at(&path))
        .map(|(side, held)| {
            (
                side.key().to_owned(),
                held.at(&path).cloned().unwrap_or(Value::Null),
            )
        })
        .collect();
        let marker = Value::Object(vec![(JSON_MARK.to_owned(), Value::Object(sides))]);
        edits.extend(place(&mut value, &path, marker));
        reported.push(conflict.to_string());
    }
    let text = apply(&text, &edits).map_err(MergeFailure::Edit)?;
    projection(&text, &value)?;
    Ok(Rebased {
        text,
        conflicts: reported,
    })
}

/// Rebases `current`, a copy of the text file `base`, onto `incoming`, the
/// base after an update, by line. Lines both sides change differently are
/// left in the copy between `current` and `incoming` markers, and reported.
pub(crate) fn rebase_text(base: &str, current: &str, incoming: &str) -> Rebased {
    let sides = [(Rebase::Incoming, incoming), (Rebase::Current, current)];
    let merge = merge_text(base, &sides, InsertTie::Chain);
    // The copy's side opens the marker, as editors show a diff3 conflict.
    let (text, marked) = write_marked(
        current,
        &merge,
        REBASE_BASE_MARK,
        |left, right| right.cmp(left),
        |side| side.marker().to_owned(),
    );
    let conflicts = marked
        .into_iter()
        .map(|(_, base_lines, sides)| {
            let held: Vec<String> = sides
                .iter()
                .map(|(side, change)| format!("{side} {change}"))
                .collect();
            format!("{}: {}", describe_lines(&base_lines), held.join("; "))
        })
        .collect();
    Rebased { text, conflicts }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(name: &str) -> UnitIdentity {
        UnitIdentity::parse(name).unwrap()
    }

    fn layer<'a>(
        amender: &'a UnitIdentity,
        copy: &'a str,
        below: &[&'a UnitIdentity],
    ) -> Layer<'a> {
        Layer {
            amender,
            copy,
            below: below.iter().copied().collect(),
            base: None,
        }
    }

    const BASE: &str =
        "{\n  \"name\": \"demo\",\n  \"scripts\": {\n    \"lint\": \"eslint\"\n  }\n}\n";

    #[test]
    fn siblings_that_conflict_are_marked_and_a_unit_above_both_resolves_the_marker() {
        let (a, b, app) = (unit("@acme/a"), unit("@acme/b"), unit("@acme/app"));
        let copy_a = "{\n  \"name\": \"demo\",\n  \"scripts\": {\n    \"lint\": \"eslint\",\n    \"verify\": \"a\"\n  }\n}\n";
        let copy_b = "{\n  \"name\": \"demo\",\n  \"scripts\": {\n    \"lint\": \"eslint\",\n    \"verify\": \"b\"\n  }\n}\n";
        let siblings = [layer(&a, copy_a, &[]), layer(&b, copy_b, &[])];
        let layered = layer_json(BASE, &siblings).unwrap();
        assert_eq!(layered.unresolved.len(), 1);
        let text = layered.merged.text();
        assert!(has_markers(text.as_bytes()));
        let path = vec!["scripts".to_owned(), "verify".to_owned()];
        let value = parse_value(text).unwrap();
        assert_eq!(
            value.at(&path).unwrap().to_string(),
            "{\"<<<<<<< conflict\":{\"@acme/a\":\"a\",\"@acme/b\":\"b\"}}"
        );
        let resolved = apply(
            text,
            &[Edit::Set {
                path,
                value: Value::String("a && b".to_owned()),
            }],
        )
        .unwrap();
        let all = [
            layer(&a, copy_a, &[]),
            layer(&b, copy_b, &[]),
            layer(&app, &resolved, &[&a, &b]),
        ];
        let layered = layer_json(BASE, &all).unwrap();
        assert!(layered.unresolved.is_empty());
        assert_eq!(layered.merged.text(), resolved);
        assert!(!has_markers(layered.merged.text().as_bytes()));
    }

    #[test]
    fn a_unit_above_overrides_a_value_the_unit_below_added_and_a_stale_override_is_refused() {
        let (ts, worker) = (unit("@acme/ts"), unit("@acme/worker"));
        let copy_ts = "{\n  \"name\": \"demo\",\n  \"scripts\": {\n    \"lint\": \"eslint\",\n    \"typecheck\": \"tsc\"\n  }\n}\n";
        let copy_worker = copy_ts.replace("\"tsc\"", "\"tsc --noEmit\"");
        let layers = [
            layer(&ts, copy_ts, &[]),
            layer(&worker, &copy_worker, &[&ts]),
        ];
        let layered = layer_json(BASE, &layers).unwrap();
        assert_eq!(layered.merged.text(), copy_worker);
        // ts changes the value under the override.
        let copy_ts2 = copy_ts.replace("\"tsc\"", "\"tsc -b\"");
        let layers = [
            layer(&ts, &copy_ts2, &[]),
            layer(&worker, &copy_worker, &[&ts]),
        ];
        let old_base = copy_ts;
        let stale = [
            layer(&ts, &copy_ts2, &[]),
            Layer {
                base: Some(old_base),
                ..layer(&worker, &copy_worker, &[&ts])
            },
        ];
        assert!(matches!(
            layer_json(BASE, &stale),
            Err(MergeFailure::Conflicts(_))
        ));
        // Without the recorded base, the override is derived against the
        // new composition and simply changes the value.
        assert_eq!(
            layer_json(BASE, &layers).unwrap().merged.text(),
            copy_worker
        );
    }

    #[test]
    fn text_siblings_are_marked_diff3_style_and_resolved_above() {
        let (d1, r2, app) = (unit("@acme/d1"), unit("@acme/r2"), unit("@acme/app"));
        let base = "# Agents\n\nRun verify.\n\n## Notes\n";
        let copy_d1 = "# Agents\n\nRun verify (d1).\n\n## Notes\n";
        let copy_r2 = "# Agents\n\nRun verify (r2).\n\n## Notes\n";
        let provider = unit("@acme/base");
        let siblings = [layer(&d1, copy_d1, &[]), layer(&r2, copy_r2, &[])];
        let layered = layer_text(&provider, base, &siblings).unwrap();
        assert_eq!(
            layered.merged.text(),
            "# Agents\n\n<<<<<<< @acme/d1\nRun verify (d1).\n||||||| base\nRun verify.\n=======\nRun verify (r2).\n>>>>>>> @acme/r2\n\n## Notes\n"
        );
        assert_eq!(layered.unresolved.len(), 1);
        let recorded = layered.merged.text().to_owned();
        let resolved = "# Agents\n\nRun verify (d1, r2).\n\n## Notes\n";
        let all = [
            layer(&d1, copy_d1, &[]),
            layer(&r2, copy_r2, &[]),
            layer(&app, resolved, &[&d1, &r2]),
        ];
        let layered = layer_text(&provider, base, &all).unwrap();
        assert!(layered.unresolved.is_empty());
        assert_eq!(layered.merged.text(), resolved);
        // A change elsewhere below flows through, the resolver's base being
        // the composition at its locked versions.
        let copy_d1b = "# Agents\n\nRun verify (d1).\n\n## Notes\n\n## D1\n";
        let all = [
            layer(&d1, copy_d1b, &[]),
            layer(&r2, copy_r2, &[]),
            Layer {
                base: Some(&recorded),
                ..layer(&app, resolved, &[&d1, &r2])
            },
        ];
        let layered = layer_text(&provider, base, &all).unwrap();
        assert_eq!(
            layered.merged.text(),
            "# Agents\n\nRun verify (d1, r2).\n\n## Notes\n\n## D1\n"
        );
    }

    #[test]
    fn a_side_that_removes_the_contested_value_is_null_in_the_marker() {
        let (a, b) = (unit("@acme/a"), unit("@acme/b"));
        let copy_a = BASE.replace("\"eslint\"", "\"eslint --fix\"");
        let copy_b = "{\n  \"name\": \"demo\",\n  \"scripts\": {}\n}\n";
        let siblings = [layer(&a, &copy_a, &[]), layer(&b, copy_b, &[])];
        let layered = layer_json(BASE, &siblings).unwrap();
        assert_eq!(layered.unresolved.len(), 1);
        let value = parse_value(layered.merged.text()).unwrap();
        assert_eq!(
            value
                .at(&["scripts".to_owned(), "lint".to_owned()])
                .unwrap()
                .to_string(),
            "{\"<<<<<<< conflict\":{\"@acme/a\":\"eslint --fix\",\"@acme/b\":null}}"
        );
    }

    #[test]
    fn two_text_conflicts_with_the_same_sides_are_resolved_one_at_a_time() {
        let (d1, r2, app) = (unit("@acme/d1"), unit("@acme/r2"), unit("@acme/app"));
        let provider = unit("@acme/base");
        let base = "# One

Run one.

# Two

Run two.
";
        let copy_d1 = "# One

Run one (d1).

# Two

Run two (d1).
";
        let copy_r2 = "# One

Run one (r2).

# Two

Run two (r2).
";
        let siblings = [layer(&d1, copy_d1, &[]), layer(&r2, copy_r2, &[])];
        let layered = layer_text(&provider, base, &siblings).unwrap();
        assert_eq!(layered.unresolved.len(), 2);
        let marked = layered.merged.text().to_owned();
        let second = marked.find("# Two").unwrap();
        // The unit above resolves the first conflict only: the second one,
        // opened by the same marker line, is the one still reported.
        let half = format!("# One\n\nRun one (both).\n\n{}", &marked[second..]);
        let all = [
            layer(&d1, copy_d1, &[]),
            layer(&r2, copy_r2, &[]),
            layer(&app, &half, &[&d1, &r2]),
        ];
        let layered = layer_text(&provider, base, &all).unwrap();
        assert_eq!(layered.unresolved.len(), 1);
        assert!(
            layered.unresolved[0].to_string().contains("two"),
            "{}",
            layered.unresolved[0]
        );
        // Two identical conflicts: resolving one leaves one.
        let base = "Run it.\n\n---\n\nRun it.\n";
        let copy_d1 = "Run it (d1).\n\n---\n\nRun it (d1).\n";
        let copy_r2 = "Run it (r2).\n\n---\n\nRun it (r2).\n";
        let siblings = [layer(&d1, copy_d1, &[]), layer(&r2, copy_r2, &[])];
        let layered = layer_text(&provider, base, &siblings).unwrap();
        assert_eq!(layered.unresolved.len(), 2);
        let marked = layered.merged.text().to_owned();
        let separator = marked.find("---").unwrap();
        let half = format!("Run it (both).\n\n{}", &marked[separator..]);
        let all = [
            layer(&d1, copy_d1, &[]),
            layer(&r2, copy_r2, &[]),
            layer(&app, &half, &[&d1, &r2]),
        ];
        let layered = layer_text(&provider, base, &all).unwrap();
        assert_eq!(layered.unresolved.len(), 1);
    }

    #[test]
    fn markers_are_found_in_text_and_json_but_not_in_underlines() {
        assert!(has_markers(b"a\n<<<<<<< @acme/a\nb\n"));
        assert!(has_markers(b"a\n>>>>>>> @acme/b\n"));
        assert!(has_markers(b"<<<<<<< current (your copy)\n"));
        assert!(has_markers(
            b">>>>>>> incoming (the base after the update)\n"
        ));
        assert!(has_markers(b"  \"<<<<<<< conflict\": {}\r\n"));
        assert!(!has_markers(b"Title\n=======\n||||||| base\n"));
        // Another tool's markers, shown in a code block, are content.
        assert!(!has_markers(
            b"```\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\n```\n"
        ));
    }

    #[test]
    fn a_copy_is_rebased_onto_its_new_base_and_contested_values_are_marked() {
        // JSON: the update adds a script and bumps a dependency the copy
        // also bumped; the copy's own addition and layout stay.
        let base = BASE;
        let current = "{\n  \"name\": \"demo\",\n  \"scripts\": {\n    \"lint\": \"eslint\",\n    \"verify\": \"mine\"\n  }\n}\n";
        let incoming = "{\n  \"name\": \"demo\",\n  \"scripts\": {\n    \"lint\": \"eslint --fix\",\n    \"test\": \"vitest\"\n  }\n}\n";
        let rebased = rebase_json(base, current, incoming).unwrap();
        assert!(rebased.conflicts.is_empty());
        assert_eq!(
            rebased.text,
            "{\n  \"name\": \"demo\",\n  \"scripts\": {\n    \"lint\": \"eslint --fix\",\n    \"test\": \"vitest\",\n    \"verify\": \"mine\"\n  }\n}\n"
        );
        // A value both change differently is marked, naming each side.
        let contested = "{\n  \"name\": \"demo\",\n  \"scripts\": {\n    \"lint\": \"biome\",\n    \"verify\": \"mine\"\n  }\n}\n";
        let rebased = rebase_json(base, contested, incoming).unwrap();
        assert_eq!(
            rebased.conflicts,
            [
                "/scripts/lint: the update changes \"eslint\" to \"eslint --fix\"; the copy changes \"eslint\" to \"biome\""
            ]
        );
        assert!(has_markers(rebased.text.as_bytes()));
        let value = parse_value(&rebased.text).unwrap();
        let path = vec!["scripts".to_owned(), "lint".to_owned()];
        assert_eq!(
            value.at(&path).unwrap().to_string(),
            "{\"<<<<<<< conflict\":{\"current\":\"biome\",\"incoming\":\"eslint --fix\"}}"
        );
        assert_eq!(
            value
                .at(&["scripts".to_owned(), "test".to_owned()])
                .unwrap()
                .to_string(),
            "\"vitest\""
        );

        // Text: the update's change elsewhere flows through; the lines both
        // change are left between markers, the copy's side first.
        let base = "# Title\n\nOne.\n\nTwo.\n";
        let current = "# Title\n\nOne, mine.\n\nTwo.\n";
        let incoming = "# New title\n\nOne.\n\nTwo.\n";
        let rebased = rebase_text(base, current, incoming);
        assert!(rebased.conflicts.is_empty());
        assert_eq!(rebased.text, "# New title\n\nOne, mine.\n\nTwo.\n");
        let incoming = "# Title\n\nOne, theirs.\n\nTwo.\n";
        let rebased = rebase_text(base, current, incoming);
        assert_eq!(
            rebased.conflicts,
            ["line 3: the copy sets \"One, mine.\"; the update sets \"One, theirs.\""]
        );
        assert_eq!(
            rebased.text,
            "# Title\n\n<<<<<<< current (your copy)\nOne, mine.\n||||||| base (before the update)\nOne.\n=======\nOne, theirs.\n>>>>>>> incoming (the base after the update)\n\nTwo.\n"
        );
        assert!(has_markers(rebased.text.as_bytes()));
    }
}
