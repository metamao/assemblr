//! `assemblr registry create` and `assemblr catalog refresh`, run as the real binary. Units are repositories in the
//! test's directory, declared by relative path or reached through `file://`
//! URLs, so the tests are offline.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a fixture that cannot be built fails the test"
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::Value;

use common::{Fixture, fails, git, stderr, stdout, succeeds};

impl Fixture {
    fn assemblr(&self, dir: &Path, args: &[&str]) -> Output {
        self.command(dir, args).output().unwrap()
    }

    /// The catalog `@acme/web` in `web`, declaring `units`.
    fn catalog(&self, units: &str) -> PathBuf {
        succeeds(self.assemblr(&self.dir, &["registry", "create", "@acme/web"]));
        let catalog = self.dir.join("web");
        fs::write(
            catalog.join("registry.jsonc"),
            format!(r#"{{"name": "@acme/web", "units": {{{units}}}}}"#),
        )
        .unwrap();
        catalog
    }

    /// A repository named `name` with `versions` of `identity` published.
    fn unit(&self, name: &str, identity: &str, versions: &[&str]) -> PathBuf {
        git(&self.dir, &["init", "-q", name]);
        let path = self.dir.join(name);
        for version in versions {
            publish(&path, identity, version);
        }
        path
    }
}

fn publish(path: &Path, identity: &str, version: &str) {
    fs::write(
        path.join("manifest.jsonc"),
        format!(
            r#"{{
                // A comment the index does not keep.
                "name": "{identity}",
                "version": "{version}",
                "description": "{identity} for the web."
            }}"#
        ),
    )
    .unwrap();
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/file.txt"), format!("{version}\n")).unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-q", "-m", version]);
    git(path, &["tag", &format!("v{version}")]);
}

fn url(path: &Path) -> String {
    let path = path.to_str().unwrap().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

fn index(catalog: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(catalog.join("registry-index.json")).unwrap()).unwrap()
}

