//! `assemblr unit check` and `assemblr unit publish`, run as a real process
//! against unit repositories and registries built in temporary directories.
//!
//! Every test gives the binary its own registry through `ASSEMBLR_DATA_DIR`
//! and its own home directory, so the developer's registry and Git
//! configuration are never read or changed. The binary gets a Git identity
//! through the environment, as an author's shell would provide one, except
//! where a test takes it away.

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
    /// An empty unit repository named `name`.
    fn repo(&self, name: &str) -> Repo {
        git(&self.dir, &["init", "-q", name]);
        Repo {
            path: self.dir.join(name),
        }
    }

    /// Runs `assemblr` in `dir`.
    fn assemblr(&self, dir: &Path, args: &[&str]) -> Output {
        self.command(dir, args).output().unwrap()
    }

    fn register(&self, repo: &Repo) -> Output {
        let path = repo.path.to_str().unwrap();
        succeeds(self.assemblr(&self.dir, &["registry", "add", path]))
    }

    fn build(&self, repo: &Repo) -> Output {
        self.assemblr(&repo.path, &["unit", "build"])
    }

    fn check(&self, repo: &Repo) -> Output {
        self.assemblr(&repo.path, &["unit", "check"])
    }

    fn publish(&self, repo: &Repo) -> Output {
        self.assemblr(&repo.path, &["unit", "publish"])
    }

    fn list(&self) -> Value {
        let output = succeeds(self.assemblr(&self.dir, &["registry", "list", "--json"]));
        serde_json::from_str(&stdout(&output)).unwrap()
    }

    /// The versions the registry lists for `identity`.
    fn versions(&self, identity: &str) -> Vec<String> {
        self.list()["units"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|unit| unit["identity"] == identity)
            .flat_map(|unit| unit["versions"].as_array().unwrap().clone())
            .map(|version| version["version"].as_str().unwrap().to_owned())
            .collect()
    }

    fn index_file(&self) -> PathBuf {
        self.data.join("registry").join("index.json")
    }
}

/// A manifest with these dependencies and extra top-level fields.
fn manifest(name: &str, version: &str, dependencies: &[(&str, &str)], extra: &str) -> String {
    let dependencies: Vec<String> = dependencies
        .iter()
        .map(|(identity, range)| format!(r#""{identity}": "{range}""#))
        .collect();
    format!(
        r#"{{"name": "{name}", "version": "{version}", "description": "A unit.",
            "dependencies": {{{}}}{extra}}}"#,
        dependencies.join(", ")
    )
}

/// A unit repository with a working tree.
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

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.path.join(path)).unwrap()
    }

    /// Commits everything, `build/` excepted.
    fn commit(&self, message: &str) {
        if !self.path.join(".gitignore").exists() {
            self.write(".gitignore", "/build/\n");
        }
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
    }

    /// Commits everything and tags it `v<version>` by hand, as the tests
    /// of earlier increments publish.
    fn publish_by_hand(&self, version: &str) {
        self.commit(version);
        self.git(&["tag", &format!("v{version}")]);
    }

    fn rev(&self, name: &str) -> String {
        self.git(&["rev-parse", name])
    }

    fn tags(&self) -> Vec<String> {
        self.git(&["tag", "--list"])
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The files of the working tree, `.git` and `build/` excepted, with
    /// their contents: what a check must leave as it is.
    fn tree(&self) -> Vec<(String, Vec<u8>)> {
        let mut found = Vec::new();
        let mut pending = vec![self.path.clone()];
        while let Some(next) = pending.pop() {
            for entry in fs::read_dir(next).unwrap() {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if path.is_dir() {
                    if path.parent() == Some(&self.path) && (name == ".git" || name == "build") {
                        continue;
                    }
                    pending.push(path);
                } else {
                    let relative = path.strip_prefix(&self.path).unwrap();
                    found.push((
                        relative.to_string_lossy().replace('\\', "/"),
                        fs::read(&path).unwrap(),
                    ));
                }
            }
        }
        found.sort();
        found
    }

    /// The files under `build/`, by path, with their contents.
    fn build_files(&self) -> Vec<(String, String)> {
        let mut found: Vec<(String, String)> = self
            .tree_of(&self.path.join("build"))
            .into_iter()
            .map(|(path, contents)| (path, String::from_utf8(contents).unwrap()))
            .collect();
        found.sort();
        found
    }

    fn tree_of(&self, dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut found = Vec::new();
        let mut pending = vec![dir.to_owned()];
        while let Some(next) = pending.pop() {
            for entry in fs::read_dir(next).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    let relative = path.strip_prefix(dir).unwrap();
                    found.push((
                        relative.to_string_lossy().replace('\\', "/"),
                        fs::read(&path).unwrap(),
                    ));
                }
            }
        }
        found
    }
}

