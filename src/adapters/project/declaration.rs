//! Editing the consumer's declaration, `assemblr.jsonc`, in place for
//! `add` and `remove`.
//!
//! The text is edited with the JSONC editor of the merge, so the file's
//! comments, indentation, and line endings stay as its author wrote them.
//! Every edit is checked by reading the edited text back: it must hold
//! exactly the value the edit meant, so a layout the editor misjudges is a
//! refusal, never a different declaration.

use crate::adapters::jsonc_edit::{apply, parse_value};
use crate::domain::{Edit, UnitIdentity, Value};

const DEPENDENCIES: &str = "dependencies";
const OPTIONS: &str = "options";

/// What an edit did to the declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclarationChange {
    /// The unit was selected with this requirement.
    Added { requirement: String },
    /// The unit was selected already; its requirement was replaced.
    Replaced { from: String, to: String },
    /// The unit was selected already with this requirement, which stays.
    Kept { requirement: String },
    /// The unit's selection was removed, together with its `options` entry
    /// when it had one.
    Removed { options: bool },
}

/// Why the declaration was not edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EditFailure {
    /// The unit is not an explicit selection.
    NotSelected,
    /// The text could not be edited safely.
    Cannot(String),
}

/// The requirement `text` selects `identity` with, if it does.
pub(super) fn selected_requirement(text: &str, identity: &UnitIdentity) -> Option<String> {
    let root = parse_value(text).ok()?;
    let key = identity.to_string();
    match root.member(DEPENDENCIES) {
        Some(Value::Object(selected)) => selected.iter().find_map(|(name, value)| match value {
            Value::String(requirement) if *name == key => Some(requirement.clone()),
            _ => None,
        }),
        _ => None,
    }
}

/// `text` with `identity` selected at `requirement`: added at the end of
/// `dependencies`, or its requirement replaced when it is selected already.
pub(super) fn add(
    text: &str,
    identity: &UnitIdentity,
    requirement: &str,
) -> Result<(String, DeclarationChange), EditFailure> {
    let root = parse_value(text).map_err(|error| EditFailure::Cannot(error.to_string()))?;
    let Value::Object(members) = &root else {
        return Err(EditFailure::Cannot("it is not a JSON object".to_owned()));
    };
    let key = identity.to_string();
    let value = Value::String(requirement.to_owned());
    let (edit, change) = match root.member(DEPENDENCIES) {
        Some(Value::Object(selected)) => match selected.iter().find(|(name, _)| *name == key) {
            Some((_, Value::String(current))) if current == requirement => {
                return Ok((
                    text.to_owned(),
                    DeclarationChange::Kept {
                        requirement: requirement.to_owned(),
                    },
                ));
            }
            Some((_, Value::String(current))) => (
                Edit::Set {
                    path: vec![DEPENDENCIES.to_owned(), key.clone()],
                    value: value.clone(),
                },
                DeclarationChange::Replaced {
                    from: current.clone(),
                    to: requirement.to_owned(),
                },
            ),
            Some(_) => {
                return Err(EditFailure::Cannot(format!(
                    "the requirement of {identity} is not a string"
                )));
            }
            None => (
                Edit::Insert {
                    parent: vec![DEPENDENCIES.to_owned()],
                    key: key.clone(),
                    value: value.clone(),
                    after: selected.last().map(|(name, _)| name.clone()),
                },
                DeclarationChange::Added {
                    requirement: requirement.to_owned(),
                },
            ),
        },
        Some(_) => {
            return Err(EditFailure::Cannot(
                "`dependencies` is not an object".to_owned(),
            ));
        }
        None => (
            Edit::Insert {
                parent: Vec::new(),
                key: DEPENDENCIES.to_owned(),
                value: Value::Object(vec![(key.clone(), value.clone())]),
                after: members.last().map(|(name, _)| name.clone()),
            },
            DeclarationChange::Added {
                requirement: requirement.to_owned(),
            },
        ),
    };
    let mut expected = root.clone();
    set_selection(&mut expected, &key, Some(value));
    let edited = apply_checked(text, &[edit], &expected)?;
    Ok((edited, change))
}

/// `text` without the selection of `identity` and without its entry in
/// `options`.
pub(super) fn remove(
    text: &str,
    identity: &UnitIdentity,
) -> Result<(String, DeclarationChange), EditFailure> {
    let root = parse_value(text).map_err(|error| EditFailure::Cannot(error.to_string()))?;
    let key = identity.to_string();
    let selected = matches!(
        root.member(DEPENDENCIES),
        Some(Value::Object(selected)) if selected.iter().any(|(name, _)| *name == key)
    );
    if !selected {
        return Err(EditFailure::NotSelected);
    }
    let mut edits = vec![Edit::Delete {
        path: vec![DEPENDENCIES.to_owned(), key.clone()],
    }];
    let options = matches!(
        root.member(OPTIONS),
        Some(Value::Object(entries)) if entries.iter().any(|(name, _)| *name == key)
    );
    if options {
        edits.push(Edit::Delete {
            path: vec![OPTIONS.to_owned(), key.clone()],
        });
    }
    let mut expected = root.clone();
    set_selection(&mut expected, &key, None);
    if options {
        if let Value::Object(members) = &mut expected {
            for (name, value) in members.iter_mut() {
                if name == OPTIONS {
                    if let Value::Object(entries) = value {
                        entries.retain(|(entry, _)| *entry != key);
                    }
                }
            }
        }
    }
    let edited = apply_checked(text, &edits, &expected)?;
    Ok((edited, DeclarationChange::Removed { options }))
}

