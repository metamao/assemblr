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
//! reconstructed at those versions. A unit build renders templates before
//! layering; `unit amend` and `unit update` layer the sources.

use std::collections::{BTreeMap, BTreeSet};

use super::jsonc_edit::{apply, parse_value};
use super::merge::{
    MergeFailure, MergeNote, Merged, RebaseSide, TextConflictReport, conflict_reports, not_carried,
    round_trip,
};
use crate::domain::{
    Conflict, ConflictKind, Edit, InsertTie, Lists, Operation, TextSegment, UnitIdentity, Value,
    combine, derive, describe_change, merge_text, plan,
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
/// text conflict, or the member that marks a JSON value. The base section
/// and the separator are not looked for: a line of `=` signs underlines a
/// Markdown heading.
pub(crate) fn has_markers(bytes: &[u8]) -> bool {
    let json_mark = format!("\"{JSON_MARK}\"");
    bytes.split(|byte| *byte == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let trimmed = line.trim_ascii_start();
        trimmed.starts_with(OPEN_MARK.as_bytes())
            || trimmed.starts_with(CLOSE_MARK.as_bytes())
            || trimmed.starts_with(json_mark.as_bytes())
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
    if let [index] = *maximal.iter().copied().collect::<Vec<_>>().as_slice()
        && bases[&index] == common
    {
        let layer = &layers[index];
        let value = read(Some(layer.amender), layer.copy)?;
        return Ok((layer.copy.to_owned(), value, marked));
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
                let held = copy.at(&path)?;
                (base.at(&path) != Some(held)).then(|| (layer.amender.to_string(), held.clone()))
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
    let unresolved = marked
        .into_iter()
        .filter(|(header, _)| text.lines().any(|line| line == header))
        .map(|(_, conflict)| Unresolved::Lines(conflict))
        .collect();
    notes.sort();
    notes.dedup();
    Ok(Layered {
        merged: Merged::new(text, notes),
        unresolved,
    })
}

/// Each conflict marked so far, by the line that opens its marker.
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
    if let [index] = *maximal.iter().copied().collect::<Vec<_>>().as_slice()
        && bases[&index] == common
    {
        return Ok((layers[index].copy.to_owned(), marked));
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
    let eol = if common.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut text = String::new();
    for segment in merge.segments() {
        match segment {
            TextSegment::Text(lines) => lines.iter().for_each(|line| text.push_str(line)),
            TextSegment::Conflict(conflict) => {
                let changed: Vec<(&UnitIdentity, &[&str])> = conflict
                    .sides
                    .iter()
                    .zip(&conflict.changed)
                    .filter(|(_, changed)| **changed)
                    .map(|((side, lines), _)| (side, lines.as_slice()))
                    .collect();
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
                let header = format!("{OPEN_MARK}{first}");
                section(header.clone(), first_lines);
                section(BASE_MARK.to_owned(), &conflict.base);
                // Two sides are a diff3 conflict as editors show it; with
                // more, each separator names its side.
                for (side, lines) in rest {
                    let separator = if rest.len() == 1 {
                        SEPARATOR.to_owned()
                    } else {
                        format!("{SEPARATOR} {side}")
                    };
                    section(separator, lines);
                }
                let last = rest.last().map(|(side, _)| *side).unwrap_or(first);
                text.push_str(&format!("{CLOSE_MARK}{last}{eol}"));
                let mut report_sides: Vec<(UnitIdentity, String)> = changed
                    .iter()
                    .map(|(side, lines)| ((*side).clone(), describe_change(&conflict.base, lines)))
                    .collect();
                report_sides.sort_by(|left, right| left.0.cmp(&right.0));
                marked.push((
                    header,
                    TextConflictReport {
                        base_lines: conflict.base_lines.clone(),
                        sides: report_sides,
                    },
                ));
            }
        }
    }
    Ok((text, marked))
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
    fn markers_are_found_in_text_and_json_but_not_in_underlines() {
        assert!(has_markers(b"a\n<<<<<<< @acme/a\nb\n"));
        assert!(has_markers(b"  \"<<<<<<< conflict\": {}\r\n"));
        assert!(!has_markers(b"Title\n=======\n||||||| base\n"));
    }
}