/// The `error:` lines of a failure, without the hint.
fn errors(output: &Output) -> Vec<String> {
    stderr(output)
        .lines()
        .filter(|line| line.starts_with("error:"))
        .map(str::to_owned)
        .collect()
}

/// A registered `@acme/node` with versions 1.0.0 and 1.1.0, published by
/// hand, and an unpublished `@acme/app` that depends on it with `range`.
fn app_on_node(fixture: &Fixture, range: &str) -> (Repo, Repo) {
    let node = fixture.repo("node");
    node.write("manifest.jsonc", &manifest("@acme/node", "1.0.0", &[], ""));
    node.write("src/node.txt", "node 1.0.0");
    node.publish_by_hand("1.0.0");
    node.write("manifest.jsonc", &manifest("@acme/node", "1.1.0", &[], ""));
    node.write("src/node.txt", "node 1.1.0");
    node.publish_by_hand("1.1.0");
    fixture.register(&node);
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.1.0", &[("@acme/node", range)], ""),
    );
    app.write("src/app.txt", "app");
    (node, app)
}

/// Builds and commits `app`, ready to publish.
fn built_and_committed(fixture: &Fixture, app: &Repo, message: &str) {
    succeeds(fixture.build(app));
    app.commit(message);
}

#[test]
fn check_runs_the_build_s_validations_and_writes_nothing() {
    let fixture = Fixture::new("check");
    let (_node, app) = app_on_node(&fixture, "^1");

    // Before the first build there is no lockfile, which a check reports
    // and never creates.
    let before = app.tree();
    let err = stderr(&fails(fixture.check(&app)));
    assert!(err.contains("there is no assemblr-lock.json"), "{err}");
    assert!(
        err.contains("note: nothing was changed\nhint: run `assemblr unit build`"),
        "{err}"
    );
    assert_eq!(app.tree(), before);
    assert!(!app.path.join("build").exists());

    succeeds(fixture.build(&app));
    let before = app.tree();
    let output = succeeds(fixture.check(&app));
    assert_eq!(
        stdout(&output).trim(),
        "Checked @acme/app 0.1.0: no problems; a build delivers 2 files, with 1 dependency."
    );
    assert_eq!(app.tree(), before);

    // A lockfile that no longer records the resolution.
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.1.0", &[("@acme/node", "~1.0.0")], ""),
    );
    let err = stderr(&fails(fixture.check(&app)));
    assert!(
        err.contains(
            "assemblr-lock.json does not record the current resolution: @acme/node 1.1.0 -> \
             1.0.0; run `assemblr unit build`"
        ),
        "{err}"
    );

    // Problems a build reports are reported with the same wording, and the
    // check reports every independent one, in the same order.
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[("@acme/node", "^1"), ("@acme/missing", "^1")],
            "",
        ),
    );
    app.write("src/aux.txt", "reserved on Windows");
    let before = app.tree();
    let check = fails(fixture.check(&app));
    let build = fails(fixture.build(&app));
    assert_eq!(errors(&check), errors(&build));
    assert!(
        stderr(&check).contains("@acme/missing"),
        "{}",
        stderr(&check)
    );
    assert!(stderr(&check).contains("aux.txt"), "{}", stderr(&check));
    assert_eq!(app.tree(), before);
}

