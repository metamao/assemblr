//! Catalogs, run as the real binary: creating and refreshing one as its
//! maintainer, and registering and synchronizing one as a consumer. Units
//! and catalogs are repositories in the test's directory, declared by
//! relative path or reached through `file://` URLs, so the tests are
//! offline.

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

    /// The catalog `@acme/web` declaring `units`, refreshed and committed:
    /// what a maintainer pushes.
    fn published_catalog(&self, units: &str) -> PathBuf {
        let catalog = self.catalog(units);
        commit_refresh(self, &catalog);
        catalog
    }

    fn list(&self) -> Value {
        serde_json::from_str(&stdout(&succeeds(
            self.assemblr(&self.dir, &["registry", "list", "--json"]),
        )))
        .unwrap()
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

/// Refreshes the catalog's index and commits it with `registry.jsonc`.
fn commit_refresh(fixture: &Fixture, catalog: &Path) {
    succeeds(fixture.assemblr(catalog, &["catalog", "refresh"]));
    git(catalog, &["add", "-A"]);
    git(catalog, &["commit", "-q", "--allow-empty", "-m", "Refresh"]);
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

#[test]
fn a_remote_catalog_registers_its_units_for_discovery_and_composition() {
    let fixture = Fixture::new("register");
    let node = fixture.unit("node", "@acme/node", &["1.0.0", "1.1.0"]);
    // A unit with a committed manifest and no published version yet.
    let api = fixture.unit("api", "@acme/api", &["0.1.0"]);
    git(&api, &["tag", "-d", "v0.1.0"]);
    let catalog = fixture.published_catalog(&format!(
        r#""@acme/node": "{}", "@acme/api": "{}""#,
        url(&node),
        url(&api)
    ));
    let catalog_url = url(&catalog);

    let added = succeeds(fixture.assemblr(&fixture.dir, &["registry", "add", &catalog_url]));
    let out = stdout(&added);
    assert!(
        out.contains(&format!(
            "Registered the catalog @acme/web from {catalog_url}, with 2 units"
        )),
        "{out}"
    );
    assert!(out.contains("@acme/node: 1.0.0, 1.1.0"), "{out}");
    assert!(out.contains("@acme/api: no published versions"), "{out}");

    let listing = fixture.list();
    assert_eq!(listing["catalogs"][0]["identity"], "@acme/web");
    assert_eq!(listing["catalogs"][0]["source"], catalog_url.as_str());
    assert_eq!(
        listing["catalogs"][0]["commit"],
        git(&catalog, &["rev-parse", "HEAD"]).trim()
    );
    let node_entry = &listing["units"][1];
    assert_eq!(node_entry["identity"], "@acme/node");
    assert_eq!(node_entry["direct"], false);
    assert_eq!(node_entry["catalogs"][0], "@acme/web");

    // Discovery and composition read Assemblr's copies, offline.
    fs::remove_dir_all(&node).unwrap();
    let found = stdout(&succeeds(
        fixture.assemblr(&fixture.dir, &["search", "web"]),
    ));
    assert!(
        found.contains("@acme/node 1.1.0: @acme/node for the web."),
        "{found}"
    );
    let project = fixture.dir.join("app");
    succeeds(fixture.assemblr(&fixture.dir, &["init", "app"]));
    succeeds(fixture.assemblr(&project, &["add", "@acme/node"]));
    assert_eq!(
        fs::read_to_string(project.join("file.txt")).unwrap(),
        "1.1.0\n"
    );

    // Registering it again changes nothing.
    let again = succeeds(fixture.assemblr(&fixture.dir, &["registry", "add", &catalog_url]));
    assert!(stdout(&again).contains("already registered as the catalog @acme/web"));
}

#[test]
fn sync_follows_the_catalog_index_not_the_units_tags() {
    let fixture = Fixture::new("sync");
    let node = fixture.unit("node", "@acme/node", &["1.0.0"]);
    let api = fixture.unit("api", "@acme/api", &["0.1.0"]);
    let catalog = fixture.published_catalog(&format!(r#""@acme/node": "{}""#, url(&node)));
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "add", &url(&catalog)]));

    // A new tag is not discoverable until the catalog lists it.
    publish(&node, "@acme/node", "1.1.0");
    let synced = succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    assert!(
        stdout(&synced).contains("Synchronized 1 unit and 1 catalog; no new versions"),
        "{}",
        stdout(&synced)
    );
    commit_refresh(&fixture, &catalog);
    let synced = stdout(&succeeds(
        fixture.assemblr(&fixture.dir, &["registry", "sync"]),
    ));
    assert!(synced.contains("@acme/node: indexed 1.1.0"), "{synced}");

    // A unit the catalog adds is registered, and one it drops leaves.
    fs::write(
        catalog.join("registry.jsonc"),
        format!(
            r#"{{"name": "@acme/web", "units": {{"@acme/api": "{}"}}}}"#,
            url(&api)
        ),
    )
    .unwrap();
    commit_refresh(&fixture, &catalog);
    let synced = stdout(&succeeds(
        fixture.assemblr(&fixture.dir, &["registry", "sync"]),
    ));
    assert!(synced.contains("@acme/api: indexed 0.1.0"), "{synced}");
    assert!(synced.contains("@acme/node: removed"), "{synced}");
    let units = fixture.list()["units"].clone();
    assert_eq!(units.as_array().unwrap().len(), 1);
    assert_eq!(units[0]["identity"], "@acme/api");
}

#[test]
fn a_catalog_whose_index_does_not_describe_its_units_is_rejected() {
    let fixture = Fixture::new("stale");
    let node = fixture.unit("node", "@acme/node", &["1.0.0"]);
    let catalog = fixture.catalog(&format!(r#""@acme/node": "{}""#, url(&node)));
    git(&catalog, &["add", "-A"]);
    git(&catalog, &["commit", "-q", "-m", "No index"]);

    // No index committed.
    let rejected = fails(fixture.assemblr(&fixture.dir, &["registry", "add", &url(&catalog)]));
    let err = stderr(&rejected);
    assert!(
        err.contains("has no committed registry-index.json"),
        "{err}"
    );
    assert!(
        err.contains("hint: the catalog's maintainer runs `assemblr catalog refresh`"),
        "{err}"
    );
    assert!(err.contains("note: the registry was not changed"), "{err}");

    // An index the declaration has moved past.
    commit_refresh(&fixture, &catalog);
    let other = fixture.unit("other", "@acme/other", &[]);
    fs::write(
        catalog.join("registry.jsonc"),
        format!(
            r#"{{"name": "@acme/web", "units": {{"@acme/node": "{}", "@acme/other": "{}"}}}}"#,
            url(&node),
            url(&other)
        ),
    )
    .unwrap();
    git(
        &catalog,
        &["commit", "-q", "-am", "Declare without refreshing"],
    );
    let rejected = fails(fixture.assemblr(&fixture.dir, &["registry", "add", &url(&catalog)]));
    assert!(
        stderr(&rejected).contains("does not list @acme/other"),
        "{}",
        stderr(&rejected)
    );

    // A listed version whose tag moved.
    git(&catalog, &["reset", "-q", "--hard", "HEAD~1"]);
    fs::write(node.join("src/file.txt"), "moved\n").unwrap();
    git(&node, &["commit", "-q", "-am", "moved"]);
    git(&node, &["tag", "-f", "v1.0.0"]);
    let rejected = fails(fixture.assemblr(&fixture.dir, &["registry", "add", &url(&catalog)]));
    assert!(
        stderr(&rejected).contains("the catalog's index lists @acme/node 1.0.0, but its tag"),
        "{}",
        stderr(&rejected)
    );
    assert_eq!(fixture.list()["units"], serde_json::json!([]));
}

#[test]
fn a_local_catalog_is_read_as_committed_and_resolves_relative_paths() {
    let fixture = Fixture::new("local");
    fixture.unit("node", "@acme/node", &["1.0.0"]);
    let catalog = fixture.published_catalog(r#""@acme/node": "../node""#);
    // An uncommitted edit is not read.
    fs::write(catalog.join("registry.jsonc"), "not json").unwrap();

    succeeds(fixture.assemblr(&fixture.dir, &["registry", "add", "web"]));
    let listing = fixture.list();
    let source = listing["units"][0]["source"].as_str().unwrap();
    assert!(Path::new(source).is_absolute(), "{source}");
    assert!(source.ends_with("node"), "{source}");

    // A remote catalog cannot declare a local path.
    let fixture = Fixture::new("remote_relative");
    fixture.unit("node", "@acme/node", &["1.0.0"]);
    let catalog = fixture.published_catalog(r#""@acme/node": "../node""#);
    let rejected = fails(fixture.assemblr(&fixture.dir, &["registry", "add", &url(&catalog)]));
    assert!(
        stderr(&rejected).contains("which a remote catalog cannot declare"),
        "{}",
        stderr(&rejected)
    );
}

#[test]
fn units_and_catalogs_are_removed_by_what_registered_them() {
    let fixture = Fixture::new("remove");
    let node = fixture.unit("node", "@acme/node", &["1.0.0"]);
    let api = fixture.unit("api", "@acme/api", &["1.0.0"]);
    let catalog = fixture.published_catalog(&format!(
        r#""@acme/node": "{}", "@acme/api": "{}""#,
        url(&node),
        url(&api)
    ));
    // The same reference registered directly and through the catalog is one
    // registration.
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "add", &url(&api)]));
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "add", &url(&catalog)]));

    let rejected = fails(fixture.assemblr(&fixture.dir, &["registry", "remove", "@acme/node"]));
    let err = stderr(&rejected);
    assert!(
        err.contains("registered through the catalog @acme/web"),
        "{err}"
    );
    assert!(err.contains("registry remove --catalog @acme/web"), "{err}");

    let rejected = fails(fixture.assemblr(&fixture.dir, &["registry", "remove", "@acme/web"]));
    assert!(
        stderr(&rejected).contains("registry remove --catalog @acme/web"),
        "{}",
        stderr(&rejected)
    );

    let removed = succeeds(fixture.assemblr(&fixture.dir, &["registry", "remove", "@acme/api"]));
    assert!(
        stdout(&removed).contains("stays registered through @acme/web"),
        "{}",
        stdout(&removed)
    );

    let removed = stdout(&succeeds(fixture.assemblr(
        &fixture.dir,
        &["registry", "remove", "--catalog", "@acme/web"],
    )));
    assert!(
        removed.contains("and 2 units only it declared: @acme/api, @acme/node"),
        "{removed}"
    );
    assert_eq!(
        fixture.list(),
        serde_json::json!({ "units": [], "catalogs": [] })
    );
}

