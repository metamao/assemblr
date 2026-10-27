//! Composing several amendments of one JSON or JSONC file: the domain's
//! value merge, fed by the JSONC reader and written through the JSONC
//! editor, with two checks around it (see
//! `docs/domain/amendments-and-composition.md` and
//! `docs/adr/0015-value-merge.md`).
//!
//! The inputs are text: the provider's file and each amender's copy. Each
//! copy is derived against the base, the
//! operations are combined and planned against the provider's file, and the
//! edits are applied to its text. Then:
//!
//! - the **projection check** reads the edited text back and requires it to
//!   hold exactly the planned value, so that a layout the editor misjudged
//!   is a refusal, never a silently different file;
//! - the **round-trip guard** replays each amender's own operations on its
//!   base and requires its copy's value, member order aside. A copy whose
//!   operations do not reproduce it means the derivation is wrong, so the
//!   composition is refused (see [`MergeFailure::RoundTrip`]). What the
//!   value model cannot carry on purpose, a comment or a member order, is
//!   different: it is reported as a note, never dropped in silence.

use std::collections::BTreeMap;
use std::fmt;

use super::jsonc_edit::{EditError, ValueError, apply, comments_of, parse_value};
use crate::domain::{
    Conflict, Contributor, Derived, Lists, Operation, TextMerge, UnitIdentity, Value, combine,
    derive, describe_change, describe_lines, merge_values, plan,
};
#[cfg(test)]
use crate::domain::{InsertTie, merge_text};

/// Composes `copies`, each amender's copy, onto `base`, the provider's
/// file. The amenders are ordered by identity, whatever
/// order `copies` lists them in.
#[cfg(test)]
pub(crate) fn merge_json(
    base: &str,
    copies: &[(&UnitIdentity, &str)],
) -> Result<Merged, MergeFailure> {
    let copies: Vec<(&UnitIdentity, &str, &str)> = copies
        .iter()
        .map(|(amender, copy)| (*amender, base, *copy))
        .collect();
    merge_json_from(base, &copies)
}

/// [`merge_json`] where each copy has its own base: the provider's file at
/// the version the copy was taken from. Each amender's operations are derived against its own base and
/// planned against `provider`, the file at the resolved version, so that
/// the provider's own changes between the two stay and a change that
/// collides with an amender's is a conflict.
#[cfg(test)]
pub(crate) fn merge_json_from(
    provider: &str,
    copies: &[(&UnitIdentity, &str, &str)],
) -> Result<Merged, MergeFailure> {
    let provider_value = parse_value(provider).map_err(|error| MergeFailure::Unreadable {
        amender: None,
        error,
    })?;
    let mut sets: BTreeMap<UnitIdentity, Vec<Operation>> = BTreeMap::new();
    let mut notes = Vec::new();
    for (amender, base, copy) in copies {
        let base_value = if *base == provider {
            provider_value.clone()
        } else {
            parse_value(base).map_err(|error| MergeFailure::Unreadable {
                amender: Some((*amender).clone()),
                error,
            })?
        };
        let copy_value = parse_value(copy).map_err(|error| MergeFailure::Unreadable {
            amender: Some((*amender).clone()),
            error,
        })?;
        let derived = derive(&base_value, &copy_value);
        notes.extend(not_carried(amender, base, copy, &derived));
        // The guard: the amender's operations alone, replayed on its base,
        // must give its copy's value, member order aside, which the
        // operations do not carry and is reported above.
        round_trip(amender, &base_value, &copy_value, &derived.operations)?;
        sets.insert((*amender).clone(), derived.operations);
    }
    let combined = combine(&sets, Lists::Union).map_err(MergeFailure::Conflicts)?;
    let planned = plan(&provider_value, combined).map_err(MergeFailure::Conflicts)?;
    let text = apply(provider, &planned.edits).map_err(MergeFailure::Edit)?;
    let written = parse_value(&text).map_err(|error| MergeFailure::Projection {
        detail: error.to_string(),
    })?;
    if written != planned.result {
        return Err(MergeFailure::Projection {
            detail: format!(
                "the edited text holds {}, not the planned {}",
                written.brief(),
                planned.result.brief()
            ),
        });
    }
    notes.sort();
    notes.dedup();
    Ok(Merged { text, notes })
}