#[test]
fn check_rejects_an_ineffective_unit_and_a_redundant_amendment() {
    let fixture = Fixture::new("check-contribution");
    let (_node, app) = app_on_node(&fixture, "^1");

    // No file and no dependency: a build composes it, a check rejects it.
    let empty = fixture.repo("empty");
    empty.write("manifest.jsonc", &manifest("@acme/empty", "0.1.0", &[], ""));
    succeeds(fixture.build(&empty));
    let err = stderr(&fails(fixture.check(&empty)));
    assert!(err.contains("the unit contributes nothing"), "{err}");
    assert!(
        err.contains("add files under `src/`, or declare the dependencies"),
        "{err}"
    );

    // One dependency and nothing else aggregates that dependency.
    let alias = fixture.repo("alias");
    alias.write(
        "manifest.jsonc",
        &manifest("@acme/alias", "0.1.0", &[("@acme/node", "^1")], ""),
    );
    succeeds(fixture.build(&alias));
    let output = succeeds(fixture.check(&alias));
    assert!(
        stdout(&output).contains("Checked @acme/alias 0.1.0: no problems"),
        "{}",
        stdout(&output)
    );

    // An amendment identical to its base is redundant until it is edited.
    succeeds(fixture.build(&app));
    succeeds(fixture.assemblr(&app.path, &["unit", "amend", "node.txt"]));
    succeeds(fixture.build(&app));
    let err = stderr(&fails(fixture.check(&app)));
    assert!(
        err.contains(
            "amendments/node.txt is identical to src/node.txt of @acme/node 1.1.0, so the \
             amendment changes nothing"
        ),
        "{err}"
    );
    app.write("amendments/node.txt", "node, amended");
    succeeds(fixture.check(&app));
}

#[test]
fn check_refuses_while_an_update_is_interrupted() {
    let fixture = Fixture::new("check-interrupted");
    let (_node, app) = app_on_node(&fixture, "^1");
    succeeds(fixture.build(&app));
    app.write(".git/assemblr-update.json", "{}");

    let err = stderr(&fails(fixture.check(&app)));
    assert!(
        err.contains("an update of this unit was interrupted and not completed"),
        "{err}"
    );
    assert!(err.contains("run `assemblr unit update`"), "{err}");
}