#[test]
fn a_catalog_cannot_give_a_registered_unit_another_repository() {
    let fixture = Fixture::new("conflict");
    let node = fixture.unit("node", "@acme/node", &["1.0.0"]);
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "add", "node"]));
    let before = fixture.list();
    let catalog = fixture.published_catalog(&format!(r#""@acme/node": "{}""#, url(&node)));

    let rejected = fails(fixture.assemblr(&fixture.dir, &["registry", "add", &url(&catalog)]));
    assert!(
        stderr(&rejected).contains("source conflict"),
        "{}",
        stderr(&rejected)
    );
    assert_eq!(fixture.list(), before);
}

#[test]
fn publishing_a_catalog_managed_unit_points_to_the_catalog() {
    let fixture = Fixture::new("publish");
    let node = fixture.unit("node", "@acme/node", &["1.0.0"]);
    fixture.published_catalog(r#""@acme/node": "../node""#);
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "add", "web"]));

    fs::write(
        node.join("manifest.jsonc"),
        r#"{"name": "@acme/node", "version": "1.1.0", "description": "Node."}"#,
    )
    .unwrap();
    succeeds(fixture.assemblr(&node, &["unit", "build"]));
    fs::write(node.join(".gitignore"), "build/\n").unwrap();
    git(&node, &["add", "-A"]);
    git(&node, &["commit", "-q", "-m", "1.1.0"]);
    let published = stdout(&succeeds(fixture.assemblr(&node, &["unit", "publish"])));

    assert!(published.contains("through @acme/web"), "{published}");
    assert!(
        published.contains("assemblr catalog refresh"),
        "{published}"
    );
    let versions = &fixture.list()["units"][0]["versions"];
    assert_eq!(versions.as_array().unwrap().len(), 1);
}
