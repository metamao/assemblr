//! Advice for the problems a composition reports, shared by the commands
//! that compose: what the person can do about each kind of problem, since
//! each needs its own way out.

use assemblr::adapters::{CrossVersionReason, MergeFailure};
use assemblr::domain::{CompositionError, Requirer, UnitIdentity, Version};

/// Who reads the advice, since the same problem is fixed in different
/// files: a consumer edits `assemblr.jsonc`, and an author edits the
/// manifest of the unit being built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Audience {
    Consumer,
    Author,
}

/// What to do about `error`, or `None` when the message already says it or
/// there is nothing the reader can do.
pub(super) fn composition_advice(error: &CompositionError, audience: Audience) -> Option<String> {
    Some(match error {
        // A defect in Assemblr, not something to act on.
        CompositionError::ContentMismatch { .. } => return None,
        CompositionError::SeveralAmenders { .. } => {
            "remove one of the amending units, or ask an author to deliver the change in a file \
             of its own"
                .to_owned()
        }
        CompositionError::AmendmentBase { unit, .. } => format!(
            "`assemblr unit update` moves the locked version of {unit} and the base of the \
             amendment together; a build never changes either"
        ),
        CompositionError::AmendmentUnreachable { amender, unit, .. } => match amender {
            Requirer::Root => format!(
                "declare a dependency that leads to {unit}, or remove the declaration and the \
                 copy"
            ),
            Requirer::Unit(identity, _) => format!(
                "select a version of {identity} whose dependencies lead to {unit}, or a version \
                 that does not amend the file"
            ),
        },
        CompositionError::AmendmentNotProvided {
            amender,
            unit,
            version,
            ..
        } => match amender {
            Requirer::Root => return None,
            Requirer::Unit(identity, _) => format!(
                "{unit} {version} does not provide the file: select an earlier version of {unit} \
                 that provides it, or a version of {identity} taken from a version without it"
            ),
        },
        CompositionError::AmendmentMissingCopy { amender, .. }
        | CompositionError::AmendmentUndeclared { amender, .. } => match amender {
            Requirer::Root => return None,
            Requirer::Unit(identity, _) => format!(
                "the published version of {identity} is inconsistent and never changes: select \
                 another version of it, or ask its author to publish a corrected one"
            ),
        },
        CompositionError::PathCollision { .. } | CompositionError::NestedPathCollision { .. } => {
            let (drop, from) = match audience {
                Audience::Consumer => ("one of the units", "the selection"),
                Audience::Author => ("one of the dependencies", "the manifest"),
            };
            format!(
                "remove {drop} from {from}, or have the units' authors move one of the files, \
                 or have one unit amend the other's file instead of delivering it"
            )
        }
    })
}

/// What to do when the amendments of one file could not be combined.
fn merge_advice(failure: Option<&MergeFailure>, audience: Audience) -> String {
    let who = match audience {
        Audience::Consumer => "the amending units' authors must change the copies",
        Audience::Author => "change the copies",
    };
    match failure {
        Some(MergeFailure::Unreadable { .. }) => {
            "every side of a JSON merge must be a JSON document: fix the copy, or the \
             provider's file, so that it reads as JSON"
                .to_owned()
        }
        None => "every side of a merge must be UTF-8 text".to_owned(),
        Some(MergeFailure::Edit(_) | MergeFailure::Projection { .. }) => {
            "the provider's file has a layout the merge could not edit safely; nothing was \
             written, and reformatting that file conventionally works around it"
                .to_owned()
        }
        Some(MergeFailure::RoundTrip { .. }) => {
            "this is a defect in Assemblr's merge, not in the copy; nothing was written, and the \
             copy and the provider's file at its base are what a report needs"
                .to_owned()
        }
        Some(MergeFailure::Rebase { .. }) => {
            "the amendment and the provider both changed those lines".to_owned()
        }
        Some(MergeFailure::Conflicts(_)) => format!(
            "the amendments of one file must agree: {who} so that no two amenders set one \
             value differently, or have the provider leave the value to the consumer as a \
             placeholder"
        ),
        Some(MergeFailure::TextConflicts(_)) => format!(
            "no amender takes precedence over another: {who} so that they change different \
             lines that are not next to each other, or move an amender's text to a file of \
             its own"
        ),
    }
}

/// What to do when the amendments of one file could not be combined, with
/// the way back to the provider's version when an amendment was taken from
/// another one.
pub(super) fn merge_problem_advice(
    unit: &UnitIdentity,
    failure: Option<&MergeFailure>,
    crossed: &[(UnitIdentity, Version)],
    audience: Audience,
) -> String {
    let mut advice = merge_advice(failure, audience);
    // A rebase collision names the amender it concerns.
    let crossing = match failure {
        Some(MergeFailure::Rebase { amender, .. }) => crossed
            .iter()
            .find(|(crossing, _)| crossing == amender)
            .or(crossed.first()),
        _ => crossed.first(),
    };
    if let Some((amender, base)) = crossing {
        advice.push_str("; ");
        advice.push_str(&cross_version_advice(
            unit,
            amender,
            base,
            &CrossVersionReason::NoMerge,
            audience,
        ));
    }
    advice
}

/// What to do when an amendment cannot be applied to the provider version a
/// composition resolved, which differs from the one the copy was taken from.
/// A consumer pins the provider with `add`, which also makes it an explicit
/// selection that `update` no longer moves, so the advice says how to undo
/// it; an author pins it in the manifest.
pub(super) fn cross_version_advice(
    unit: &UnitIdentity,
    amender: &UnitIdentity,
    base: &Version,
    reason: &CrossVersionReason,
    audience: Audience,
) -> String {
    if matches!(reason, CrossVersionReason::BaseUnavailable) {
        return format!(
            "the registered repository of {unit} must still hold {base}: run `assemblr registry \
             sync`, or register it again"
        );
    }
    let pin = match audience {
        Audience::Consumer => format!(
            "pin {unit} to the amendment's base with `assemblr add {unit}@={base}`, which \
             `assemblr update` then leaves where it is (`assemblr add {unit}@^{base}` lets it move \
             again, and `assemblr remove {unit}` undoes a selection made only to pin it)"
        ),
        Audience::Author => format!(
            "pin {unit} to the amendment's base with `\"{unit}\": \"={base}\"` in the \
             manifest's `dependencies`"
        ),
    };
    format!(
        "{pin}, or select a version of {amender} whose amendment was taken from the version \
         resolved, which its author publishes after amending the file there"
    )
}

/// The advice of every problem, once each and in the order of the problems,
/// since problems of different kinds are reported together and each needs
/// its own way out.
pub(super) fn distinct(advice: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = Vec::new();
    for advice in advice {
        if !seen.contains(&advice) {
            seen.push(advice);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::{Audience, composition_advice};
    use assemblr::domain::{CompositionError, Requirer};

    #[test]
    fn an_own_amendment_base_points_at_unit_update() {
        let error = CompositionError::AmendmentBase {
            amender: Requirer::Root,
            path: "config/app.json".parse().unwrap(),
            unit: "@acme/node".parse().unwrap(),
            base: "1.0.0".parse().unwrap(),
            resolved: "1.1.0".parse().unwrap(),
        };
        let advice = composition_advice(&error, Audience::Author).unwrap();
        assert!(advice.contains("unit update"), "{advice}");
    }

    #[test]
    fn an_internal_mismatch_has_no_advice() {
        let error = CompositionError::ContentMismatch {
            unit: "@acme/node".parse().unwrap(),
            version: "1.0.0".parse().unwrap(),
        };
        assert_eq!(composition_advice(&error, Audience::Author), None);
    }
}