#[test]
fn publish_tags_head_indexes_the_version_and_a_dependent_builds_against_it() {
    let fixture = Fixture::new("publish");
    let (node, app) = app_on_node(&fixture, "^1");
    built_and_committed(&fixture, &app, "first");
    fixture.register(&app);
    let head = app.rev("HEAD");

    let output = succeeds(fixture.publish(&app));
    assert_eq!(
        stdout(&output).trim(),
        format!(
            "Published @acme/app 0.1.0 as tag v0.1.0 at commit {}.\nIndexed it in the local \
             registry.\nNext: run `git push --follow-tags` so that other machines can sync it.",
            &head[..12]
        )
    );
    assert_eq!(app.tags(), ["v0.1.0"]);
    assert_eq!(app.git(&["cat-file", "-t", "v0.1.0"]), "tag");
    assert_eq!(app.rev("v0.1.0^{commit}"), head);
    let tag = app.git(&["cat-file", "-p", "v0.1.0"]);
    assert!(tag.contains("tagger Author <author@example.com>"), "{tag}");
    assert!(tag.ends_with("@acme/app 0.1.0"), "{tag}");
    assert_eq!(fixture.versions("@acme/app"), ["0.1.0"]);
    let listing = fixture.list();
    let versions = listing["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|unit| unit["identity"] == "@acme/app")
        .unwrap()["versions"]
        .clone();
    assert_eq!(versions[0]["commit"], head);
    assert_eq!(versions[0]["tree"], app.rev("HEAD^{tree}"));

    // Publishing again creates nothing and succeeds.
    let output = succeeds(fixture.publish(&app));
    assert_eq!(
        stdout(&output).trim(),
        format!(
            "@acme/app 0.1.0 is already published: tag v0.1.0 points at commit {}.\nThe local \
             registry already indexes it.",
            &head[..12]
        )
    );
    assert_eq!(app.tags(), ["v0.1.0"]);

    // A dependent resolves the published version without a synchronization.
    let site = fixture.repo("site");
    site.write(
        "manifest.jsonc",
        &manifest("@acme/site", "0.1.0", &[("@acme/app", "^0.1")], ""),
    );
    site.write("src/site.txt", "site");
    let output = succeeds(fixture.build(&site));
    assert!(
        stdout(&output).contains("with 2 dependencies"),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        site.build_files(),
        [
            ("app.txt".to_owned(), "app".to_owned()),
            ("node.txt".to_owned(), "node 1.1.0".to_owned()),
            ("site.txt".to_owned(), "site".to_owned()),
        ]
    );
    // The published lockfile is the one the build wrote.
    assert_eq!(
        app.git(&["show", "v0.1.0:assemblr-lock.json"]),
        app.read("assemblr-lock.json").trim()
    );
    assert_eq!(node.tags(), ["v1.0.0", "v1.1.0"]);
}

#[test]
fn publish_refuses_and_changes_nothing() {
    let fixture = Fixture::new("publish-refusals");
    let (_node, app) = app_on_node(&fixture, "^1");
    let unchanged = |app: &Repo, before: &[(String, Vec<u8>)]| {
        assert!(app.tags().is_empty(), "{:?}", app.tags());
        assert_eq!(app.tree(), before);
        assert_eq!(fixture.versions("@acme/app"), Vec::<String>::new());
    };

    // No commit yet.
    succeeds(fixture.build(&app));
    let before = app.tree();
    let err = stderr(&fails(fixture.publish(&app)));
    assert!(err.contains("the repository has no commit"), "{err}");
    unchanged(&app, &before);

    // Uncommitted changes and untracked files, named.
    app.commit("first");
    fixture.register(&app);
    app.write("src/app.txt", "changed");
    app.write("src/extra.txt", "untracked");
    app.write("notes.txt", "untracked too");
    let before = app.tree();
    let err = stderr(&fails(fixture.publish(&app)));
    assert!(
        err.contains("the working tree is not committed and clean"),
        "{err}"
    );
    assert!(
        err.contains("error:   notes.txt\nerror:   src/app.txt\nerror:   src/extra.txt"),
        "{err}"
    );
    assert!(
        err.contains("note: no tag was created and nothing was changed"),
        "{err}"
    );
    unchanged(&app, &before);
    // Ignored files do not count; `build/` is ignored.
    app.write("src/app.txt", "app");
    fs::remove_file(app.path.join("src/extra.txt")).unwrap();
    fs::remove_file(app.path.join("notes.txt")).unwrap();
    app.write(".gitignore", "/build/\n*.log\n");
    app.write("debug.log", "ignored");
    app.commit("ignore logs");

    // A stale lockfile, committed.
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.1.0", &[("@acme/node", "~1.0.0")], ""),
    );
    app.commit("narrow the range");
    let before = app.tree();
    let err = stderr(&fails(fixture.publish(&app)));
    assert!(
        err.contains("assemblr-lock.json does not record the current resolution"),
        "{err}"
    );
    unchanged(&app, &before);
    // The same refusal twice gives the same diagnostics.
    assert_eq!(
        errors(&fails(fixture.publish(&app))),
        errors(&fails(fixture.publish(&app)))
    );

    // A problem a check reports: a missing dependency, found on the
    // committed content.
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[("@acme/node", "^1"), ("@acme/missing", "^1")],
            "",
        ),
    );
    app.commit("depend on a missing unit");
    let before = app.tree();
    let err = stderr(&fails(fixture.publish(&app)));
    assert!(err.contains("@acme/missing"), "{err}");
    unchanged(&app, &before);

    // An interrupted update.
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.1.0", &[("@acme/node", "^1")], ""),
    );
    built_and_committed(&fixture, &app, "restore");
    app.write(".git/assemblr-update.json", "{}");
    let err = stderr(&fails(fixture.publish(&app)));
    assert!(
        err.contains("an update of this unit was interrupted"),
        "{err}"
    );
    fs::remove_file(app.path.join(".git/assemblr-update.json")).unwrap();
    assert!(app.tags().is_empty());

    // No tagger identity: Git refuses, and nothing is created.
    app.git(&["config", "user.useConfigOnly", "true"]);
    let output = fixture
        .command(&app.path, &["unit", "publish"])
        .env_remove("GIT_AUTHOR_NAME")
        .env_remove("GIT_AUTHOR_EMAIL")
        .env_remove("GIT_COMMITTER_NAME")
        .env_remove("GIT_COMMITTER_EMAIL")
        .output()
        .unwrap();
    let err = stderr(&fails(output));
    assert!(err.contains("could not create the tag"), "{err}");
    assert!(
        err.contains("`git tag --annotate --message <message> v0.1.0"),
        "{err}"
    );
    assert!(err.contains("Git identity"), "{err}");
    assert!(app.tags().is_empty());
    assert_eq!(fixture.versions("@acme/app"), Vec::<String>::new());
    app.git(&["config", "--unset", "user.useConfigOnly"]);

    // A version published at another commit, and one of equal precedence.
    succeeds(fixture.publish(&app));
    app.write("src/app.txt", "app, changed");
    built_and_committed(&fixture, &app, "change without a new version");
    let err = stderr(&fails(fixture.publish(&app)));
    assert!(
        err.contains("tag v0.1.0 already exists and points at commit"),
        "{err}"
    );
    assert!(err.contains("not at `HEAD`"), "{err}");
    assert!(err.contains("raise the version in manifest.jsonc"), "{err}");
    assert_eq!(app.tags(), ["v0.1.0"]);
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.2.0+build.1", &[("@acme/node", "^1")], ""),
    );
    app.publish_by_hand("0.2.0+build.1");
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.2.0", &[("@acme/node", "^1")], ""),
    );
    built_and_committed(&fixture, &app, "0.2.0");
    let err = stderr(&fails(fixture.publish(&app)));
    assert!(
        err.contains(
            "tag v0.2.0+build.1 publishes version 0.2.0+build.1, which has the same precedence"
        ),
        "{err}"
    );
    assert_eq!(app.tags(), ["v0.1.0", "v0.2.0+build.1"]);
}

