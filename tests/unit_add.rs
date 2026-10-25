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

    /// A published `@acme/base` 1.0.0 whose `appName` is required with an
    /// example, `mode` is an enumerated string with an example, and `port`
    /// has a default.
    fn base(&self) -> Repo {
        let base = self.repo("base");
        base.write(
            "manifest.jsonc",
            &manifest(
                "@acme/base",
                "1.0.0",
                &[],
                r#", "params": {
                    "appName": { "type": "string", "description": "The app name.", "example": "demo" },
                    "mode": { "type": "string", "description": "Mode.", "allowed": ["dev", "prod"], "example": "dev" },
                    "port": { "type": "integer", "description": "Port.", "default": 8080 }
                }"#,
            ),
        );
        base.write(
            "src/cfg.txt.assemblr.tmpl",
            "name={{ appName }} mode={{ mode }} port={{ port }}\n",
        );
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
    assert!(
        out.contains("declared `appName`, `mode` in params and mapped them"),
        "{out}"
    );
    assert!(out.contains("Built @acme/app 0.1.0 into build/"), "{out}");
    assert!(
        stderr(&output).contains("created assemblr-lock.json: @acme/base 1.0.0 (new)"),
        "{}",
        stderr(&output)
    );
    // The manifest keeps its comment and holds the declaration, the mapped
    // parameters with the dependency's type, allowed values, and example,
    // and the mappings.
    let text = app.read("manifest.jsonc");
    assert!(text.contains("// The app."), "{text}");
    let manifest = app.manifest();
    assert_eq!(manifest["dependencies"]["@acme/base"], "^1.0.0");
    assert_eq!(manifest["params"]["appName"]["type"], "string");
    assert_eq!(manifest["params"]["appName"]["example"], "demo");
    assert_eq!(
        manifest["params"]["mode"]["allowed"],
        serde_json::json!(["dev", "prod"])
    );
    assert!(manifest["params"]["mode"].get("default").is_none());
    assert_eq!(
        manifest["options"]["@acme/base"]["appName"]["from"],
        "appName"
    );
    assert_eq!(manifest["options"]["@acme/base"]["mode"]["from"], "mode");
    assert!(manifest["params"].get("port").is_none(), "{manifest}");
    assert_eq!(app.read("build/cfg.txt"), "name=demo mode=dev port=8080\n");
    assert_eq!(app.lockfile()["units"]["@acme/base"]["version"], "1.0.0");

    // No separate build is needed before amending.
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "amend", "cfg.txt"]));
    assert!(
        stdout(&output).contains("created amendments/cfg.txt.assemblr.tmpl"),
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
fn a_required_parameter_the_unit_declares_already_is_mapped_from_it() {
    let fixture = Fixture::new("a_required_parameter_the_unit_declares_already_is_mapped_from_it");
    fixture.base();
    let app = fixture.app();
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[],
            r#", "params": {
                "appName": { "type": "string", "description": "Our name.", "example": "ours" },
                "mode": { "type": "string", "description": "Our mode.", "allowed": ["dev"], "example": "dev" }
            }"#,
        ),
    );
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "add", "@acme/base"]));
    assert!(
        stdout(&output).contains("mapped `appName`, `mode` from the unit's own parameters"),
        "{}",
        stdout(&output)
    );
    let manifest = app.manifest();
    assert_eq!(manifest["params"]["appName"]["description"], "Our name.");
    assert_eq!(
        manifest["options"]["@acme/base"]["appName"]["from"],
        "appName"
    );
    assert_eq!(app.read("build/cfg.txt"), "name=ours mode=dev port=8080\n");
}

#[test]
fn an_own_parameter_that_cannot_be_mapped_is_refused_with_the_option_as_the_way_out() {
    let fixture = Fixture::new(
        "an_own_parameter_that_cannot_be_mapped_is_refused_with_the_option_as_the_way_out",
    );
    fixture.base();
    let app = fixture.app();
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[],
            r#", "params": { "appName": { "type": "integer", "description": "A number.", "example": 1 } }"#,
        ),
    );
    let before = app.read("manifest.jsonc");
    let output = fails(fixture.assemblr(&app.path, &["unit", "add", "@acme/base"]));
    let err = stderr(&output);
    assert!(
        err.contains("the unit's own parameter `appName` cannot supply it"),
        "{err}"
    );
    assert!(err.contains("--option appName=<value>"), "{err}");
    assert_eq!(app.read("manifest.jsonc"), before);
    assert!(!app.exists("assemblr-lock.json"));

    let output = succeeds(fixture.assemblr(
        &app.path,
        &["unit", "add", "@acme/base", "--option", "appName=shop"],
    ));
    assert!(
        stdout(&output).contains("set `appName`"),
        "{}",
        stdout(&output)
    );
    let manifest = app.manifest();
    assert_eq!(manifest["options"]["@acme/base"]["appName"], "shop");
    assert_eq!(manifest["options"]["@acme/base"]["mode"]["from"], "mode");
    assert_eq!(manifest["params"]["appName"]["type"], "integer");
}