/// Composes `copies`, each amender's copy of a text file, onto `base`, the
/// provider's file, by a line-level three-way merge
/// (see [`merge_text`]). Changes in different places are all kept,
/// identical changes are one change, insertions at one position are all
/// kept in the order of the amenders' identities, and overlapping or
/// touching differing changes are conflicts that name the lines and the
/// amenders. The amenders are ordered by identity, whatever order `copies`
/// lists them in.
#[cfg(test)]
pub(crate) fn merge_text_files(
    base: &str,
    copies: &[(&UnitIdentity, &str)],
) -> Result<Merged, MergeFailure> {
    let copies: Vec<(&UnitIdentity, &str, &str)> = copies
        .iter()
        .map(|(amender, copy)| (*amender, base, *copy))
        .collect();
    let provider_unit = copies[0].0.clone();
    merge_text_files_from(&provider_unit, base, &copies)
}

/// [`merge_text_files`] where each copy has its own base (see
/// [`merge_json_from`]). A copy whose base is not `provider` is first moved
/// onto it, by a three-way merge of the copy, its base, and the provider's
/// file, so that what remains of every copy is the amender's own
/// contribution to the provider's file. Changes that overlap or touch
/// conflict; insertions both make at one position are both kept, the
/// provider's first (see [`RebaseSide`]).
#[cfg(test)]
pub(crate) fn merge_text_files_from(
    provider_unit: &UnitIdentity,
    provider: &str,
    copies: &[(&UnitIdentity, &str, &str)],
) -> Result<Merged, MergeFailure> {
    let mut moved: Vec<(UnitIdentity, String)> = Vec::new();
    for (amender, base, copy) in copies {
        // The guard of the JSON merge, for text: an amender's changes
        // alone, replayed on its base, must give its copy back, byte for
        // byte.
        let alone = merge_text(base, &[((*amender).clone(), *copy)], InsertTie::Chain);
        if alone.text().as_deref() != Some(copy) {
            return Err(MergeFailure::RoundTrip {
                amender: (*amender).clone(),
                detail: "its line changes replayed on its base do not give the copy".to_owned(),
            });
        }
        if *base == provider {
            moved.push(((*amender).clone(), (*copy).to_owned()));
            continue;
        }
        let sides = [
            (RebaseSide::Amender((*amender).clone()), *copy),
            (RebaseSide::Provider(provider_unit.clone()), provider),
        ];
        let rebased = merge_text(base, &sides, InsertTie::Chain);
        match rebased.text() {
            Some(text) => moved.push(((*amender).clone(), text)),
            None => {
                return Err(MergeFailure::Rebase {
                    amender: (*amender).clone(),
                    conflicts: conflict_reports(&rebased, RebaseSide::unit),
                });
            }
        }
    }
    let sides: Vec<(UnitIdentity, &str)> = moved
        .iter()
        .map(|(amender, text)| (amender.clone(), text.as_str()))
        .collect();
    let merge = merge_text(provider, &sides, InsertTie::Chain);
    match merge.text() {
        Some(text) => Ok(Merged {
            text,
            notes: merge
                .chained()
                .iter()
                .map(|chained| MergeNote::Chained {
                    amenders: chained.sides.clone(),
                    after_line: chained.after_line,
                })
                .collect(),
        }),
        None => Err(MergeFailure::TextConflicts(conflict_reports(
            &merge,
            |unit| unit,
        ))),
    }
}

/// The two sides of moving an amender's copy onto the provider's file at
/// the version resolved. The provider orders first, so that when both
/// insert at one position the provider's lines come before the amender's:
/// the provider's release is the file the amender now amends, and nothing
/// either side wrote is lost.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum RebaseSide {
    Provider(UnitIdentity),
    Amender(UnitIdentity),
}

impl RebaseSide {
    pub(super) fn unit(&self) -> &UnitIdentity {
        match self {
            Self::Provider(unit) | Self::Amender(unit) => unit,
        }
    }
}