#[test]
fn publish_refuses_a_unit_without_a_committed_lockfile_or_contribution() {
    let fixture = Fixture::new("publish-lockfile");
    let (_node, app) = app_on_node(&fixture, "^1");

    // Committed without a build: no lockfile in the commit.
    app.commit("no build yet");
    let err = stderr(&fails(fixture.publish(&app)));
    assert!(err.contains("there is no assemblr-lock.json"), "{err}");
    assert!(err.contains("run `assemblr unit build`"), "{err}");
    assert!(app.tags().is_empty());

    // The lockfile exists but is ignored, so the commit lacks it.
    succeeds(fixture.build(&app));
    app.write(".gitignore", "/build/\nassemblr-lock.json\n");
    app.commit("ignore the lockfile");
    let err = stderr(&fails(fixture.publish(&app)));
    assert!(err.contains("there is no assemblr-lock.json"), "{err}");
    assert!(app.tags().is_empty());

    // A committed lockfile, then a committed manifest, that cannot be read.
    app.write(".gitignore", "/build/\n");
    app.write("assemblr-lock.json", "{");
    app.commit("break the lockfile");
    let err = stderr(&fails(fixture.publish(&app)));
    assert!(
        err.contains("in the commit at `HEAD`, `assemblr-lock.json` is not a valid lockfile"),
        "{err}"
    );
    assert!(err.contains("fix the file, commit, and run"), "{err}");
    fs::remove_file(app.path.join("assemblr-lock.json")).unwrap();
    succeeds(fixture.build(&app));
    app.write("manifest.jsonc", "{\"name\": ");
    app.commit("break the manifest");
    let err = stderr(&fails(fixture.publish(&app)));
    assert!(
        err.contains("the commit at `HEAD` cannot be published:"),
        "{err}"
    );
    assert!(app.tags().is_empty());

    // An ineffective unit.
    let empty = fixture.repo("empty");
    empty.write("manifest.jsonc", &manifest("@acme/empty", "0.1.0", &[], ""));
    built_and_committed(&fixture, &empty, "empty");
    let err = stderr(&fails(fixture.publish(&empty)));
    assert!(err.contains("the unit contributes nothing"), "{err}");
    assert!(empty.tags().is_empty());
}

