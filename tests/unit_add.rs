//! `assemblr unit add`, run as a real process against unit repositories
//! and a registry built in temporary directories.
//!
//! Every test gives the binary its own registry and home.

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
    fn repo(&self, name: &str) -> Repo {
        git(&self.dir, &["init", "-q", name]);
        Repo {
            path: self.dir.join(name),
        }
    }

    fn assemblr(&self, dir: &Path, args: &[&str]) -> Output {
        self.command(dir, args).output().unwrap()
    }

    fn register(&self, repo: &Repo) {
        let path = repo.path.to_str().unwrap();
        succeeds(self.assemblr(&self.dir, &["registry", "add", path]));
    }

    /// A published `@acme/base` 1.0.0 whose `cfg.txt` holds the placeholder
    /// `__APP_NAME__`.
    fn base(&self) -> Repo {
        let base = self.repo("base");
        base.write(
            "manifest.jsonc",
            &manifest(
                "@acme/base",
                "1.0.0",
                &[],
                r#", "placeholders": {"replace": {"__APP_NAME__": "The app's name."}}"#,
            ),
        );
        base.write("src/cfg.txt", "name=__APP_NAME__ port=8080\n");
        base.publish("1.0.0");
        self.register(&base);
        base
    }

    /// An unpublished, unbuilt `@acme/app` with no dependencies.
    fn app(&self) -> Repo {
        let app = self.repo("app");
        app.write(
            "manifest.jsonc",
            "{\n  // The app.\n  \"name\": \"@acme/app\",\n  \"version\": \"0.1.0\",\n  \
             \"description\": \"The app.\"\n}\n",
        );
        app.write("src/app.txt", "app\n");
        app
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

    fn exists(&self, path: &str) -> bool {
        self.path.join(path).exists()
    }

    /// Commits everything and tags it `v<version>`.
    fn publish(&self, version: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", version]);
        self.git(&["tag", &format!("v{version}")]);
    }

    fn manifest(&self) -> Value {
        let text = self.read("manifest.jsonc");
        let stripped: String = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        serde_json::from_str(&stripped).unwrap()
    }

    fn lockfile(&self) -> Value {
        serde_json::from_str(&self.read("assemblr-lock.json")).unwrap()
    }
}

#[test]
fn add_declares_the_dependency_builds_and_lets_amend_follow() {
    let fixture = Fixture::new("add_declares_the_dependency_builds_and_lets_amend_follow");
    fixture.base();
    let app = fixture.app();

    let output = succeeds(fixture.assemblr(&app.path, &["unit", "add", "@acme/base"]));
    let out = stdout(&output);
    assert!(
        out.contains("Added @acme/base (^1.0.0) to manifest.jsonc"),
        "{out}"
    );
    assert!(out.contains("Built @acme/app 0.1.0 into build/"), "{out}");
    assert!(
        out.contains("Placeholders: __APP_NAME__ in 1 file."),
        "{out}"
    );
    assert!(
        stderr(&output).contains("created assemblr-lock.json: @acme/base 1.0.0 (new)"),
        "{}",
        stderr(&output)
    );
    // The manifest keeps its comment and holds the declaration only: the
    // dependency's placeholder stays the dependency's, declared once.
    let text = app.read("manifest.jsonc");
    assert!(text.contains("// The app."), "{text}");
    let manifest = app.manifest();
    assert_eq!(manifest["dependencies"]["@acme/base"], "^1.0.0");
    assert!(manifest.get("placeholders").is_none(), "{manifest}");
    assert_eq!(app.read("build/cfg.txt"), "name=__APP_NAME__ port=8080\n");
    assert_eq!(app.lockfile()["units"]["@acme/base"]["version"], "1.0.0");

    // No separate build is needed before amending.
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "amend", "cfg.txt"]));
    assert!(
        stdout(&output).contains("created amendments/cfg.txt"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn add_to_a_built_unit_updates_the_lock_and_keeps_an_existing_requirement() {
    let fixture =
        Fixture::new("add_to_a_built_unit_updates_the_lock_and_keeps_an_existing_requirement");
    fixture.base();
    let tool = fixture.repo("tool");
    tool.write("manifest.jsonc", &manifest("@acme/tool", "2.0.0", &[], ""));
    tool.write("src/tool.txt", "tool\n");
    tool.publish("2.0.0");
    fixture.register(&tool);
    let app = fixture.app();
    succeeds(fixture.assemblr(&app.path, &["unit", "add", "@acme/tool@~2.0"]));
    assert_eq!(app.manifest()["dependencies"]["@acme/tool"], "~2.0");

    let output = succeeds(fixture.assemblr(&app.path, &["unit", "add", "@acme/base"]));
    assert!(
        stderr(&output).contains("updated assemblr-lock.json: @acme/base 1.0.0 (new)"),
        "{}",
        stderr(&output)
    );
    let manifest = app.manifest();
    assert_eq!(manifest["dependencies"]["@acme/tool"], "~2.0");
    assert_eq!(manifest["dependencies"]["@acme/base"], "^1.0.0");
    assert!(app.exists("build/tool.txt"));
    assert!(app.exists("build/cfg.txt"));

    // Declared already: the requirement stays unless another is given.
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "add", "@acme/tool"]));
    assert!(
        stdout(&output).contains("@acme/tool is already a dependency (~2.0)"),
        "{}",
        stdout(&output)
    );
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "add", "@acme/tool@2.0.0"]));
    assert!(
        stdout(&output).contains("Changed the requirement of @acme/tool from ~2.0 to ^2.0.0"),
        "{}",
        stdout(&output)
    );
    assert_eq!(app.manifest()["dependencies"]["@acme/tool"], "^2.0.0");
}