/// The conflicts of a text merge, for a message, naming only the sides that
/// changed the lines, each by the unit `unit` gives for it, in the order of
/// their identities.
pub(super) fn conflict_reports<C>(
    merge: &TextMerge<'_, C>,
    unit: impl Fn(&C) -> &UnitIdentity,
) -> Vec<TextConflictReport> {
    merge
        .conflicts()
        .map(|conflict| {
            let mut sides: Vec<(UnitIdentity, String)> = conflict
                .sides
                .iter()
                .zip(&conflict.changed)
                .filter(|(_, changed)| **changed)
                .map(|((side, lines), _)| {
                    (unit(side).clone(), describe_change(&conflict.base, lines))
                })
                .collect();
            sides.sort_by(|left, right| left.0.cmp(&right.0));
            TextConflictReport {
                base_lines: conflict.base_lines.clone(),
                sides,
            }
        })
        .collect()
}

/// A line-level conflict between amenders, for a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextConflictReport {
    /// The base lines the changes cover, as indexes into its lines.
    pub(super) base_lines: std::ops::Range<usize>,
    /// What each amender holds there, briefly, ordered by identity.
    pub(super) sides: Vec<(UnitIdentity, String)>,
}

impl fmt::Display for TextConflictReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&describe_lines(&self.base_lines))?;
        f.write_str(":")?;
        for (index, (amender, held)) in self.sides.iter().enumerate() {
            write!(f, "{} {amender} {held}", if index == 0 { "" } else { ";" })?;
        }
        Ok(())
    }
}

/// What the value model cannot carry from `copy`, an amender's copy of
/// `base`: the order of members it changes and the comments it
/// adds. They are reported, never dropped in silence.
pub(super) fn not_carried(
    amender: &UnitIdentity,
    base: &str,
    copy: &str,
    derived: &Derived,
) -> Vec<MergeNote> {
    let mut notes = Vec::new();
    for path in &derived.reordered {
        notes.push(MergeNote::NotCarried {
            amender: amender.clone(),
            what: format!(
                "the order of the members of {}",
                crate::domain::show_path(path)
            ),
        });
    }
    // Comments the copy adds are not values, and the merged file is the
    // provider's text.
    let base_comments = comments_of(base);
    for comment in comments_of(copy) {
        if !base_comments.contains(&comment) {
            let shown: String = comment.chars().take(60).collect();
            notes.push(MergeNote::NotCarried {
                amender: amender.clone(),
                what: format!("the comment {shown:?}"),
            });
        }
    }
    notes
}

/// What a merge would not carry from `copy`, the copy of `amender` of
/// `base`, if another unit amended the same file too. A copy
/// alone takes the file's place byte for byte, so nothing is lost until a
/// second amender joins; the unit's author hears of it early, from their
/// own build or check, rather than only when a consumer composes. Sides
/// that are not JSON have nothing to report here.
pub(crate) fn unshared_notes(base: &str, amender: &UnitIdentity, copy: &str) -> Vec<MergeNote> {
    let (Ok(base_value), Ok(copy_value)) = (parse_value(base), parse_value(copy)) else {
        return Vec::new();
    };
    let derived = derive(&base_value, &copy_value);
    let mut notes = not_carried(amender, base, copy, &derived);
    notes.sort();
    notes.dedup();
    notes
}

/// Requires that `operations`, the ones derived from `copy` against
/// `base`, give `copy` back when replayed on `base` alone, member order
/// aside. They always do unless the derivation is wrong, and a wrong
/// derivation must never end in a successful composition: the file would
/// not hold what the copy asked for.
pub(super) fn round_trip(
    amender: &UnitIdentity,
    base: &Value,
    copy: &Value,
    operations: &[Operation],
) -> Result<(), MergeFailure> {
    let own = BTreeMap::from([(amender.clone(), operations.to_vec())]);
    let detail = match combine(&own, Lists::Union).and_then(|combined| plan(base, combined)) {
        Ok(planned) if sorted(&planned.result) == sorted(copy) => return Ok(()),
        Ok(planned) => format!(
            "replayed on its base they give {}, not {}",
            planned.result.brief(),
            copy.brief()
        ),
        Err(conflicts) => {
            let shown: Vec<String> = conflicts.iter().map(ToString::to_string).collect();
            format!("replaying them on its base fails: {}", shown.join("; "))
        }
    };
    Err(MergeFailure::RoundTrip {
        amender: amender.clone(),
        detail,
    })
}

