//! Composing several amendments of one JSON or JSONC file: the domain's
//! value merge, fed by the JSONC reader and written through the JSONC
//! editor, with two checks around it (see
//! `docs/domain/amendments-and-composition.md` and
//! `docs/adr/0015-value-merge.md`).
//!
//! The inputs are rendered text: the provider's file and each amender's
//! copy, both rendered with the same provider values, the copy with the
//! amender's own as `own`. Each copy is derived against the base, the
//! operations are combined and planned against the provider's file, and the
//! edits are applied to its text. Then:
//!
//! - the **projection check** reads the edited text back and requires it to
//!   hold exactly the planned value, so that a layout the editor misjudged
//!   is a refusal, never a silently different file;
//! - the **round-trip guard** replays each amender's own operations on its
//!   base and compares the result with its copy, layout aside; what differs
//!   is something the value model cannot carry, a comment or a member
//!   order, and is reported as a note rather than dropped in silence.

use std::collections::BTreeMap;
use std::fmt;

use super::jsonc_edit::{EditError, ValueError, apply, comments_of, parse_value};
use crate::domain::{Conflict, Operation, UnitIdentity, Value, combine, derive, plan};

/// Composes `copies`, each amender's rendered copy, onto `base`, the
/// provider's rendered file. The amenders are ordered by identity, whatever
/// order `copies` lists them in.
pub(crate) fn merge_json(
    base: &str,
    copies: &[(&UnitIdentity, &str)],
) -> Result<Merged, MergeFailure> {
    let base_value = parse_value(base).map_err(|error| MergeFailure::Unreadable {
        amender: None,
        error,
    })?;
    let mut sets: BTreeMap<UnitIdentity, Vec<Operation>> = BTreeMap::new();
    let mut notes = Vec::new();
    for (amender, copy) in copies {
        let copy_value = parse_value(copy).map_err(|error| MergeFailure::Unreadable {
            amender: Some((*amender).clone()),
            error,
        })?;
        let derived = derive(&base_value, &copy_value);
        for path in &derived.reordered {
            notes.push(MergeNote::NotCarried {
                amender: (*amender).clone(),
                what: format!(
                    "the order of the members of {}",
                    crate::domain::show_path(path)
                ),
            });
        }
        // The guard: the amender's operations alone, replayed on its base,
        // must give its copy's value, member order aside, which the
        // operations do not carry and is reported above.
        let own = BTreeMap::from([((*amender).clone(), derived.operations.clone())]);
        let replayed = combine(&own)
            .and_then(|combined| plan(&base_value, combined))
            .ok()
            .map(|plan| plan.result);
        if replayed.map(|value| sorted(&value)) != Some(sorted(&copy_value)) {
            notes.push(MergeNote::NotCarried {
                amender: (*amender).clone(),
                what: "its operations do not reproduce its copy".to_owned(),
            });
        }
        // Comments the copy adds are not values, and the merged file is the
        // provider's text: they are reported, never dropped in silence.
        let base_comments = comments_of(base);
        for comment in comments_of(copy) {
            if !base_comments.contains(&comment) {
                let shown: String = comment.chars().take(60).collect();
                notes.push(MergeNote::NotCarried {
                    amender: (*amender).clone(),
                    what: format!("the comment {shown:?}"),
                });
            }
        }
        sets.insert((*amender).clone(), derived.operations);
    }
    let combined = combine(&sets).map_err(MergeFailure::Conflicts)?;
    let planned = plan(&base_value, combined).map_err(MergeFailure::Conflicts)?;
    let text = apply(base, &planned.edits).map_err(MergeFailure::Edit)?;
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

/// Something the composition could not carry from a copy, reported rather
/// than dropped in silence.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MergeNote {
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
    /// The editor could not locate an edit in the provider's file.
    Edit(EditError),
    /// The edited text does not hold the planned value.
    Projection { detail: String },
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
            Self::Edit(error) => write!(f, "the merged file could not be written: {error}"),
            Self::Projection { detail } => write!(
                f,
                "the merged file was refused by the projection check: {detail}"
            ),
        }
    }
}

impl std::error::Error for MergeFailure {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]

    use super::{MergeFailure, MergeNote, merge_json};
    use crate::domain::UnitIdentity;

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
}