#[test]
fn create_makes_a_git_repository_with_an_empty_catalog() {
    let fixture = Fixture::new("init");
    let created = succeeds(fixture.assemblr(&fixture.dir, &["registry", "create", "@acme/web"]));
    assert!(
        stdout(&created).contains("Created the catalog @acme/web in web"),
        "{}",
        stdout(&created)
    );
    let catalog = fixture.dir.join("web");
    assert!(catalog.join(".git").is_dir());
    let manifest = fs::read_to_string(catalog.join("registry.jsonc")).unwrap();
    assert!(manifest.contains(r#""name": "@acme/web""#), "{manifest}");

    // An empty catalog refreshes to an empty index.
    let refreshed = succeeds(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    assert!(
        stdout(&refreshed).contains("0 units"),
        "{}",
        stdout(&refreshed)
    );
    assert_eq!(index(&catalog)["units"], serde_json::json!({}));

    // A directory with files in it is not used.
    let rejected = fails(fixture.assemblr(&fixture.dir, &["registry", "create", "@acme/web"]));
    assert!(
        stderr(&rejected).contains("is not empty"),
        "{}",
        stderr(&rejected)
    );
    let rejected = fixture.assemblr(&fixture.dir, &["registry", "create", "acme"]);
    assert!(!rejected.status.success());
    assert!(
        stderr(&rejected).contains(r#"invalid catalog "acme""#),
        "{}",
        stderr(&rejected)
    );
}

#[test]
fn refresh_indexes_local_and_remote_units_deterministically() {
    let fixture = Fixture::new("refresh");
    fixture.unit("node", "@acme/node", &["1.0.0", "1.1.0"]);
    let react = fixture.unit("react", "@acme/react", &["2.0.0"]);
    let react_url = url(&react);
    let catalog = fixture.catalog(&format!(
        r#""@acme/react": "{react_url}", "@acme/node": "../node""#
    ));

    let refreshed = succeeds(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    let out = stdout(&refreshed);
    assert!(out.contains("2 units and 3 versions"), "{out}");
    assert!(out.contains("@acme/node: 1.0.0, 1.1.0"), "{out}");
    assert!(
        out.contains("Next: commit and push registry-index.json"),
        "{out}"
    );

    let written = index(&catalog);
    assert_eq!(written["format"], 1);
    assert_eq!(written["name"], "@acme/web");
    let node = &written["units"]["@acme/node"];
    assert_eq!(node["source"], "../node");
    let version = &node["versions"]["1.1.0"];
    assert_eq!(
        version["manifest"]["description"],
        "@acme/node for the web."
    );
    assert_eq!(version["commit"].as_str().unwrap().len(), 40);
    assert_eq!(
        written["units"]["@acme/react"]["source"],
        react_url.as_str()
    );

    // The same sources give the same bytes, and nothing is rewritten.
    let before = fs::read(catalog.join("registry-index.json")).unwrap();
    let again = succeeds(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    assert!(
        stdout(&again).contains("is up to date"),
        "{}",
        stdout(&again)
    );
    assert_eq!(
        fs::read(catalog.join("registry-index.json")).unwrap(),
        before
    );

    // A new version shows after the next refresh.
    publish(&react, "@acme/react", "2.1.0");
    succeeds(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    assert!(index(&catalog)["units"]["@acme/react"]["versions"]["2.1.0"].is_object());
}

#[test]
fn a_unit_whose_repository_is_another_unit_is_rejected() {
    let fixture = Fixture::new("mismatch");
    fixture.unit("node", "@acme/node", &["1.0.0"]);
    let catalog = fixture.catalog(r#""@acme/deno": "../node""#);

    let rejected = fails(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    let err = stderr(&rejected);
    assert!(err.contains("@acme/deno"), "{err}");
    assert!(err.contains("@acme/node"), "{err}");
    assert!(
        err.contains("note: registry-index.json was not changed"),
        "{err}"
    );
    assert!(!catalog.join("registry-index.json").exists());
}

#[test]
fn one_unreadable_unit_leaves_the_index_untouched() {
    let fixture = Fixture::new("unreadable");
    fixture.unit("node", "@acme/node", &["1.0.0"]);
    let catalog = fixture.catalog(r#""@acme/node": "../node""#);
    succeeds(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    let before = fs::read(catalog.join("registry-index.json")).unwrap();

    let missing = url(&fixture.dir.join("missing"));
    fs::write(
        catalog.join("registry.jsonc"),
        format!(
            r#"{{"name": "@acme/web", "units": {{"@acme/node": "../node", "@acme/gone": "{missing}", "@acme/lost": "../lost"}}}}"#
        ),
    )
    .unwrap();
    let rejected = fails(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    let err = stderr(&rejected);
    // Every failing unit is reported, not only the first.
    assert!(err.contains("@acme/gone"), "{err}");
    assert!(err.contains("@acme/lost"), "{err}");
    assert_eq!(
        fs::read(catalog.join("registry-index.json")).unwrap(),
        before
    );
}

#[test]
fn an_invalid_tag_is_excluded_with_a_warning() {
    let fixture = Fixture::new("excluded");
    let node = fixture.unit("node", "@acme/node", &["1.0.0"]);
    // A tag whose manifest declares another version.
    git(&node, &["tag", "v1.1.0"]);
    let catalog = fixture.catalog(r#""@acme/node": "../node""#);

    let refreshed = succeeds(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    assert!(
        stderr(&refreshed).contains("excluded tag v1.1.0"),
        "{}",
        stderr(&refreshed)
    );
    let versions = &index(&catalog)["units"]["@acme/node"]["versions"];
    assert_eq!(versions.as_object().unwrap().len(), 1);
}

#[test]
fn refresh_outside_a_catalog_says_where_to_run_it() {
    let fixture = Fixture::new("outside");
    let rejected = fails(fixture.assemblr(&fixture.dir, &["catalog", "refresh"]));
    let err = stderr(&rejected);
    assert!(err.contains("registry.jsonc"), "{err}");
    assert!(
        err.contains("hint: run the command in the catalog's directory"),
        "{err}"
    );

    let catalog = fixture.dir.join("bad");
    fs::create_dir_all(&catalog).unwrap();
    fs::write(
        catalog.join("registry.jsonc"),
        r#"{"name": "@acme/web", "units": {}, "extends": "x"}"#,
    )
    .unwrap();
    let rejected = fails(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    assert!(
        stderr(&rejected).contains("extends"),
        "{}",
        stderr(&rejected)
    );
}