/// `value` with every object's members sorted by key, for a comparison
/// that ignores member order.
fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(members) => {
            let mut members: Vec<(String, Value)> = members
                .iter()
                .map(|(key, value)| (key.clone(), sorted(value)))
                .collect();
            members.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(members)
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        scalar => scalar.clone(),
    }
}

/// A composed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    text: String,
    notes: Vec<MergeNote>,
}

impl Merged {
    pub(super) fn new(text: String, notes: Vec<MergeNote>) -> Self {
        Self { text, notes }
    }

    /// The composed text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// What was reported along the way, sorted.
    pub fn notes(&self) -> &[MergeNote] {
        &self.notes
    }

    /// The composed text, consuming the result.
    pub fn into_text(self) -> String {
        self.text
    }
}

/// Something the composition did that no copy asked for, or could not carry
/// from a copy, reported rather than done in silence.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MergeNote {
    /// Several amenders add lines at one position of a text file; all are
    /// kept, in the order of their identities.
    Chained {
        /// The amenders whose lines were kept, in the order they stand.
        amenders: Vec<UnitIdentity>,
        /// The number of lines of the provider's file before the position.
        after_line: usize,
    },
    /// Part of an amender's copy is not expressed by its operations.
    NotCarried {
        /// The amender.
        amender: UnitIdentity,
        /// What is not carried.
        what: String,
    },
}

impl fmt::Display for MergeNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Chained {
                amenders,
                after_line,
            } => {
                let names: Vec<String> = amenders.iter().map(ToString::to_string).collect();
                let place = if *after_line == 0 {
                    "at the start".to_owned()
                } else {
                    format!("after line {after_line}")
                };
                write!(
                    f,
                    "{} each add lines {place}; all are kept, in the order of the units' \
                     identities",
                    names.join(", ")
                )
            }
            Self::NotCarried { amender, what } => write!(
                f,
                "not carried from the copy of {amender}: {what}; the value model carries members \
                 and values, not comments or order"
            ),
        }
    }
}

/// Why several amendments of one file cannot be composed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeFailure {
    /// The provider's file (`amender` is `None`) or an amender's copy is
    /// not a JSON document the merge can read.
    Unreadable {
        amender: Option<UnitIdentity>,
        error: ValueError,
    },
    /// The amendments conflict, each conflict with its path and amenders,
    /// sorted by path.
    Conflicts(Vec<Conflict>),
    /// The amendments of a text file change the same lines differently,
    /// each conflict with its lines and amenders, in the order of the lines.
    TextConflicts(Vec<TextConflictReport>),
    /// An amendment based on another version of the provider than the one
    /// resolved changes lines that the provider changed too, between the
    /// two versions. The lines are those of the amender's base.
    Rebase {
        /// The amender whose contribution collides.
        amender: UnitIdentity,
        /// The collisions, in the order of the lines.
        conflicts: Vec<TextConflictReport>,
    },
    /// The editor could not locate an edit in the provider's file.
    Edit(EditError),
    /// The edited text does not hold the planned value.
    Projection { detail: String },
    /// The operations derived from an amender's copy do not reproduce its
    /// value when replayed on its base. That is a defect in the
    /// derivation, never something the copy can fix, and it is an error
    /// because composing with it would write a file the copy did not ask
    /// for.
    RoundTrip {
        /// The amender whose copy is not reproduced.
        amender: UnitIdentity,
        /// What differs.
        detail: String,
    },
}

impl fmt::Display for MergeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable {
                amender: None,
                error,
            } => write!(f, "the provider's file cannot be read as JSON: {error}"),
            Self::Unreadable {
                amender: Some(amender),
                error,
            } => write!(f, "the copy of {amender} cannot be read as JSON: {error}"),
            Self::Conflicts(conflicts) => {
                write!(f, "the amendments conflict:")?;
                for conflict in conflicts {
                    write!(f, "\n  {conflict}")?;
                }
                Ok(())
            }
            Self::TextConflicts(conflicts) => {
                write!(
                    f,
                    "the amendments change the same lines of the provider's file differently:"
                )?;
                for conflict in conflicts {
                    write!(f, "\n  {conflict}")?;
                }
                Ok(())
            }
            Self::Rebase { amender, conflicts } => {
                write!(
                    f,
                    "the amendment of {amender} and the provider change the same lines between the \
                     amendment's base and the resolved version (lines of the base):"
                )?;
                for conflict in conflicts {
                    write!(f, "\n  {conflict}")?;
                }
                Ok(())
            }
            Self::Edit(error) => write!(f, "the merged file could not be written: {error}"),
            Self::Projection { detail } => write!(
                f,
                "the merged file was refused by the projection check: {detail}"
            ),
            Self::RoundTrip { amender, detail } => write!(
                f,
                "the operations derived from the copy of {amender} do not reproduce its value \
                 ({detail}); the composition was refused rather than written without part of the \
                 copy"
            ),
        }
    }
}

