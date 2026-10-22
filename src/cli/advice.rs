//! Advice for the problems a composition reports, shared by the commands
//! that compose: what the person can do about each kind of problem, since a
//! single sentence for all of them sent people to `options` for problems
//! that `options` cannot fix.

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
            "spell the parameter as the unit declares it in {options}, or remove the entry; the \
             message lists the parameters the unit declares"
        ),
        CompositionError::InvalidValue { .. } => format!(
            "give the option in {options} a value of the parameter's type and, for an \
             enumerated parameter, one of its allowed values"
        ),
        CompositionError::OptionsConflict { .. } => format!(
            "make the requirements agree: change the value in {options}, or select versions of \
             the units whose requirements match"
        ),
        CompositionError::MissingValue { .. } => {
            format!("supply the value in {options}, under the unit's identity")
        }
        CompositionError::SeveralAmenders { .. } => {
            "remove one of the amending units, or ask an author to deliver the change in a file \
             of its own"
                .to_owned()
        }
        CompositionError::AmendmentBase {
            amender,
            unit,
            base,
            resolved,
            ..
        } => match amender {
            Requirer::Root => format!(
                "`assemblr unit update` moves the locked version of {unit} and the base of the \
                 amendment together; a build never changes either"
            ),
            Requirer::Unit(identity, _) => format!(
                "an amendment is applied only to {unit} {base}: select {unit} at that version \
                 (`{unit}@={base}`), or a version of {identity} whose amendment is based on \
                 {resolved}, which its author publishes after `assemblr unit update`"
            ),
        },
        CompositionError::AmendmentNotProvided { amender, .. }
        | CompositionError::AmendmentMissingCopy { amender, .. }
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
    fn an_amendment_base_names_the_versions_it_needs_and_options_are_not_mentioned() {
        let error = CompositionError::AmendmentBase {
            amender: Requirer::Unit("@acme/web".parse().unwrap(), "1.0.0".parse().unwrap()),
            path: "config/app.json".parse().unwrap(),
            unit: "@acme/node".parse().unwrap(),
            base: "1.0.0".parse().unwrap(),
            resolved: "1.1.0".parse().unwrap(),
        };
        let advice = composition_advice(&error, Audience::Consumer).unwrap();
        assert!(advice.contains("@acme/node@=1.0.0"), "{advice}");
        assert!(advice.contains("based on 1.1.0"), "{advice}");
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