#[test]
fn an_invalid_requirement_is_refused_and_nothing_is_written() {
    let fixture = Fixture::new("an_invalid_requirement_is_refused_and_nothing_is_written");
    fixture.base();
    let app = fixture.app();
    let before = app.read("manifest.jsonc");
    let output = fails(fixture.assemblr(&app.path, &["unit", "add", "@acme/base@latest"]));
    let err = stderr(&output);
    assert!(err.contains("invalid requirement"), "{err}");
    assert!(err.contains("nothing was changed"), "{err}");
    assert_eq!(app.read("manifest.jsonc"), before);
    assert!(!app.exists("assemblr-lock.json"));
    assert!(!app.exists("build"));
}

#[test]
fn promoting_a_transitive_dependency_keeps_its_locked_version() {
    let fixture = Fixture::new("promoting_a_transitive_dependency_keeps_its_locked_version");
    let base = fixture.repo("base");
    base.write("manifest.jsonc", &manifest("@acme/base", "1.0.0", &[], ""));
    base.write("src/base.txt", "base 1.0.0\n");
    base.publish("1.0.0");
    fixture.register(&base);
    let mid = fixture.repo("mid");
    mid.write(
        "manifest.jsonc",
        &manifest("@acme/mid", "1.0.0", &[("@acme/base", "^1.0.0")], ""),
    );
    mid.write("src/mid.txt", "mid\n");
    mid.publish("1.0.0");
    fixture.register(&mid);
    let app = fixture.app();
    succeeds(fixture.assemblr(&app.path, &["unit", "add", "@acme/mid"]));
    assert_eq!(app.lockfile()["units"]["@acme/base"]["version"], "1.0.0");

    // A newer base is published, but the lock keeps 1.0.0.
    base.write("manifest.jsonc", &manifest("@acme/base", "1.1.0", &[], ""));
    base.write("src/base.txt", "base 1.1.0\n");
    base.publish("1.1.0");
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "add", "@acme/base"]));
    assert!(
        stdout(&output).contains("Added @acme/base (^1.0.0)"),
        "{}",
        stdout(&output)
    );
    assert!(
        !stderr(&output).contains("assemblr-lock.json"),
        "the lock must not change: {}",
        stderr(&output)
    );
    assert_eq!(app.lockfile()["units"]["@acme/base"]["version"], "1.0.0");
    assert_eq!(app.read("build/base.txt"), "base 1.0.0\n");
}

#[test]
fn the_consumer_add_in_a_unit_points_to_unit_add() {
    let fixture = Fixture::new("the_consumer_add_in_a_unit_points_to_unit_add");
    fixture.base();
    let app = fixture.app();
    let output = fails(fixture.assemblr(&app.path, &["add", "@acme/base"]));
    let err = stderr(&output);
    assert!(err.contains("is a unit's repository"), "{err}");
    assert!(err.contains("`assemblr unit add <UNIT>`"), "{err}");
    assert!(!err.contains("assemblr init` to create"), "{err}");
    assert!(!app.exists("assemblr.jsonc"));
}

#[test]
fn a_unit_cannot_add_itself_and_an_unknown_unit_leaves_the_manifest_alone() {
    let fixture =
        Fixture::new("a_unit_cannot_add_itself_and_an_unknown_unit_leaves_the_manifest_alone");
    let app = fixture.app();
    let before = app.read("manifest.jsonc");
    let output = fails(fixture.assemblr(&app.path, &["unit", "add", "@acme/app"]));
    assert!(
        stderr(&output).contains("cannot depend on itself"),
        "{}",
        stderr(&output)
    );
    let output = fails(fixture.assemblr(&app.path, &["unit", "add", "@acme/nowhere"]));
    let err = stderr(&output);
    assert!(err.contains("@acme/nowhere"), "{err}");
    assert!(err.contains("nothing was changed"), "{err}");
    assert_eq!(app.read("manifest.jsonc"), before);
    assert!(!app.exists("assemblr-lock.json"));
}