impl std::error::Error for MergeFailure {}

/// Reconciles a consumer project's JSON or JSONC file: the changes the
/// incoming composition makes against `base` are edited into `current`,
/// the project's text, whose own changes and layout stay. The edited text
/// must parse back to the planned value.
pub(crate) fn reconcile_json(
    base: &str,
    current: &str,
    incoming: &str,
) -> Result<Reconciled, ReconcileFailure> {
    let read = |text: &str, side: Option<Contributor>| {
        parse_value(text).map_err(|error| ReconcileFailure::Unreadable { side, error })
    };
    let base_value = read(base, None)?;
    let current_value = read(current, Some(Contributor::Current))?;
    let incoming_value = read(incoming, Some(Contributor::Incoming))?;
    let planned = merge_values(&base_value, &current_value, &incoming_value)
        .map_err(ReconcileFailure::Conflicts)?;
    // What the composition changed beyond values is not edited into the
    // project's text, which keeps its own layout: said, not dropped.
    let derived = derive(&base_value, &incoming_value);
    let mut notes: Vec<String> = derived
        .reordered
        .iter()
        .map(|path| {
            format!(
                "the order of the members of {}",
                crate::domain::show_path(path)
            )
        })
        .collect();
    let base_comments = comments_of(base);
    for comment in comments_of(incoming) {
        if !base_comments.contains(&comment) {
            let shown: String = comment.chars().take(60).collect();
            notes.push(format!("the comment {shown:?}"));
        }
    }
    notes.sort();
    notes.dedup();
    let text = apply(current, &planned.edits).map_err(ReconcileFailure::Edit)?;
    let written = parse_value(&text).map_err(|error| ReconcileFailure::Projection {
        detail: error.to_string(),
    })?;
    if written != planned.result {
        return Err(ReconcileFailure::Projection {
            detail: format!(
                "the edited text holds {}, not the planned {}",
                written.brief(),
                planned.result.brief()
            ),
        });
    }
    Ok(Reconciled { text, notes })
}

/// A project's file reconciled with the composition's changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reconciled {
    /// The project's text with the composition's changes edited in.
    pub(crate) text: String,
    /// What the composition changed that the value model cannot carry, a
    /// member order or a comment, and that the project's file therefore
    /// does not have.
    pub(crate) notes: Vec<String>,
}

/// Why a consumer's file cannot be reconciled by the value merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileFailure {
    /// The base (`side` is `None`), the project's file, or the composed
    /// file is not a JSON document the merge can read.
    Unreadable {
        side: Option<Contributor>,
        error: ValueError,
    },
    /// The project and the composition change the same values, sorted by
    /// path.
    Conflicts(Vec<Conflict<Contributor>>),
    /// The editor could not locate an edit in the project's file.
    Edit(EditError),
    /// The edited text does not hold the planned value.
    Projection { detail: String },
}

impl fmt::Display for ReconcileFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { side: None, error } => {
                write!(f, "its base cannot be read as JSON: {error}")
            }
            Self::Unreadable {
                side: Some(Contributor::Current),
                error,
            } => write!(f, "the project's file cannot be read as JSON: {error}"),
            Self::Unreadable {
                side: Some(Contributor::Incoming),
                error,
            } => write!(f, "the composed file cannot be read as JSON: {error}"),
            Self::Conflicts(conflicts) => {
                write!(f, "the project and the composition change the same values:")?;
                for conflict in conflicts {
                    write!(f, "\n    {conflict}")?;
                }
                Ok(())
            }
            Self::Edit(error) => write!(f, "the merged file could not be written: {error}"),
            Self::Projection { detail } => write!(
                f,
                "the merged file was refused by the projection check: {detail}"
            ),
        }
    }
}

