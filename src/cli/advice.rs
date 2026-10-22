//! Advice for the problems a composition reports, shared by the commands
//! that compose: what the person can do about each kind of problem, since a
//! single sentence for all of them sent people to `options` for problems
//! that `options` cannot fix.

use assemblr::adapters::MergeFailure;
use assemblr::domain::{CompositionError, Requirer, RootTargetReason};

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
    let options = match audience {
        Audience::Consumer => "assemblr.jsonc `options`",
        Audience::Author => "the manifest's `options`",
    };
    Some(match error {
        // A defect in Assemblr, not something to act on.
        CompositionError::ContentMismatch { .. } => return None,
        CompositionError::RootTarget { reason, .. } => match reason {
            RootTargetReason::NotComposed | RootTargetReason::Transitive => format!(
                "remove the entry from {options}, or declare the unit in `dependencies` so that \
                 it is selected"
            ),
            RootTargetReason::NotTheBuiltUnit => {
                format!("remove the entry from {options}; a unit's build configures only itself")
            }
        },
        CompositionError::UndeclaredParameter { .. } => format!(
            "spell the parameter as the unit declares it in {options}, or remove the entry; `assemblr \
             info <unit>` lists the parameters the unit declares"
        ),
        CompositionError::InvalidValue { .. } => format!(
            "give the option in {options} a value of the parameter's type and, for an \
             enumerated parameter, one of its allowed values"
        ),
        CompositionError::MappingIncompatible { .. } => {
            "the unit's author must change the mapping or the parameters; a mapping never \
             converts a value"
                .to_owned()
        }
        CompositionError::OptionsConflict { requirements, .. } => {
            let mapped = requirements.iter().find_map(|requirement| {
                requirement
                    .from()
                    .map(|from| (requirement.requirer(), from))
            });
            match mapped {
                Some((requirer, from)) => format!(
                    "make the requirements agree: {requirer} carries its `{from}` to this \
                     parameter, so set `{from}` of that unit in {options} rather than this \
                     parameter directly, or give both the same value"
                ),
                None => format!(
                    "make the requirements agree: change the value in {options}, or select \
                     versions of the units whose requirements match"
                ),
            }
        }
        CompositionError::MissingValue { .. } => {
            format!("supply the value in {options}, under the unit's identity")
        }
        CompositionError::SeveralAmenders { .. } => {
            "remove one of the amending units, or ask an author to deliver the change in a file \
             of its own"
                .to_owned()
        }
        CompositionError::AmendmentBase { unit, .. } => format!(
            "`assemblr unit update` moves the locked version of {unit} and the base of the \
             amendment together; a build never changes either"
        ),
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
        CompositionError::InvalidDeliveredPath { .. } => {
            "the unit's author must rename the template so that its path without \
             `.assemblr.tmpl` is a valid path"
                .to_owned()
        }
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
pub(super) fn merge_advice(failure: Option<&MergeFailure>, audience: Audience) -> String {
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
            "the amendment and the provider both changed those lines: select the provider at the \
             amendment's base, or a version of the amender whose author amended the file again at \
             the version resolved"
                .to_owned()
        }
        Some(MergeFailure::Conflicts(_)) => format!(
            "the amendments of one file must agree: {who} so that no two amenders set one \
             value differently, or expose the value as a parameter of the provider"
        ),
        Some(MergeFailure::TextConflicts(_)) => format!(
            "no amender takes precedence over another: {who} so that they change different \
             lines that are not next to each other, or move an amender's text to a file of \
             its own"
        ),
    }
}

/// What to do when an amendment cannot be applied to the provider version a
/// composition resolved, which differs from the one the copy was taken from.
pub(super) fn cross_version_advice(
    unit: &assemblr::domain::UnitIdentity,
    amender: &assemblr::domain::UnitIdentity,
    base: &assemblr::domain::Version,
    reason: &assemblr::adapters::CrossVersionReason,
) -> String {
    use assemblr::adapters::CrossVersionReason;
    if matches!(reason, CrossVersionReason::BaseUnavailable) {
        return format!(
            "the registered repository of {unit} must still hold {base}: run `assemblr registry \
             sync`, or register it again"
        );
    }
    format!(
        "select {unit} at the amendment's base (`{unit}@={base}`), or a version of {amender} \
         whose amendment was taken from the version resolved, which its author publishes after \
         amending the file there"
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
    fn an_own_amendment_base_points_at_unit_update_and_not_at_options() {
        let error = CompositionError::AmendmentBase {
            amender: Requirer::Root,
            path: "config/app.json".parse().unwrap(),
            unit: "@acme/node".parse().unwrap(),
            base: "1.0.0".parse().unwrap(),
            resolved: "1.1.0".parse().unwrap(),
        };
        let advice = composition_advice(&error, Audience::Author).unwrap();
        assert!(advice.contains("unit update"), "{advice}");
        assert!(!advice.contains("options"), "{advice}");
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
