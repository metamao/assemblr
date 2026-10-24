//! Catalogs, run as the real binary: creating, editing, and refreshing one
//! as its maintainer, and registering and synchronizing one as a consumer.
//! Units and catalogs are repositories in the test's directory, reached
//! through `file://` URLs, so the tests are offline.

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

/// What `catalog init` writes for the maintainer to fill in and commit,
/// which the catalog's own commits leave alone.
const UNTRACKED_SCAFFOLD: &str = "?? AGENTS.md\n?? README.md";

impl Fixture {
    fn assemblr(&self, dir: &Path, args: &[&str]) -> Output {
        self.command(dir, args).output().unwrap()
    }

    /// The catalog `@acme/web` in `web`, declaring `units`.
    fn catalog(&self, units: &str) -> PathBuf {
        succeeds(self.assemblr(&self.dir, &["catalog", "init", "@acme/web"]));
        let catalog = self.dir.join("web");
        fs::write(
            catalog.join("registry.jsonc"),
            format!(r#"{{"name": "@acme/web", "units": {{{units}}}}}"#),
        )
        .unwrap();
        catalog
    }

    /// The catalog `@acme/web` declaring `units`, refreshed and committed:
    /// what a maintainer pushes. The copies the refresh fetched are removed,
    /// so a consumer starts with none, as on another machine.
    fn published_catalog(&self, units: &str) -> PathBuf {
        let catalog = self.catalog(units);
        commit_refresh(self, &catalog);
        let _ = fs::remove_dir_all(self.data.join("sources"));
        catalog
    }

    /// How many remote repositories Assemblr holds copies of.
    fn copies(&self) -> usize {
        fs::read_dir(self.data.join("sources")).map_or(0, |entries| entries.count())
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

/// Refreshes the catalog's index, which commits it with `registry.jsonc`.
fn commit_refresh(fixture: &Fixture, catalog: &Path) {
    succeeds(fixture.assemblr(catalog, &["catalog", "refresh"]));
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
    let created = succeeds(fixture.assemblr(&fixture.dir, &["catalog", "init", "@acme/web"]));
    assert!(
        stdout(&created).contains("Created the catalog @acme/web in web"),
        "{}",
        stdout(&created)
    );
    let catalog = fixture.dir.join("web");
    assert!(catalog.join(".git").is_dir());
    let manifest = fs::read_to_string(catalog.join("registry.jsonc")).unwrap();
    assert!(manifest.contains(r#""name": "@acme/web""#), "{manifest}");
    // A README and an AGENTS.md to fill in, the latter pointing to the
    // guide.
    assert!(
        stdout(&created).contains("Fill in README.md and AGENTS.md"),
        "{}",
        stdout(&created)
    );
    let readme = fs::read_to_string(catalog.join("README.md")).unwrap();
    assert!(readme.starts_with("# @acme/web\n"), "{readme}");
    let agents = fs::read_to_string(catalog.join("AGENTS.md")).unwrap();
    assert!(agents.contains("run `assemblr guide`"), "{agents}");

    // An empty catalog refreshes to an empty index.
    let refreshed = succeeds(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    assert!(
        stdout(&refreshed).contains("0 units"),
        "{}",
        stdout(&refreshed)
    );
    assert_eq!(index(&catalog)["units"], serde_json::json!({}));

    // A directory with files in it is not used.
    let rejected = fails(fixture.assemblr(&fixture.dir, &["catalog", "init", "@acme/web"]));
    assert!(
        stderr(&rejected).contains("is not empty"),
        "{}",
        stderr(&rejected)
    );
    let rejected = fixture.assemblr(&fixture.dir, &["catalog", "init", "acme"]);
    assert!(!rejected.status.success());
    assert!(
        stderr(&rejected).contains(r#"invalid catalog "acme""#),
        "{}",
        stderr(&rejected)
    );
}

#[test]
fn refresh_indexes_units_deterministically_and_commits() {
    let fixture = Fixture::new("refresh");
    let node_url = url(&fixture.unit("node", "@acme/node", &["1.0.0", "1.1.0"]));
    let react = fixture.unit("react", "@acme/react", &["2.0.0"]);
    let react_url = url(&react);
    let catalog = fixture.catalog(&format!(
        r#""@acme/react": "{react_url}", "@acme/node": "{node_url}""#
    ));

    let refreshed = succeeds(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    let out = stdout(&refreshed);
    assert!(out.contains("2 units and 3 versions"), "{out}");
    assert!(out.contains("@acme/node: 1.0.0, 1.1.0"), "{out}");
    assert!(
        out.contains("Committed registry.jsonc and registry-index.json"),
        "{out}"
    );
    assert!(out.contains("Next: run `git push`"), "{out}");
    assert_eq!(
        git(&catalog, &["status", "--porcelain"]),
        UNTRACKED_SCAFFOLD
    );
    assert_eq!(
        git(&catalog, &["log", "-1", "--format=%s"]).trim(),
        "Refresh the catalog index"
    );

    let written = index(&catalog);
    assert_eq!(written["format"], 1);
    assert_eq!(written["name"], "@acme/web");
    let node = &written["units"]["@acme/node"];
    assert_eq!(node["source"], node_url.as_str());
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

    // The same sources give the same bytes, and nothing is rewritten or
    // committed.
    let before = fs::read(catalog.join("registry-index.json")).unwrap();
    let again = succeeds(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    assert!(
        stdout(&again).contains("is up to date"),
        "{}",
        stdout(&again)
    );
    assert!(!stdout(&again).contains("Committed"), "{}", stdout(&again));
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
    let node = fixture.unit("node", "@acme/node", &["1.0.0"]);
    let catalog = fixture.catalog(&format!(r#""@acme/deno": "{}""#, url(&node)));

    let rejected = fails(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    let err = stderr(&rejected);
    assert!(err.contains("@acme/deno"), "{err}");
    assert!(err.contains("@acme/node"), "{err}");
    assert!(err.contains("note: nothing was changed"), "{err}");
    assert!(!catalog.join("registry-index.json").exists());
}

#[test]
fn one_unreadable_unit_leaves_the_index_untouched() {
    let fixture = Fixture::new("unreadable");
    let node = url(&fixture.unit("node", "@acme/node", &["1.0.0"]));
    let catalog = fixture.catalog(&format!(r#""@acme/node": "{node}""#));
    succeeds(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    let before = fs::read(catalog.join("registry-index.json")).unwrap();

    // One unit that cannot be fetched, and one declared by a local path.
    let missing = url(&fixture.dir.join("missing"));
    fs::write(
        catalog.join("registry.jsonc"),
        format!(
            r#"{{"name": "@acme/web", "units": {{"@acme/node": "{node}", "@acme/gone": "{missing}", "@acme/lost": "../node"}}}}"#
        ),
    )
    .unwrap();
    let rejected = fails(fixture.assemblr(&catalog, &["catalog", "refresh"]));
    let err = stderr(&rejected);
    // Every failing unit is reported, not only the first.
    assert!(err.contains("@acme/gone"), "{err}");
    assert!(
        err.contains(r#"@acme/lost: "../node" is a local path, which a catalog cannot declare"#),
        "{err}"
    );
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
    let catalog = fixture.catalog(&format!(r#""@acme/node": "{}""#, url(&node)));

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
fn an_up_to_date_refresh_has_nothing_to_commit_or_suggest() {
    let fixture = Fixture::new("up_to_date");
    let node = url(&fixture.unit("node", "@acme/node", &["1.0.0"]));
    // A catalog outside any Git repository, refreshed once already.
    let catalog = fixture.dir.join("plain");
    fs::create_dir_all(&catalog).unwrap();
    fs::write(
        catalog.join("registry.jsonc"),
        format!(r#"{{"name": "@acme/web", "units": {{"@acme/node": "{node}"}}}}"#),
    )
    .unwrap();
    succeeds(fixture.assemblr(&catalog, &["catalog", "refresh", "--no-commit"]));

    for args in [
        &["catalog", "refresh"][..],
        &["catalog", "refresh", "--no-commit"],
    ] {
        let out = stdout(&succeeds(fixture.assemblr(&catalog, args)));
        assert!(out.contains("is up to date"), "{out}");
        assert!(!out.contains("Next:"), "{out}");
    }

    // Writing a change outside a repository cannot be committed, and says so.
    let api = fixture.unit("api", "@acme/api", &["1.0.0"]);
    let rejected = stderr(&fails(
        fixture.assemblr(&catalog, &["catalog", "add", &url(&api)]),
    ));
    assert!(
        rejected.contains(
            "note: registry.jsonc and registry-index.json were written but not committed"
        ),
        "{rejected}"
    );
    assert!(
        rejected.contains("git commit -- registry.jsonc registry-index.json"),
        "{rejected}"
    );
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

    // Registration read the catalog alone: no unit was fetched, and
    // discovery reads the catalog's metadata.
    assert_eq!(fixture.copies(), 1);
    let found = stdout(&succeeds(
        fixture.assemblr(&fixture.dir, &["search", "web"]),
    ));
    assert!(
        found.contains("@acme/node 1.1.0: @acme/node for the web."),
        "{found}"
    );
    let info = stdout(&succeeds(
        fixture.assemblr(&fixture.dir, &["info", "@acme/node"]),
    ));
    assert!(info.contains("@acme/node 1.1.0"), "{info}");
    assert_eq!(fixture.copies(), 1);

    // Composition retrieves the unit it needs, and only that one.
    let project = fixture.dir.join("app");
    succeeds(fixture.assemblr(&fixture.dir, &["init", "app"]));
    succeeds(fixture.assemblr(&project, &["add", "@acme/node"]));
    assert_eq!(fixture.copies(), 2);
    // Later builds read the copy, offline.
    fs::remove_dir_all(&node).unwrap();
    succeeds(fixture.assemblr(&project, &["compose"]));
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

    // A listed manifest that is not one of the unit it is listed for.
    git(&catalog, &["reset", "-q", "--hard", "HEAD~1"]);
    let mut listed = index(&catalog);
    listed["units"]["@acme/node"]["versions"]["1.0.0"]["manifest"]["name"] =
        serde_json::json!("@acme/other");
    fs::write(
        catalog.join("registry-index.json"),
        serde_json::to_string_pretty(&listed).unwrap(),
    )
    .unwrap();
    git(&catalog, &["commit", "-q", "-am", "Tamper"]);
    let rejected = fails(fixture.assemblr(&fixture.dir, &["registry", "add", &url(&catalog)]));
    assert!(
        stderr(&rejected)
            .contains("the manifest it lists for @acme/node 1.0.0 declares @acme/other"),
        "{}",
        stderr(&rejected)
    );
    assert_eq!(fixture.list()["units"], serde_json::json!([]));
}

#[test]
fn a_local_catalog_is_read_as_committed() {
    let fixture = Fixture::new("local");
    let node = fixture.unit("node", "@acme/node", &["1.0.0"]);
    let catalog = fixture.published_catalog(&format!(r#""@acme/node": "{}""#, url(&node)));
    // An uncommitted edit is not read.
    fs::write(catalog.join("registry.jsonc"), "not json").unwrap();

    succeeds(fixture.assemblr(&fixture.dir, &["registry", "add", "web"]));
    let listing = fixture.list();
    assert_eq!(listing["units"][0]["source"], url(&node).as_str());
}

#[test]
fn a_catalog_cannot_declare_a_local_path() {
    let fixture = Fixture::new("local_path");
    let node = fixture.unit("node", "@acme/node", &["1.0.0"]);
    let catalog = fixture.published_catalog(&format!(r#""@acme/node": "{}""#, url(&node)));
    // A declaration and an index that agree on a local path, written by
    // hand, as an earlier Assemblr accepted.
    for file in ["registry.jsonc", "registry-index.json"] {
        let text = fs::read_to_string(catalog.join(file)).unwrap();
        fs::write(catalog.join(file), text.replace(&url(&node), "../node")).unwrap();
    }
    git(&catalog, &["commit", "-q", "-am", "Declare a path"]);

    for reference in ["web".to_owned(), url(&catalog)] {
        let rejected = fails(fixture.assemblr(&fixture.dir, &["registry", "add", &reference]));
        assert!(
            stderr(&rejected).contains("is a local path, which a catalog cannot declare"),
            "{}",
            stderr(&rejected)
        );
    }
    assert_eq!(
        fixture.list(),
        serde_json::json!({ "units": [], "catalogs": [] })
    );
}

#[test]
fn add_and_remove_edit_refresh_and_commit_the_catalog() {
    let fixture = Fixture::new("entries");
    let node = url(&fixture.unit("node", "@acme/node", &["1.0.0", "1.1.0"]));
    let api = url(&fixture.unit("api", "@acme/api", &["0.1.0"]));
    // A unit with a committed manifest and no published version yet.
    let deno = fixture.unit("deno", "@acme/deno", &["0.1.0"]);
    git(&deno, &["tag", "-d", "v0.1.0"]);
    let deno = url(&deno);
    succeeds(fixture.assemblr(&fixture.dir, &["catalog", "init", "@acme/web"]));
    let catalog = fixture.dir.join("web");
    let commits = || git(&catalog, &["log", "--format=%s"]);

    // The name is read from the unit's repository, and the entry is
    // committed with the index, in a catalog with no commit yet.
    let added = stdout(&succeeds(
        fixture.assemblr(&catalog, &["catalog", "add", &node]),
    ));
    assert!(
        added.contains("Added @acme/node to the catalog @acme/web, with versions 1.0.0, 1.1.0"),
        "{added}"
    );
    assert!(added.contains("Next: run `git push`"), "{added}");
    assert_eq!(commits(), "Add @acme/node to the catalog");
    assert_eq!(
        git(&catalog, &["status", "--porcelain"]),
        UNTRACKED_SCAFFOLD
    );
    let versions = &index(&catalog)["units"]["@acme/node"]["versions"];
    assert_eq!(versions.as_object().unwrap().len(), 2);

    // An entry goes where its name sorts, and the file keeps its comments.
    let declaration = catalog.join("registry.jsonc");
    let text = fs::read_to_string(&declaration).unwrap();
    fs::write(&declaration, format!("// The web units.\n{text}")).unwrap();
    git(&catalog, &["commit", "-q", "-am", "Comment"]);
    succeeds(fixture.assemblr(&catalog, &["catalog", "add", &api]));
    let text = fs::read_to_string(&declaration).unwrap();
    assert!(text.starts_with("// The web units."), "{text}");
    let api_at = text.find("@acme/api").unwrap();
    assert!(api_at < text.find("@acme/node").unwrap(), "{text}");

    // Adding it again changes nothing.
    let again = stdout(&succeeds(
        fixture.assemblr(&catalog, &["catalog", "add", &api]),
    ));
    assert!(again.contains("declares @acme/api from"), "{again}");
    assert!(again.contains("nothing changed"), "{again}");

    // A local path, a repository that cannot be read, and a second
    // repository for a declared unit are refused, and nothing is written.
    let before = (
        fs::read(&declaration).unwrap(),
        fs::read(catalog.join("registry-index.json")).unwrap(),
        commits(),
    );
    let state = || {
        (
            fs::read(&declaration).unwrap(),
            fs::read(catalog.join("registry-index.json")).unwrap(),
            commits(),
        )
    };
    let rejected = stderr(&fails(
        fixture.assemblr(&catalog, &["catalog", "add", "../node"]),
    ));
    assert!(
        rejected.contains(r#""../node" is a local path, which a catalog cannot declare"#),
        "{rejected}"
    );
    assert!(rejected.contains("note: nothing was changed"), "{rejected}");
    let missing = url(&fixture.dir.join("missing"));
    let rejected = stderr(&fails(
        fixture.assemblr(&catalog, &["catalog", "add", &missing]),
    ));
    assert!(rejected.contains("cannot be added"), "{rejected}");
    git(
        &fixture.dir,
        &["clone", "-q", "--bare", "node", "node-mirror.git"],
    );
    let mirror = url(&fixture.dir.join("node-mirror.git"));
    let rejected = stderr(&fails(
        fixture.assemblr(&catalog, &["catalog", "add", &mirror]),
    ));
    assert!(rejected.contains("declares @acme/node from"), "{rejected}");
    assert!(
        rejected.contains("assemblr catalog remove @acme/node"),
        "{rejected}"
    );
    // A unit with no published version is declared all the same.
    let added = stdout(&succeeds(
        fixture.assemblr(&catalog, &["catalog", "add", &deno, "--no-commit"]),
    ));
    assert!(added.contains("with no published version"), "{added}");
    assert!(
        added.contains("Next: commit and push registry.jsonc and registry-index.json"),
        "{added}"
    );
    assert_eq!(commits(), before.2);
    assert_ne!(state(), before);
    git(&catalog, &["checkout", "-q", "--", "."]);
    assert_eq!(state(), before);

    // Removing takes the entry and its versions, and commits.
    let removed = stdout(&succeeds(
        fixture.assemblr(&catalog, &["catalog", "remove", "@acme/node"]),
    ));
    assert!(
        removed.contains(&format!("Removed @acme/node, declared from {node}")),
        "{removed}"
    );
    assert!(commits().starts_with("Remove @acme/node from the catalog\n"));
    assert!(
        !fs::read_to_string(&declaration)
            .unwrap()
            .contains("@acme/node")
    );
    assert!(index(&catalog)["units"].get("@acme/node").is_none());
    let rejected = stderr(&fails(
        fixture.assemblr(&catalog, &["catalog", "remove", "@acme/node"]),
    ));
    assert!(
        rejected.contains("the catalog @acme/web does not declare @acme/node"),
        "{rejected}"
    );
}

#[test]
fn a_catalog_commit_leaves_other_changes_alone() {
    let fixture = Fixture::new("other_changes");
    let node = url(&fixture.unit("node", "@acme/node", &["1.0.0"]));
    let catalog = fixture.published_catalog("");
    fs::write(catalog.join("README.md"), "Web units.\n").unwrap();
    git(&catalog, &["add", "README.md"]);

    succeeds(fixture.assemblr(&catalog, &["catalog", "add", &node]));
    let committed = git(&catalog, &["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(committed, "registry-index.json\nregistry.jsonc");
    assert_eq!(
        git(&catalog, &["status", "--porcelain"]),
        "A  README.md\n?? AGENTS.md"
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
    // The catalog declares the unit by its URL, and the author publishes
    // from a checkout, whose path is another reference: the version still
    // reaches discovery only through the catalog.
    let catalog = fixture.published_catalog(&format!(r#""@acme/node": "{}""#, url(&node)));
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "add", &url(&catalog)]));

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
    assert!(
        !published.contains("not from this repository"),
        "{published}"
    );
    let versions = &fixture.list()["units"][0]["versions"];
    assert_eq!(versions.as_array().unwrap().len(), 1);
}

#[test]
fn a_moved_tag_is_found_when_the_content_is_retrieved() {
    let fixture = Fixture::new("moved");
    let node = fixture.unit("node", "@acme/node", &["1.0.0"]);
    let catalog = fixture.published_catalog(&format!(r#""@acme/node": "{}""#, url(&node)));
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "add", &url(&catalog)]));

    // The tagged commit is rewritten after the catalog indexed it, so the
    // recorded commit no longer exists.
    fs::write(node.join("src/file.txt"), "moved\n").unwrap();
    git(&node, &["commit", "-q", "--amend", "-am", "moved"]);
    git(&node, &["tag", "-f", "v1.0.0"]);
    git(&node, &["reflog", "expire", "--expire=now", "--all"]);
    git(&node, &["gc", "-q", "--prune=now"]);

    let project = fixture.dir.join("app");
    succeeds(fixture.assemblr(&fixture.dir, &["init", "app"]));
    let rejected = fails(fixture.assemblr(&project, &["add", "@acme/node"]));
    assert!(!project.join("file.txt").exists());
    assert!(
        stderr(&rejected).contains("could not be retrieved")
            && stderr(&rejected).contains("git ls-remote"),
        "{}",
        stderr(&rejected)
    );
}