#[test]
fn a_requirement_spelled_differently_is_kept_as_written() {
    let fixture = Fixture::new("a_requirement_spelled_differently_is_kept_as_written");
    let tool = fixture.repo("tool");
    tool.write("manifest.jsonc", &manifest("@acme/tool", "1.0.0", &[], ""));
    tool.write("src/tool.txt", "tool\n");
    tool.publish("1.0.0");
    fixture.register(&tool);
    let app = fixture.app();
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[("@acme/tool", ">=1.0.0,<2.0.0")],
            "",
        ),
    );
    succeeds(fixture.assemblr(&app.path, &["unit", "build"]));
    let before = app.read("manifest.jsonc");
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "add", "@acme/tool"]));
    assert!(
        stdout(&output).contains("already a dependency (>=1.0.0,<2.0.0)"),
        "{}",
        stdout(&output)
    );
    assert_eq!(app.read("manifest.jsonc"), before);
}

#[test]
fn a_unit_with_placeholders_after_its_description_takes_a_dependency() {
    // `unit init` writes `description` last; fields added below it used to
    // make every `unit add` refuse the edit (#120).
    let fixture = Fixture::new("a_unit_with_placeholders_after_its_description_takes_a_dependency");
    fixture.base();
    let app = fixture.app();
    app.write(
        "manifest.jsonc",
        "{\n  \"name\": \"@acme/app\",\n  \"version\": \"0.1.0\",\n  \"description\": \
         \"The app.\",\n  \"placeholders\": { \"replace\": { \"__TITLE__\": \"The title.\" } }\n}\n",
    );
    app.write("src/title.txt", "__TITLE__\n");
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "add", "@acme/base"]));
    assert!(
        stdout(&output).contains("Placeholders: __APP_NAME__ in 1 file, __TITLE__ in 1 file."),
        "{}",
        stdout(&output)
    );
    let manifest = app.manifest();
    assert_eq!(manifest["dependencies"]["@acme/base"], "^1.0.0");
    // The one-line `placeholders` object stays as it was.
    assert!(
        app.read("manifest.jsonc")
            .contains("\"placeholders\": { \"replace\": { \"__TITLE__\": \"The title.\" } }"),
        "{}",
        app.read("manifest.jsonc")
    );
    assert_eq!(app.read("build/title.txt"), "__TITLE__\n");
}

#[test]
fn tokens_written_before_they_are_declared_are_a_warning_not_a_refusal() {
    let fixture =
        Fixture::new("tokens_written_before_they_are_declared_are_a_warning_not_a_refusal");
    fixture.base();
    let app = fixture.app();
    app.write("src/page.txt", "__TITLE__ __SLUG__\n__TITLE__\n");
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "add", "@acme/base"]));
    let err = stderr(&output);
    for expected in [
        "warning: 2 tokens look like placeholders but are not declared:",
        "  __SLUG__\n      src/page.txt:1\n",
        "  __TITLE__\n      src/page.txt:1\n      src/page.txt:2\n",
        "hint: add each one to \"placeholders.replace\" with a description, or to \
         \"placeholders.ignore\" if it is not a placeholder",
    ] {
        assert!(err.contains(expected), "{expected:?} in {err}");
    }
    assert_eq!(app.manifest()["dependencies"]["@acme/base"], "^1.0.0");
    assert_eq!(
        app.read("build/page.txt"),
        "__TITLE__ __SLUG__\n__TITLE__\n"
    );
    // The same tokens make `unit check` fail: a unit is not published with
    // them undeclared.
    let err = stderr(&fails(fixture.assemblr(&app.path, &["unit", "check"])));
    assert!(err.contains("__SLUG__"), "{err}");
}

#[test]
fn a_manifest_with_trailing_commas_keeps_them_in_what_add_writes() {
    // Prettier writes JSONC with trailing commas by default.
    let fixture = Fixture::new("a_manifest_with_trailing_commas_keeps_them_in_what_add_writes");
    fixture.base();
    let app = fixture.app();
    app.write(
        "manifest.jsonc",
        "{\n  \"name\": \"@acme/app\",\n  \"version\": \"0.1.0\",\n  \"description\": \
         \"The app.\",\n}\n",
    );
    succeeds(fixture.assemblr(&app.path, &["unit", "add", "@acme/base"]));
    let text = app.read("manifest.jsonc");
    assert!(
        text.contains("\"dependencies\": {\n    \"@acme/base\": \"^1.0.0\",\n  },"),
        "{text}"
    );
    assert!(text.ends_with("  },\n}\n"), "{text}");
}
