//! `assemblr info`, run as the real binary against registries built in
//! temporary directories. Every test gives the binary its own registry and
//! home.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a fixture that cannot be built fails the test"
)]

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Output;

use serde_json::Value;

use common::{Fixture, fails, git, stderr, stdout, succeeds};

impl Fixture {
    fn repo(&self, name: &str) -> Repo {
        git(&self.dir, &["init", "-q", name]);
        Repo {
            path: self.dir.join(name),
        }
    }

    fn assemblr(&self, args: &[&str]) -> Output {
        self.command(&self.dir, args).output().unwrap()
    }

    fn register(&self, repo: &Repo) {
        succeeds(self.assemblr(&["registry", "add", repo.path.to_str().unwrap()]));
    }
}

struct Repo {
    path: PathBuf,
}

impl Repo {
    fn git(&self, args: &[&str]) -> String {
        git(&self.path, args)
    }

    fn write(&self, path: &str, contents: &str) {
        let path = self.path.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn publish(&self, version: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", version]);
        self.git(&["tag", &format!("v{version}")]);
    }
}

/// `@acme/node` 1.0.0 and 1.1.0-beta.1 with a placeholder, and `@acme/web`
/// 1.0.0 with a dependency, placeholders of its own, a token that is not
/// one, and an amendment of node's file.
fn units(fixture: &Fixture) -> (Repo, Repo) {
    let node = fixture.repo("node");
    let node_manifest = |version: &str| {
        format!(
            r#"{{"name": "@acme/node", "version": "{version}", "description": "The Node runtime.",
                "placeholders": {{"replace": {{"__PORT__": "The port the server listens on."}}}}}}"#
        )
    };
    node.write("manifest.jsonc", &node_manifest("1.0.0"));
    node.write("src/config/app.json", "{\n  \"port\": \"__PORT__\"\n}\n");
    node.publish("1.0.0");
    node.write("manifest.jsonc", &node_manifest("1.1.0-beta.1"));
    node.publish("1.1.0-beta.1");
    fixture.register(&node);
    let web = fixture.repo("web");
    web.write(
        "manifest.jsonc",
        r#"{
  "name": "@acme/web",
  "version": "1.0.0",
  "description": "A web front end.",
  "dependencies": {"@acme/node": "=1.0.0"},
  "placeholders": {
    "replace": {"__TITLE__": "The page title.", "__API_URL__": "The API's base URL."},
    "ignore": ["__DEV__"]
  },
  "amendments": {"config/app.json": {"unit": "@acme/node", "base": "1.0.0"}}
}"#,
    );
    web.write(
        "amendments/config/app.json",
        "{\n  \"port\": \"__PORT__\",\n  \"api\": \"__API_URL__\"\n}\n",
    );
    web.write(
        "assemblr-lock.json",
        &format!(
            "{{\n  \"format\": 1,\n  \"units\": {{\n    \"@acme/node\": {{\n      \"commit\": \
             \"{}\",\n      \"tree\": \"{}\",\n      \"version\": \"1.0.0\"\n    }}\n  }}\n}}\n",
            node.git(&["rev-parse", "v1.0.0"]),
            node.git(&["rev-parse", "v1.0.0^{tree}"])
        ),
    );
    web.write(
        "src/index.html",
        "<title>__TITLE__</title>\n<script>__DEV__</script>\n",
    );
    web.publish("1.0.0");
    fixture.register(&web);
    (node, web)
}

#[test]
fn info_describes_what_a_consumer_needs_to_configure_a_unit() {
    let fixture = Fixture::new("describe");
    let (_, web) = units(&fixture);
    let out = stdout(&succeeds(fixture.assemblr(&["info", "@acme/web"])));
    for expected in [
        "@acme/web 1.0.0",
        "description: A web front end.",
        "published versions: 1.0.0",
        "@acme/node =1.0.0",
        "  placeholders:\n    __API_URL__: The API's base URL.\n    __TITLE__: The page title.\n",
        "not placeholders: __DEV__",
        "config/app.json of @acme/node 1.0.0",
    ] {
        assert!(out.contains(expected), "{expected:?} in {out}");
    }
    assert!(
        out.contains(&format!("source: {}", web.path.display())),
        "{out}"
    );
    // A dependency's placeholders are its own: `info @acme/node` lists them.
    assert!(!out.contains("__PORT__"), "{out}");
}

#[test]
fn a_unit_without_placeholders_or_dependencies_says_none() {
    let fixture = Fixture::new("none");
    let bare = fixture.repo("bare");
    bare.write(
        "manifest.jsonc",
        r#"{"name": "@acme/bare", "version": "0.1.0", "description": "Nothing."}"#,
    );
    bare.publish("0.1.0");
    fixture.register(&bare);
    let out = stdout(&succeeds(fixture.assemblr(&["info", "@acme/bare"])));
    for section in ["dependencies", "placeholders", "amendments"] {
        assert!(out.contains(&format!("{section}: none")), "{out}");
    }
    assert!(!out.contains("not placeholders"), "{out}");
}

