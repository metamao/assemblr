//! Which versions of Assemblr read a unit (see
//! `docs/domain/compatibility.md` and `docs/adr/0041-format-compatibility.md`).
//!
//! A unit's `manifest.jsonc`, or a catalog's `catalog.jsonc`, declares in
//! `assemblr` the oldest version of Assemblr that reads it. `unit build`
//! writes the value, `unit publish` refuses a commit whose value is not the
//! one a build writes, and every reader refuses a unit that needs a newer
//! Assemblr than the one running, so that an older Assemblr skips such a
//! version and says why instead of misreading it.

use super::jsonc_edit::{apply, parse_value};
use crate::domain::{Edit, Value, Version};

/// The member of `manifest.jsonc` and `catalog.jsonc` that holds the oldest
/// version of Assemblr that reads the unit.
pub(crate) const FIELD: &str = "assemblr";

/// The oldest version of Assemblr that reads every form a unit can use
/// today: the first that reads [`FIELD`]. When a later version adds a form
/// that an older one cannot read, [`required`] raises the value for the
/// units that use it, and only for them.
const BASELINE: &str = "0.1.0-alpha.1";

/// The oldest version of Assemblr that reads what a unit uses, which a
/// build writes in [`FIELD`]. Every form a unit can use is read since
/// [`BASELINE`], so it is that version for every unit until a form an older
/// version cannot read exists.
pub(crate) fn required() -> Version {
    parse_constant(BASELINE)
}

/// The version of the Assemblr running.
pub(crate) fn running() -> Version {
    parse_constant(env!("CARGO_PKG_VERSION"))
}

/// `text`, a `manifest.jsonc` or `catalog.jsonc`, with [`FIELD`] set to
/// `required`: `None` when it already is, and otherwise the edited text,
/// which keeps every other byte, comments and formatting included. A new
/// member goes after `version`, or after `name` when there is no `version`.
/// An error says why the text could not be edited precisely.
pub(crate) fn declare(text: &str, required: &Version) -> Result<Option<String>, String> {
    let root = parse_value(text).map_err(|error| error.to_string())?;
    let Value::Object(members) = &root else {
        return Err("its top level is not an object".to_owned());
    };
    let value = Value::String(required.to_string());
    let edit = match members.iter().find(|(key, _)| key == FIELD) {
        Some((_, current)) if *current == value => return Ok(None),
        Some(_) => Edit::Set {
            path: vec![FIELD.to_owned()],
            value: value.clone(),
        },
        None => Edit::Insert {
            parent: Vec::new(),
            key: FIELD.to_owned(),
            value: value.clone(),
            after: ["version", "name"]
                .into_iter()
                .find(|key| members.iter().any(|(name, _)| name == key))
                .map(ToOwned::to_owned),
        },
    };
    let edited = apply(text, &[edit]).map_err(|error| error.to_string())?;
    match parse_value(&edited) {
        Ok(Value::Object(members))
            if members
                .iter()
                .any(|(key, current)| key == FIELD && *current == value) =>
        {
            Ok(Some(edited))
        }
        _ => Err("the edited text does not hold the new value".to_owned()),
    }
}

/// Why `subject`, a file Assemblr writes, is not read: it has the format
/// number `found`, and this version reads up to `newest`. A higher number
/// is one a newer Assemblr wrote, so the user is told to upgrade rather
/// than led to think the file is damaged.
pub(crate) fn unknown_format(subject: &str, found: u64, newest: u64) -> String {
    if found > newest {
        format!(
            "{subject} has format {found}, which a newer Assemblr wrote; this version reads up \
             to format {newest}: upgrade Assemblr to read it"
        )
    } else {
        format!("{subject} has format {found}, which no version of Assemblr writes")
    }
}

#[allow(
    clippy::expect_used,
    reason = "Cargo requires the package version to be SemVer, and BASELINE is a test-checked \
              constant"
)]
fn parse_constant(text: &str) -> Version {
    Version::parse(text).expect("a SemVer version known when Assemblr is compiled")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]

    use super::*;

    #[test]
    fn declaring_adds_the_field_after_the_version_and_keeps_the_rest() {
        let required = Version::parse("1.2.0").unwrap();
        let text = "{\n  // The unit.\n  \"name\": \"@acme/web\",\n  \"version\": \"1.0.0\",\n  \
                    \"description\": \"Web\"\n}\n";
        let edited = declare(text, &required).unwrap().unwrap();
        assert_eq!(
            edited,
            "{\n  // The unit.\n  \"name\": \"@acme/web\",\n  \"version\": \"1.0.0\",\n  \
             \"assemblr\": \"1.2.0\",\n  \"description\": \"Web\"\n}\n"
        );
        // Declared already: nothing to write.
        assert_eq!(declare(&edited, &required).unwrap(), None);
        // Another value is replaced in place.
        let newer = Version::parse("1.3.0").unwrap();
        let replaced = declare(&edited, &newer).unwrap().unwrap();
        assert_eq!(replaced, edited.replace("1.2.0", "1.3.0"));
    }

    #[test]
    fn declaring_in_a_catalog_without_a_version_goes_after_the_name() {
        let required = Version::parse("1.2.0").unwrap();
        let edited = declare("{ \"name\": \"@acme/web\" }", &required)
            .unwrap()
            .unwrap();
        assert!(
            edited.contains("\"name\": \"@acme/web\", \"assemblr\": \"1.2.0\""),
            "{edited}"
        );
        assert!(declare("[]", &required).is_err());
        assert!(declare("{", &required).is_err());
    }

    #[test]
    fn a_newer_format_says_to_upgrade_and_an_unknown_older_one_does_not() {
        let newer = unknown_format("it", 3, 2);
        assert!(
            newer.starts_with("it has format 3, which a newer Assemblr wrote"),
            "{newer}"
        );
        assert!(newer.ends_with("upgrade Assemblr to read it"), "{newer}");
        assert_eq!(
            unknown_format("`project`", 0, 1),
            "`project` has format 0, which no version of Assemblr writes"
        );
    }

    #[test]
    fn the_running_version_reads_what_a_build_requires() {
        // A build must never require a newer Assemblr than the one writing
        // the value, or the unit it builds would not read back.
        assert_ne!(
            required().cmp_precedence(&running()),
            std::cmp::Ordering::Greater
        );
    }
}
