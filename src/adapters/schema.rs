//! The JSON Schema for `manifest.jsonc` that `unit init` writes beside the
//! manifest for editor completion and diagnostics (see
//! `docs/domain/manifest.md` and `docs/adr/0014-unit-init.md`).
//!
//! The schema is a fixed document embedded in the binary, so every unit
//! initialized by one version of Assemblr gets the same bytes. It describes
//! the manifest vocabulary and the provisional parameter-definition rules
//! with the keywords of JSON Schema draft 7, which editors support widely.
//! Assemblr's own validation stays authoritative: the schema expresses what
//! it can, and nothing the schema accepts is thereby valid.

/// The schema's file name, at the unit repository's top level.
pub const MANIFEST_SCHEMA: &str = "manifest.schema.json";

/// The value of `$schema` in a manifest that refers to the file beside it.
pub const MANIFEST_SCHEMA_REFERENCE: &str = "./manifest.schema.json";

/// The schema document, byte for byte as `unit init` writes it.
pub fn manifest_schema() -> &'static str {
    include_str!("manifest.schema.json")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "the schema is a fixed document")]

    use regex_lite::Regex;
    use serde_json::Value;

    use super::manifest_schema;
    use crate::adapters::manifest::MANIFEST_FIELDS;
    use crate::domain::{ParameterName, UnitIdentity, Version};

    fn schema() -> Value {
        serde_json::from_str(manifest_schema()).unwrap()
    }

    /// The pattern of the named definition, compiled.
    fn pattern(definition: &str) -> Regex {
        let schema = schema();
        let pattern = schema["definitions"][definition]["pattern"]
            .as_str()
            .unwrap();
        Regex::new(pattern).unwrap()
    }

    #[test]
    fn the_schema_is_draft_7_and_names_every_manifest_field() {
        let schema = schema();
        assert_eq!(schema["$schema"], "http://json-schema.org/draft-07/schema#");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        let mut properties: Vec<&str> = schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        properties.sort_unstable();
        let mut fields = MANIFEST_FIELDS.to_vec();
        fields.sort_unstable();
        assert_eq!(properties, fields);
        assert_eq!(
            schema["required"],
            serde_json::json!(["name", "version", "description"])
        );
        assert!(manifest_schema().ends_with("}\n"));
    }

    #[test]
    fn the_parameter_definition_matches_the_provisional_rules() {
        let schema = schema();
        let parameter = &schema["definitions"]["parameter"];
        assert_eq!(parameter["additionalProperties"], false);
        assert_eq!(
            parameter["required"],
            serde_json::json!(["type", "description"])
        );
        let mut properties: Vec<&str> = parameter["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        properties.sort_unstable();
        assert_eq!(
            properties,
            [
                "allowed",
                "default",
                "description",
                "example",
                "pattern",
                "type"
            ]
        );
        assert_eq!(
            parameter["properties"]["type"]["enum"],
            serde_json::json!(["string", "integer", "boolean"])
        );
        assert_eq!(parameter["properties"]["allowed"]["minItems"], 1);
        assert_eq!(parameter["properties"]["allowed"]["uniqueItems"], true);
        // `allowed` and `pattern` are refused for any type but `string`.
        assert_eq!(
            parameter["allOf"][0]["else"],
            serde_json::json!({ "properties": { "allowed": false, "pattern": false } })
        );
        assert_eq!(
            schema["definitions"]["parameterName"]["not"],
            serde_json::json!({ "const": "own" })
        );
        // A parameter is defined or inherited, `{ "from": "<identity>" }`.
        assert_eq!(
            schema["properties"]["params"]["additionalProperties"]["oneOf"][1]["$ref"],
            "#/definitions/inheritedParameter"
        );
        let inherited = &schema["definitions"]["inheritedParameter"];
        assert_eq!(inherited["required"], serde_json::json!(["from"]));
        assert_eq!(inherited["additionalProperties"], false);
        assert_eq!(
            inherited["properties"]["from"]["$ref"],
            "#/definitions/identity"
        );
    }

    #[test]
    fn the_identity_pattern_agrees_with_the_parser() {
        let identity = pattern("identity");
        for input in [
            "@acme/node",
            "@metamao/cloudflare-angular-ssr",
            "@a1/b-2",
            "@acme/-node",
            "@acme/node-",
            "@acme/cloud--worker",
            "@Acme/node",
            "acme/node",
            "@acme",
            "@/node",
            "@acme/node/extra",
            "@acme/node ",
            "@ac me/node",
        ] {
            assert_eq!(
                identity.is_match(input),
                UnitIdentity::parse(input).is_ok(),
                "{input:?}"
            );
        }
    }

    #[test]
    fn the_version_pattern_agrees_with_the_parser() {
        let version = pattern("version");
        for input in [
            "0.1.0",
            "1.2.3",
            "10.20.30",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-0.3.7",
            "1.0.0-x.7.z.92",
            "1.0.0+build.1",
            "1.0.0-beta+exp.sha.5114f85",
            "1.0.0-rc.1+build",
            "1.0",
            "01.0.0",
            "1.0.0-",
            "1.0.0-01",
            "1.0.0+",
            "v1.0.0",
            "1.0.0 ",
            "^1.0.0",
        ] {
            assert_eq!(
                version.is_match(input),
                Version::parse(input).is_ok(),
                "{input:?}"
            );
        }
    }

    #[test]
    fn the_parameter_name_pattern_agrees_with_the_parser_except_for_own() {
        let name = pattern("parameterName");
        for input in [
            "appName",
            "node_compat",
            "a",
            "A1_",
            "1a",
            "_a",
            "app-name",
            "app name",
            "",
            "ñ",
        ] {
            assert_eq!(
                name.is_match(input),
                ParameterName::parse(input).is_ok(),
                "{input:?}"
            );
        }
        // The reserved name is excluded by `not`, which the pattern alone
        // cannot express.
        assert!(name.is_match("own"));
        assert!(ParameterName::parse("own").is_err());
    }

    #[test]
    fn descriptions_must_not_be_blank_and_paths_have_no_empty_segments() {
        let text = pattern("text");
        assert!(text.is_match("An app."));
        assert!(!text.is_match(""));
        assert!(!text.is_match("  \t"));
        let schema = schema();
        let paths = Regex::new(
            schema["properties"]["amendments"]["propertyNames"]["pattern"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert!(paths.is_match("package.json"));
        assert!(paths.is_match("config/wrangler.toml.assemblr.tmpl"));
        assert!(!paths.is_match("/config/a"));
        assert!(!paths.is_match("config//a"));
        assert!(!paths.is_match("config/"));
        assert!(!paths.is_match(""));
    }
}