#[test]
fn an_amended_unit_publishes_the_lockfile_its_dependents_verify() {
    let fixture = Fixture::new("publish-amendment");
    let (node, app) = app_on_node(&fixture, "^1");
    succeeds(fixture.build(&app));
    succeeds(fixture.assemblr(&app.path, &["unit", "amend", "node.txt"]));
    app.write("amendments/node.txt", "node 1.1.0, amended by app");
    built_and_committed(&fixture, &app, "amend node");
    fixture.register(&app);

    succeeds(fixture.publish(&app));
    assert_eq!(fixture.versions("@acme/app"), ["0.1.0"]);

    let site = fixture.repo("site");
    site.write(
        "manifest.jsonc",
        &manifest("@acme/site", "0.1.0", &[("@acme/app", "^0.1")], ""),
    );
    site.write("src/site.txt", "site");
    succeeds(fixture.build(&site));
    assert_eq!(
        site.build_files(),
        [
            ("app.txt".to_owned(), "app".to_owned()),
            (
                "node.txt".to_owned(),
                "node 1.1.0, amended by app".to_owned()
            ),
            ("site.txt".to_owned(), "site".to_owned()),
        ]
    );
    // The published lockfile identifies the base the dependent verified.
    let lock: Value =
        serde_json::from_str(&app.git(&["show", "v0.1.0:assemblr-lock.json"])).unwrap();
    assert_eq!(lock["units"]["@acme/node"]["commit"], node.rev("v1.1.0"));
}

#[test]
fn a_failed_index_update_is_a_partial_outcome_that_sync_recovers() {
    let fixture = Fixture::new("publish-partial");
    let (_node, app) = app_on_node(&fixture, "^1");
    built_and_committed(&fixture, &app, "first");
    fixture.register(&app);
    // The registry writes its new index to this path first.
    let obstacle = fixture.data.join("registry").join("index.json.tmp");
    fs::create_dir(&obstacle).unwrap();

    let output = fails(fixture.publish(&app));
    assert!(
        stdout(&output).contains("Published @acme/app 0.1.0 as tag v0.1.0"),
        "{}",
        stdout(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("error: the registry index was not updated"),
        "{err}"
    );
    assert!(
        err.contains("hint: the published tag stands; run `assemblr registry sync`"),
        "{err}"
    );
    assert_eq!(app.tags(), ["v0.1.0"]);
    assert_eq!(fixture.versions("@acme/app"), Vec::<String>::new());

    // Synchronization indexes it.
    fs::remove_dir(&obstacle).unwrap();
    let output = succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    assert!(
        stdout(&output).contains("@acme/app: indexed 0.1.0"),
        "{}",
        stdout(&output)
    );
    assert_eq!(fixture.versions("@acme/app"), ["0.1.0"]);

    // So does publishing again, when the tag already stands.
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.2.0", &[("@acme/node", "^1")], ""),
    );
    built_and_committed(&fixture, &app, "0.2.0");
    fs::create_dir(&obstacle).unwrap();
    fails(fixture.publish(&app));
    fs::remove_dir(&obstacle).unwrap();
    let output = succeeds(fixture.publish(&app));
    assert!(
        stdout(&output).contains("@acme/app 0.2.0 is already published"),
        "{}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains("Indexed it in the local registry."),
        "{}",
        stdout(&output)
    );
    assert_eq!(fixture.versions("@acme/app"), ["0.1.0", "0.2.0"]);
}