#[test]
fn the_newest_stable_version_is_the_default_and_a_version_can_be_named() {
    let fixture = Fixture::new("versions");
    units(&fixture);
    let out = stdout(&succeeds(fixture.assemblr(&["info", "@acme/node"])));
    assert!(out.starts_with("@acme/node 1.0.0\n"), "{out}");
    assert!(
        out.contains("published versions: 1.0.0, 1.1.0-beta.1"),
        "{out}"
    );
    let out = stdout(&succeeds(
        fixture.assemblr(&["info", "@acme/node@1.1.0-beta.1"]),
    ));
    assert!(out.starts_with("@acme/node 1.1.0-beta.1\n"), "{out}");
}

#[test]
fn json_carries_the_same_facts_in_a_stable_shape() {
    let fixture = Fixture::new("json");
    let (_, web) = units(&fixture);
    let output = succeeds(fixture.assemblr(&["info", "@acme/web", "--json"]));
    let json: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(json["identity"], "@acme/web");
    assert_eq!(json["version"], "1.0.0");
    assert_eq!(json["description"], "A web front end.");
    assert_eq!(json["versions"], serde_json::json!(["1.0.0"]));
    assert_eq!(json["dependencies"]["@acme/node"], "=1.0.0");
    assert_eq!(
        json["placeholders"],
        serde_json::json!({
            "replace": {"__API_URL__": "The API's base URL.", "__TITLE__": "The page title."},
            "ignore": ["__DEV__"]
        })
    );
    assert_eq!(json.get("params"), None);
    assert_eq!(json.get("options"), None);
    assert_eq!(
        json["amendments"]["config/app.json"],
        serde_json::json!({"unit": "@acme/node", "base": "1.0.0"})
    );
    assert_eq!(json["commit"], web.git(&["rev-parse", "v1.0.0"]));
    assert_eq!(json["tree"], web.git(&["rev-parse", "v1.0.0^{tree}"]));
    // The output depends on nothing but the registry and the repository.
    let again = succeeds(fixture.assemblr(&["info", "@acme/web", "--json"]));
    assert_eq!(stdout(&again), stdout(&output));
}

#[test]
fn failures_say_what_is_wrong_and_what_to_do() {
    let fixture = Fixture::new("failures");
    let (node, _) = units(&fixture);

    let err = stderr(&fails(fixture.assemblr(&["info", "@acme/missing"])));
    assert!(
        err.contains("no registered source provides @acme/missing"),
        "{err}"
    );
    assert!(err.contains("hint: `assemblr registry list`"), "{err}");

    let err = stderr(&fails(fixture.assemblr(&["info", "@acme/node@9.9.9"])));
    assert!(
        err.contains("@acme/node has no indexed version 9.9.9; it has 1.0.0, 1.1.0-beta.1"),
        "{err}"
    );
    assert!(err.contains("registry sync"), "{err}");

    // A malformed argument is a usage error, reported before anything is
    // read.
    let output = fixture.assemblr(&["info", "@acme/node@^1"]);
    assert_eq!(output.status.code(), Some(2));
    let err = stderr(&output);
    assert!(err.contains("invalid version"), "{err}");
    assert!(err.contains("exact SemVer version"), "{err}");

    let output = fixture.assemblr(&["info", "node"]);
    assert_eq!(output.status.code(), Some(2));
    let err = stderr(&output);
    assert!(err.contains("invalid unit \"node\""), "{err}");
    assert!(err.contains("like `@acme/web`"), "{err}");

    // A registered repository that moved away.
    let moved = fixture.dir.join("node-moved");
    fs::rename(&node.path, &moved).unwrap();
    let err = stderr(&fails(fixture.assemblr(&["info", "@acme/node"])));
    assert!(
        err.contains("the repository of @acme/node cannot be read"),
        "{err}"
    );
    assert!(err.contains("registry remove @acme/node"), "{err}");
}

#[test]
fn a_registered_unit_with_no_published_version_cannot_be_described() {
    let fixture = Fixture::new("unpublished");
    let draft = fixture.repo("draft");
    draft.write(
        "manifest.jsonc",
        r#"{"name": "@acme/draft", "version": "0.1.0", "description": "Not yet."}"#,
    );
    draft.git(&["add", "-A"]);
    draft.git(&["commit", "-q", "-m", "draft"]);
    fixture.register(&draft);
    let err = stderr(&fails(fixture.assemblr(&["info", "@acme/draft"])));
    assert!(
        err.contains("has no published version to describe"),
        "{err}"
    );
    assert!(err.contains("assemblr unit publish"), "{err}");
}