impl std::error::Error for ReconcileFailure {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]

    use super::{
        MergeFailure, MergeNote, merge_json, merge_text_files, merge_text_files_from, round_trip,
    };
    use crate::adapters::jsonc_edit::parse_value;
    use crate::domain::{UnitIdentity, derive};

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    const BASE: &str = "{\n  \"name\": \"worker\",\n  \"scripts\": {\n    \"dev\": \"wrangler dev\",\n    \"deploy\": \"wrangler deploy\"\n  },\n  \"dependencies\": {\n    \"hono\": \"^4\"\n  }\n}\n";

    #[test]
    fn independent_copies_compose_into_one_file_with_the_base_s_layout() {
        let api = BASE.replace(
            "    \"deploy\": \"wrangler deploy\"\n",
            "    \"deploy\": \"wrangler deploy\",\n    \"test\": \"vitest\"\n",
        );
        let db = BASE
            .replace(
                "    \"hono\": \"^4\"\n",
                "    \"drizzle-orm\": \"^0.44\",\n    \"hono\": \"^4\"\n",
            )
            .replace(
                "  }\n}\n",
                "  },\n  \"devDependencies\": {\n    \"drizzle-kit\": \"^0.31\"\n  }\n}\n",
            );
        let (db_id, api_id) = (identity("@acme/db"), identity("@acme/api"));

        let merged = merge_json(BASE, &[(&api_id, &api), (&db_id, &db)]).unwrap();

        assert_eq!(
            merged.text(),
            "{\n  \"name\": \"worker\",\n  \"scripts\": {\n    \"dev\": \"wrangler dev\",\n    \
             \"deploy\": \"wrangler deploy\",\n    \"test\": \"vitest\"\n  },\n  \"dependencies\": \
             {\n    \"drizzle-orm\": \"^0.44\",\n    \"hono\": \"^4\"\n  },\n  \"devDependencies\": \
             {\n    \"drizzle-kit\": \"^0.31\"\n  }\n}\n"
        );
        assert!(merged.notes().is_empty());
        // The order the copies are given in changes nothing.
        let again = merge_json(BASE, &[(&db_id, &db), (&api_id, &api)]).unwrap();
        assert_eq!(again, merged);
    }

    #[test]
    fn conflicts_name_the_path_and_every_amender() {
        let a = BASE.replace("\"^4\"", "\"^4.1\"");
        let b = BASE.replace("\"^4\"", "\"^4.2\"");
        let (a_id, b_id) = (identity("@acme/a"), identity("@acme/b"));
        let error = merge_json(BASE, &[(&b_id, &b), (&a_id, &a)]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "the amendments conflict:\n  /dependencies/hono: @acme/a changes \"^4\" to \"^4.1\"; \
             @acme/b changes \"^4\" to \"^4.2\""
        );
    }

    #[test]
    fn what_the_value_model_cannot_carry_is_reported() {
        let commented = BASE.replace(
            "    \"dev\": \"wrangler dev\",\n",
            "    // Local development.\n    \"dev\": \"wrangler dev --local\",\n",
        );
        let reordered = BASE.replace(
            "    \"dev\": \"wrangler dev\",\n    \"deploy\": \"wrangler deploy\"\n",
            "    \"deploy\": \"wrangler deploy\",\n    \"dev\": \"wrangler dev\"\n",
        );
        let (a_id, b_id) = (identity("@acme/a"), identity("@acme/b"));
        let merged = merge_json(BASE, &[(&a_id, &commented), (&b_id, &reordered)]).unwrap();
        assert!(merged.text().contains("\"dev\": \"wrangler dev --local\""));
        assert!(!merged.text().contains("Local development"));
        assert_eq!(
            merged.notes(),
            [
                MergeNote::NotCarried {
                    amender: a_id,
                    what: "the comment \"// Local development.\"".to_owned(),
                },
                MergeNote::NotCarried {
                    amender: b_id,
                    what: "the order of the members of /scripts".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn unreadable_sides_are_refused_with_the_reason() {
        let a_id = identity("@acme/a");
        let error = merge_json(BASE, &[(&a_id, "{ \"a\": 1, \"a\": 2 }")]).unwrap_err();
        assert!(matches!(
            error,
            MergeFailure::Unreadable {
                amender: Some(_),
                ..
            }
        ));
        assert!(
            error
                .to_string()
                .contains("the copy of @acme/a cannot be read as JSON")
        );
        let error = merge_json("not json", &[(&a_id, BASE)]).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("the provider's file cannot be read as JSON")
        );
    }

    #[test]
    fn a_derivation_that_does_not_reproduce_its_copy_is_an_error_not_a_note() {
        let (base, copy) = (
            parse_value(BASE).unwrap(),
            parse_value(&BASE.replace("wrangler dev", "wrangler dev --local")).unwrap(),
        );
        let amender = identity("@acme/a");
        let derived = derive(&base, &copy);
        // The derivation itself reproduces the copy.
        assert_eq!(
            round_trip(&amender, &base, &copy, &derived.operations),
            Ok(())
        );

        // A derivation that lost an operation does not, and the guard
        // refuses it by name instead of letting a file without the change
        // be composed.
        let error = round_trip(&amender, &base, &copy, &[]).unwrap_err();
        assert!(
            matches!(&error, MergeFailure::RoundTrip { amender: named, .. } if *named == amender)
        );
        let text = error.to_string();
        assert!(
            text.contains(
                "the operations derived from the copy of @acme/a do not reproduce its value"
            ),
            "{text}"
        );

        // A comment or a member order, which no operation carries on
        // purpose, stays a note and never reaches the guard as a failure.
        let reordered = BASE.replace(
            "    \"dev\": \"wrangler dev\",\n    \"deploy\": \"wrangler deploy\"\n",
            "    \"deploy\": \"wrangler deploy\",\n    \"dev\": \"wrangler dev\"\n",
        );
        let merged = merge_json(BASE, &[(&amender, &reordered)]).unwrap();
        assert_eq!(merged.notes().len(), 1);
    }

    #[test]
    fn across_versions_a_provider_s_insertion_and_an_amender_s_at_one_place_are_both_kept() {
        const OLD: &str = "# Guide\n\nRead the README.\n";
        let provider = format!("{OLD}\n## Security\n\nNo secrets.\n");
        let (guide, db, api) = (
            identity("@acme/guide"),
            identity("@acme/db"),
            identity("@acme/api"),
        );
        let db_copy = format!("{OLD}\n## Database\n\nUse DATABASE_URL.\n");
        // Taken from the version resolved, after the provider's section.
        let api_copy = format!("{provider}\n## API\n\nPort 8080.\n");

        let merged = merge_text_files_from(
            &guide,
            &provider,
            &[(&db, OLD, &db_copy), (&api, &provider, &api_copy)],
        )
        .unwrap();

        // The provider's section keeps its place, each amender's follows it,
        // and the amenders are in the order of their identities.
        assert_eq!(
            merged.text(),
            format!("{provider}\n## API\n\nPort 8080.\n\n## Database\n\nUse DATABASE_URL.\n")
        );

        // A change to a line the provider changed still collides, and names
        // both, in the order of their identities.
        let reworded = OLD.replace("Read the README.", "Read the docs.");
        let provider = OLD.replace("Read the README.", "Read the guide.");
        let error = merge_text_files_from(&guide, &provider, &[(&db, OLD, &reworded)]).unwrap_err();
        assert!(matches!(error, MergeFailure::Rebase { .. }), "{error}");
        let text = error.to_string();
        assert!(
            text.contains(
                "line 3: @acme/db sets \"Read the docs.\"; @acme/guide sets \"Read the guide.\""
            ),
            "{text}"
        );
    }

    #[test]
    fn a_text_conflict_names_only_the_amenders_that_changed_the_lines() {
        let base = "a\nb\nc\nd\ne\nf\ng\n";
        let (one, two, three) = (
            identity("@acme/one"),
            identity("@acme/two"),
            identity("@acme/three"),
        );
        let error = merge_text_files(
            base,
            &[
                (&one, "a\nb\nX\nd\ne\nf\ng\n"),
                (&two, "a\nb\nY\nd\ne\nf\ng\n"),
                (&three, "a\nb\nc\nd\ne\nf\ng\nh\n"),
            ],
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("line 3: @acme/one sets \"X\"; @acme/two sets \"Y\""),
            "{message}"
        );
        assert!(!message.contains("@acme/three"), "{message}");
    }
}