/// Applies `edits` to `text` and requires the result to hold `expected`.
fn apply_checked(text: &str, edits: &[Edit], expected: &Value) -> Result<String, EditFailure> {
    let edited = apply(text, edits).map_err(|error| EditFailure::Cannot(error.to_string()))?;
    match parse_value(&edited) {
        Ok(value) if value == *expected => Ok(edited),
        Ok(value) => Err(EditFailure::Cannot(format!(
            "the edited declaration holds {}, not the intended {}",
            value.brief(),
            expected.brief()
        ))),
        Err(error) => Err(EditFailure::Cannot(error.to_string())),
    }
}

/// Sets, or with `None` removes, the member `key` of `dependencies`,
/// creating `dependencies` last when the declaration has none.
fn set_selection(root: &mut Value, key: &str, value: Option<Value>) {
    let Value::Object(members) = root else {
        return;
    };
    let position = members.iter().position(|(name, _)| name == DEPENDENCIES);
    match (position, value) {
        (Some(index), value) => {
            if let Value::Object(selected) = &mut members[index].1 {
                match (selected.iter().position(|(name, _)| name == key), value) {
                    (Some(at), Some(value)) => selected[at].1 = value,
                    (None, Some(value)) => selected.push((key.to_owned(), value)),
                    (Some(at), None) => {
                        selected.remove(at);
                    }
                    (None, None) => {}
                }
            }
        }
        (None, Some(value)) => members.push((
            DEPENDENCIES.to_owned(),
            Value::Object(vec![(key.to_owned(), value)]),
        )),
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]

    use super::{DeclarationChange, EditFailure, add, remove};
    use crate::domain::UnitIdentity;

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    const DECLARATION: &str = "{\n  // The units this project selects.\n  \"dependencies\": {\n    \
                               \"@acme/a\": \"^1.0.0\" // first\n  },\n  \"options\": {\n    \
                               \"@acme/a\": { \"mode\": \"x\" }\n  }\n}\n";

    #[test]
    fn a_selection_is_added_after_the_last_one_keeping_comments_and_layout() {
        let (text, change) = add(DECLARATION, &identity("@acme/b"), "^2.1.0").unwrap();
        assert_eq!(
            change,
            DeclarationChange::Added {
                requirement: "^2.1.0".to_owned()
            }
        );
        assert_eq!(
            text,
            "{\n  // The units this project selects.\n  \"dependencies\": {\n    \"@acme/a\": \
             \"^1.0.0\", // first\n    \"@acme/b\": \"^2.1.0\"\n  },\n  \"options\": {\n    \
             \"@acme/a\": { \"mode\": \"x\" }\n  }\n}\n"
        );
    }

    #[test]
    fn an_empty_dependencies_object_and_a_missing_one_both_take_the_selection() {
        let (text, _) = add(
            "{\n  \"dependencies\": {},\n  \"options\": {}\n}\n",
            &identity("@acme/a"),
            "^1.0.0",
        )
        .unwrap();
        assert!(text.contains("\"@acme/a\": \"^1.0.0\""), "{text}");
        let (text, _) = add("{\n  \"options\": {}\n}\n", &identity("@acme/a"), "^1.0.0").unwrap();
        assert!(text.contains("\"dependencies\": {"), "{text}");
        assert!(text.contains("\"@acme/a\": \"^1.0.0\""), "{text}");
    }

    #[test]
    fn a_selected_unit_keeps_an_equal_requirement_and_has_another_replaced() {
        let (text, change) = add(DECLARATION, &identity("@acme/a"), "^1.0.0").unwrap();
        assert_eq!(text, DECLARATION);
        assert_eq!(
            change,
            DeclarationChange::Kept {
                requirement: "^1.0.0".to_owned()
            }
        );
        let (text, change) = add(DECLARATION, &identity("@acme/a"), "~1.2").unwrap();
        assert!(text.contains("\"@acme/a\": \"~1.2\" // first"), "{text}");
        assert_eq!(
            change,
            DeclarationChange::Replaced {
                from: "^1.0.0".to_owned(),
                to: "~1.2".to_owned()
            }
        );
    }

    #[test]
    fn a_removal_takes_the_selection_and_its_options_and_nothing_else() {
        let (text, change) = remove(DECLARATION, &identity("@acme/a")).unwrap();
        assert_eq!(change, DeclarationChange::Removed { options: true });
        assert!(!text.contains("@acme/a"), "{text}");
        assert!(
            text.contains("// The units this project selects."),
            "{text}"
        );
        let value = crate::adapters::jsonc_edit::parse_value(&text).unwrap();
        assert_eq!(value.brief(), "{\"dependencies\":{},\"options\":{}}");
    }

    #[test]
    fn a_unit_that_is_not_selected_is_not_removed() {
        assert_eq!(
            remove(DECLARATION, &identity("@acme/b")).unwrap_err(),
            EditFailure::NotSelected
        );
    }

    #[test]
    fn a_declaration_that_is_not_an_object_is_refused() {
        assert!(matches!(
            add("[]", &identity("@acme/a"), "^1.0.0"),
            Err(EditFailure::Cannot(_))
        ));
    }
}