#[test]
fn a_required_parameter_without_an_example_is_refused_before_anything_is_written() {
    let fixture = Fixture::new(
        "a_required_parameter_without_an_example_is_refused_before_anything_is_written",
    );
    let plain = fixture.repo("plain");
    plain.write(
        "manifest.jsonc",
        &manifest(
            "@acme/plain",
            "1.0.0",
            &[],
            r#", "params": { "token": { "type": "string", "description": "A token." } }"#,
        ),
    );
    plain.write("src/t.txt.assemblr.tmpl", "{{ token }}\n");
    plain.publish("1.0.0");
    fixture.register(&plain);
    let app = fixture.app();
    let before = app.read("manifest.jsonc");
    let output = fails(fixture.assemblr(&app.path, &["unit", "add", "@acme/plain"]));
    let err = stderr(&output);
    assert!(
        err.contains("needs a value for `token` and declares no example"),
        "{err}"
    );
    assert!(err.contains("--option token=<value>"), "{err}");
    assert_eq!(app.read("manifest.jsonc"), before);
    assert!(!app.exists("assemblr-lock.json"));

    // A literal satisfies it, read by the parameter's type.
    succeeds(fixture.assemblr(
        &app.path,
        &["unit", "add", "@acme/plain", "--option", "token=true"],
    ));
    assert_eq!(app.manifest()["options"]["@acme/plain"]["token"], "true");
    assert_eq!(app.read("build/t.txt"), "true\n");
}

#[test]
fn a_literal_is_read_by_the_parameters_type_and_a_bad_one_writes_nothing() {
    let fixture =
        Fixture::new("a_literal_is_read_by_the_parameters_type_and_a_bad_one_writes_nothing");
    fixture.base();
    let app = fixture.app();
    let before = app.read("manifest.jsonc");
    let output = fails(fixture.assemblr(
        &app.path,
        &["unit", "add", "@acme/base", "--option", "mode=staging"],
    ));
    assert!(
        stderr(&output).contains("not one of the allowed values"),
        "{}",
        stderr(&output)
    );
    assert_eq!(app.read("manifest.jsonc"), before);
    assert!(!app.exists("build"));

    succeeds(fixture.assemblr(
        &app.path,
        &[
            "unit",
            "add",
            "@acme/base",
            "--option",
            "port=9000",
            "--option",
            "mode=prod",
        ],
    ));
    let manifest = app.manifest();
    assert_eq!(manifest["options"]["@acme/base"]["port"], 9000);
    assert_eq!(manifest["options"]["@acme/base"]["mode"], "prod");
    assert!(manifest["params"].get("mode").is_none(), "{manifest}");
    assert_eq!(app.read("build/cfg.txt"), "name=demo mode=prod port=9000\n");
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
fn a_unit_with_params_after_its_description_takes_a_dependency() {
    // `unit init` writes `description` last; parameters added below it used
    // to make every `unit add` refuse the edit (#120).
    let fixture = Fixture::new("a_unit_with_params_after_its_description_takes_a_dependency");
    fixture.base();
    let app = fixture.app();
    app.write(
        "manifest.jsonc",
        "{\n  \"name\": \"@acme/app\",\n  \"version\": \"0.1.0\",\n  \"description\": \
         \"The app.\",\n  \"params\": { \"title\": { \"type\": \"string\", \"description\": \
         \"The title.\", \"example\": \"Demo\" } }\n}\n",
    );
    app.write("src/title.txt.assemblr.tmpl", "{{ title }}\n");
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "add", "@acme/base"]));
    assert!(
        stdout(&output).contains("declared `appName`, `mode` in params"),
        "{}",
        stdout(&output)
    );
    let manifest = app.manifest();
    assert_eq!(manifest["dependencies"]["@acme/base"], "^1.0.0");
    assert_eq!(manifest["params"]["title"]["example"], "Demo");
    assert_eq!(manifest["params"]["appName"]["example"], "demo");
    // The one-line `params` object stays on one line.
    assert!(
        app.read("manifest.jsonc").contains(
            "\"example\": \"Demo\" }, \"appName\": { \"type\": \"string\", \"description\": \
             \"The app name.\", \"example\": \"demo\" }"
        ),
        "{}",
        app.read("manifest.jsonc")
    );
    assert_eq!(app.read("build/title.txt"), "Demo\n");
}

#[test]
fn templates_written_first_name_every_parameter_to_declare() {
    let fixture = Fixture::new("templates_written_first_name_every_parameter_to_declare");
    fixture.base();
    let app = fixture.app();
    app.write(
        "src/page.txt.assemblr.tmpl",
        "{{ title }} {{ slug }} {{ title }}\n",
    );
    let before = app.read("manifest.jsonc");
    let output = fails(fixture.assemblr(&app.path, &["unit", "add", "@acme/base"]));
    assert!(
        stderr(&output).contains(
            "hint: declare `title`, `slug` under `params` in manifest.jsonc, each like \
             `\"title\": { \"type\": \"string\", \"description\": \"...\", \"example\": \"...\" }`"
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(app.read("manifest.jsonc"), before);
    let output = fails(fixture.assemblr(&app.path, &["unit", "build"]));
    assert!(
        stderr(&output).contains("hint: declare `title`, `slug` under `params`"),
        "{}",
        stderr(&output)
    );

    // Declared after `description`, as an author adds them, they let the
    // dependency be added.
    app.write(
        "manifest.jsonc",
        &before.replace(
            "\"The app.\"\n}",
            "\"The app.\",\n  \"params\": {\n    \"title\": { \"type\": \"string\", \
             \"description\": \"T.\", \"example\": \"T\" },\n    \"slug\": { \"type\": \
             \"string\", \"description\": \"S.\", \"example\": \"s\" }\n  }\n}",
        ),
    );
    succeeds(fixture.assemblr(&app.path, &["unit", "add", "@acme/base"]));
    assert_eq!(app.read("build/page.txt"), "T s T\n");
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
    assert!(text.contains("\"example\": \"demo\",\n    },"), "{text}");
    assert!(text.contains("\"from\": \"appName\",\n      },"), "{text}");
}