#[test]
fn an_unregistered_unit_publishes_without_touching_the_registry() {
    let fixture = Fixture::new("publish-unregistered");
    let (_node, app) = app_on_node(&fixture, "^1");
    built_and_committed(&fixture, &app, "first");
    let index_before = fs::read(fixture.index_file()).unwrap();

    let output = succeeds(fixture.publish(&app));
    assert!(
        stdout(&output).contains(
            "The local registry does not register @acme/app, so the version is not \
             discoverable until `assemblr registry add"
        ),
        "{}",
        stdout(&output)
    );
    assert_eq!(app.tags(), ["v0.1.0"]);
    assert_eq!(fs::read(fixture.index_file()).unwrap(), index_before);

    // Registering afterwards indexes it.
    let output = fixture.register(&app);
    assert!(
        stdout(&output).contains("with version 0.1.0"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_unit_registered_from_another_clone_publishes_without_an_index_update() {
    let fixture = Fixture::new("publish-other-clone");
    let (_node, app) = app_on_node(&fixture, "^1");
    built_and_committed(&fixture, &app, "first");
    // The registered source is another clone of the same unit.
    git(&fixture.dir, &["clone", "-q", "app", "clone"]);
    let clone = Repo {
        path: fixture.dir.join("clone"),
    };
    fixture.register(&clone);

    let output = succeeds(fixture.publish(&app));
    assert!(
        stdout(&output).contains(&format!(
            "The local registry registers @acme/app from {}, not from this repository",
            clone.path.display()
        )),
        "{}",
        stdout(&output)
    );
    assert_eq!(app.tags(), ["v0.1.0"]);
    assert_eq!(fixture.versions("@acme/app"), Vec::<String>::new());

    // Once the registered clone has the tag, a synchronization indexes it.
    clone.git(&["fetch", "-q", "--tags", "origin"]);
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    assert_eq!(fixture.versions("@acme/app"), ["0.1.0"]);

    // This repository registered under another identity cannot publish a
    // version the registry would reject.
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "remove", "@acme/app"]));
    fixture.register(&app);
    app.write(
        "manifest.jsonc",
        &manifest("@acme/renamed", "0.2.0", &[("@acme/node", "^1")], ""),
    );
    built_and_committed(&fixture, &app, "rename");
    let err = stderr(&fails(fixture.publish(&app)));
    assert!(
        err.contains("as @acme/app, not as the identity the manifest declares"),
        "{err}"
    );
    assert!(
        err.contains("`assemblr registry remove @acme/app`"),
        "{err}"
    );
    assert_eq!(app.tags(), ["v0.1.0"]);
}

/// Registered through a symbolic link and published from the directory it
/// names, as on macOS, where `/tmp` and `/var` are links into `/private`:
/// the working directory the process reports is the resolved path, yet the
/// repository is the registered one.
#[cfg(unix)]
#[test]
fn a_unit_registered_through_a_symbolic_link_is_indexed_on_publish() {
    let fixture = Fixture::new("publish-symlink");
    let (_node, app) = app_on_node(&fixture, "^1");
    built_and_committed(&fixture, &app, "first");
    let link = fixture.dir.join("linked");
    std::os::unix::fs::symlink(&app.path, &link).unwrap();
    let registered = link.to_str().unwrap();
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "add", registered]));

    let output = succeeds(fixture.publish(&app));
    assert!(
        stdout(&output).contains("Indexed it in the local registry"),
        "{}",
        stdout(&output)
    );
    assert_eq!(fixture.versions("@acme/app"), ["0.1.0"]);
    // The registration keeps the reference it was given.
    let list = fixture.list();
    let sources: Vec<&str> = list["units"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|unit| unit["identity"] == "@acme/app")
        .map(|unit| unit["source"].as_str().unwrap())
        .collect();
    assert_eq!(sources, [registered]);

    // A new identity is refused through the link too, before any tag.
    app.write(
        "manifest.jsonc",
        &manifest("@acme/renamed", "0.2.0", &[("@acme/node", "^1")], ""),
    );
    built_and_committed(&fixture, &app, "rename");
    let err = stderr(&fails(fixture.publish(&app)));
    assert!(
        err.contains(&format!(
            "the registry registers {registered} as @acme/app, not as the identity the \
             manifest declares"
        )),
        "{err}"
    );
    assert_eq!(app.tags(), ["v0.1.0"]);
}

#[test]
fn publish_refuses_a_tag_the_registry_would_not_index() {
    let fixture = Fixture::new("publish-rehearsal");
    let (node, _app) = app_on_node(&fixture, "^1");
    // An indexed tag moved: the source is in conflict, and a new tag would
    // never be indexed until it is restored.
    node.write("src/node.txt", "moved");
    node.commit("move");
    node.git(&["tag", "-f", "v1.0.0"]);
    node.write("manifest.jsonc", &manifest("@acme/node", "1.2.0", &[], ""));
    built_and_committed(&fixture, &node, "1.2.0");

    let err = stderr(&fails(fixture.publish(&node)));
    assert!(
        err.contains("the registry would not index the new version"),
        "{err}"
    );
    assert!(err.contains("v1.0.0"), "{err}");
    assert!(err.contains("restore the tag where it was"), "{err}");
    assert_eq!(node.tags(), ["v1.0.0", "v1.1.0"]);
}

#[test]
fn publish_never_pushes() {
    let fixture = Fixture::new("publish-no-push");
    let (_node, app) = app_on_node(&fixture, "^1");
    git(&fixture.dir, &["init", "-q", "--bare", "remote"]);
    let remote = fixture.dir.join("remote");
    app.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    built_and_committed(&fixture, &app, "first");

    succeeds(fixture.publish(&app));

    assert_eq!(app.tags(), ["v0.1.0"]);
    assert!(git(&remote, &["tag", "--list"]).is_empty());
    assert!(git(&remote, &["branch", "--list"]).is_empty());
}

#[test]
fn a_build_declares_the_oldest_assemblr_publish_requires_it_and_older_ones_skip_newer_units() {
    let fixture = Fixture::new("assemblr-field");
    let (node, app) = app_on_node(&fixture, "^1");

    // The build writes the field after `version`, and says so once.
    let output = succeeds(fixture.build(&app));
    assert!(
        stderr(&output).contains("note: set `assemblr` to 0.1.0-alpha.1 in manifest.jsonc"),
        "{}",
        stderr(&output)
    );
    let text = app.read("manifest.jsonc");
    let declared = text.find(r#""assemblr": "0.1.0-alpha.1""#).expect(&text);
    assert!(text.find(r#""version""#).unwrap() < declared, "{text}");
    let output = succeeds(fixture.build(&app));
    assert!(
        !stderr(&output).contains("`assemblr`"),
        "{}",
        stderr(&output)
    );

    // A commit that declares another version is not published.
    app.write(
        "manifest.jsonc",
        &app.read("manifest.jsonc")
            .replace(r#""assemblr": "0.1.0-alpha.1""#, r#""assemblr": "0.0.1""#),
    );
    app.commit("stale");
    let output = fails(fixture.publish(&app));
    assert!(
        errors(&output).iter().any(|line| line.contains(
            "the committed manifest.jsonc declares `assemblr` 0.0.1, but the oldest Assemblr \
             that reads the unit is 0.1.0-alpha.1"
        )),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("run `assemblr unit build`"),
        "{}",
        stderr(&output)
    );
    assert!(app.tags().is_empty());
    built_and_committed(&fixture, &app, "declared");
    succeeds(fixture.publish(&app));
    assert_eq!(app.tags(), ["v0.1.0"]);

    // A version that needs a newer Assemblr, with a field this one does not
    // know, is skipped with the reason, and the earlier versions stay.
    node.write(
        "manifest.jsonc",
        &manifest(
            "@acme/node",
            "2.0.0",
            &[],
            r#", "assemblr": "99.0.0", "hooks": {}"#,
        ),
    );
    node.publish_by_hand("2.0.0");
    let output = succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    assert!(
        stderr(&output).contains("needs Assemblr 99.0.0 or later"),
        "{}",
        stderr(&output)
    );
    assert_eq!(fixture.versions("@acme/node"), ["1.0.0", "1.1.0"]);
}
