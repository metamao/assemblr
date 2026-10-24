//! The reference scenario: real unit authoring, publication, composition,
//! updates, and reconciliation, end to end through the binary.
//!
//! It follows the shape of the reference-scenario validation (#50, EXP-12 in
//! the `system-design` research repository) with small stand-in files: an
//! application unit that requires a parameter, and three units that amend
//! it, two of them the same text file. A consumer composes them, edits the
//! result, takes two provider releases (one merging cleanly, one stopping on
//! a conflict it resolves), removes a unit, and composes again. Every step
//! is a real command; nothing is published or composed by hand.
//!
//! It runs offline in a few seconds, like the rest of the suite. Running the
//! composed project's own toolchains (npm, Angular, Wrangler, Docker) is not
//! part of it: EXP-12 does that with its full units.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a fixture that cannot be built fails the test"
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::{Fixture, fails, git, stderr, stdout, succeeds};

impl Fixture {
    fn assemblr(&self, dir: &Path, args: &[&str]) -> Output {
        self.command(dir, args).output().unwrap()
    }

    /// Runs `assemblr` in `dir`, which must succeed, and returns what it
    /// printed, standard output then standard error.
    fn run(&self, dir: &Path, args: &[&str]) -> String {
        let output = succeeds(self.assemblr(dir, args));
        stdout(&output) + &stderr(&output)
    }
}

const APP_PARAM: &str = r#""params": {"appName": {"type": "string", "description": "The application name.", "example": "demo"}}"#;

/// A unit's manifest at `version`, depending on `@ref/app` and passing its
/// own `appName` on to it, when `dependent`.
fn manifest(name: &str, version: &str, description: &str, dependent: bool) -> String {
    let dependency = if dependent {
        r#", "dependencies": {"@ref/app": "^1.0.0"}, "options": {"@ref/app": {"appName": {"from": "appName"}}}"#
    } else {
        ""
    };
    format!(
        "{{\n  \"$schema\": \"./manifest.schema.json\", \"name\": \"{name}\", \
         \"version\": \"{version}\", \"description\": \"{description}\",\n  \
         {APP_PARAM}{dependency}\n}}\n"
    )
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

/// Replaces the one occurrence of `old` in the file at `path`.
fn edit(path: &Path, old: &str, new: &str) {
    let text = read(path);
    assert_eq!(text.matches(old).count(), 1, "{old:?} in {text}");
    fs::write(path, text.replace(old, new)).unwrap();
}

/// Every file of a consumer project but its stored state, by `/`-separated
/// path.
fn files(project: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut pending = vec![project.to_owned()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|name| name == ".assemblr") {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path.strip_prefix(project).unwrap();
                let relative = relative.to_string_lossy().replace('\\', "/");
                found.push((relative, read(&path)));
            }
        }
    }
    found.sort();
    found
}

/// Authors a unit the way its author would: `unit init`, the manifest and
/// `src/`, a build that writes the lock, then `amend` with each change
/// applied to the copy, then a check, a commit, registration, and
/// publication.
struct Author<'a> {
    fixture: &'a Fixture,
    dir: PathBuf,
}

impl<'a> Author<'a> {
    fn init(fixture: &'a Fixture, identity: &str, name: &str, manifest: &str) -> Self {
        fixture.run(&fixture.dir, &["unit", "init", identity, name]);
        let dir = fixture.dir.join(name);
        write(&dir.join("manifest.jsonc"), manifest);
        Self { fixture, dir }
    }

    fn src(&self, path: &str, contents: &str) -> &Self {
        write(&self.dir.join("src").join(path), contents);
        self
    }

    fn build(&self) -> &Self {
        self.fixture.run(&self.dir, &["unit", "build"]);
        self
    }

    /// Copies `file` from the dependency with `unit amend` and replaces
    /// `old` with `new` in the copy at `copy`.
    fn amend(&self, file: &str, copy: &str, old: &str, new: &str) -> &Self {
        self.fixture.run(&self.dir, &["unit", "amend", file]);
        edit(&self.dir.join("amendments").join(copy), old, new);
        self
    }

    fn publish(&self, message: &str) -> String {
        self.build();
        self.fixture.run(&self.dir, &["unit", "check"]);
        git(&self.dir, &["add", "-A"]);
        git(&self.dir, &["commit", "-q", "-m", message]);
        // Registering a registered repository again changes nothing.
        self.fixture.run(&self.dir, &["registry", "add", "."]);
        self.fixture.run(&self.dir, &["unit", "publish"])
    }

    /// A new version: the manifest's version changed and published.
    fn release(&self, from: &str, to: &str) -> String {
        edit(
            &self.dir.join("manifest.jsonc"),
            &format!("\"version\": \"{from}\""),
            &format!("\"version\": \"{to}\""),
        );
        self.publish(to)
    }
}

const PACKAGE: &str = "{\n  \"name\": \"{{ appName }}\",\n  \"private\": true,\n  \"scripts\": {\n    \"build\": \"ng build\"\n  },\n  \"dependencies\": {\n    \"@angular/core\": \"^20\"\n  }\n}\n";
const AGENTS: &str =
    "# Agent guidance\n\nRead the README first.\n\n## Rules\n\n- Keep changes small.\n";
const README: &str = "# {{ appName }}\n\nAn application.\n\nRun `npm start`.\n";

/// The four units of the scenario, published and registered: `@ref/app`,
/// and `@ref/worker`, `@ref/docker`, and `@ref/neon`, which amend it.
fn publish_units(fixture: &Fixture) -> Author<'_> {
    let app = Author::init(
        fixture,
        "@ref/app",
        "app",
        &manifest("@ref/app", "1.0.0", "An application skeleton.", false),
    );
    app.src("package.json.assemblr.tmpl", PACKAGE)
        .src("AGENTS.md", AGENTS)
        .src("README.md.assemblr.tmpl", README)
        .build();
    app.publish("app 1.0.0");

    let worker = Author::init(
        fixture,
        "@ref/worker",
        "worker",
        &manifest("@ref/worker", "1.0.0", "Deploys to a worker.", true),
    );
    worker
        .build()
        .amend(
            "package.json",
            "package.json.assemblr.tmpl",
            "\"build\": \"ng build\"",
            "\"build\": \"ng build\",\n    \"deploy\": \"wrangler deploy\"",
        )
        .src(
            "wrangler.jsonc.assemblr.tmpl",
            "{\n  // The worker.\n  \"name\": \"{{ appName }}\",\n  \"main\": \"dist/server.mjs\"\n}\n",
        );
    worker.publish("worker 1.0.0");

    let docker = Author::init(
        fixture,
        "@ref/docker",
        "docker",
        &manifest("@ref/docker", "1.0.0", "A development container.", true),
    );
    docker
        .build()
        .amend(
            "AGENTS.md",
            "AGENTS.md",
            "- Keep changes small.\n",
            "- Keep changes small.\n\n## Dev container\n\n- Run everything inside the container.\n",
        )
        .src("compose.yaml", "services:\n  app:\n    build: .\n");
    docker.publish("docker 1.0.0");

    let neon = Author::init(
        fixture,
        "@ref/neon",
        "neon",
        &manifest("@ref/neon", "1.0.0", "A Postgres database.", true),
    );
    neon.build()
        .amend(
            "AGENTS.md",
            "AGENTS.md",
            "- Keep changes small.\n",
            "- Keep changes small.\n\n## Database\n\n- Never commit credentials.\n",
        )
        .amend(
            "package.json",
            "package.json.assemblr.tmpl",
            "\"@angular/core\": \"^20\"",
            "\"@angular/core\": \"^20\",\n    \"@neondatabase/serverless\": \"^1\"",
        )
        .src(".env.example", "DATABASE_URL=\n");
    neon.publish("neon 1.0.0");

    app
}

const DECLARATION: &str = r#"{
  "dependencies": {"@ref/worker": "^1.0.0", "@ref/docker": "^1.0.0", "@ref/neon": "^1.0.0"},
  "options": {
    "@ref/worker": {"appName": "shop"},
    "@ref/docker": {"appName": "shop"},
    "@ref/neon": {"appName": "shop"}
  }
}
"#;

/// A consumer project named `name` declaring the three amending units.
fn project(fixture: &Fixture, name: &str) -> PathBuf {
    fixture.run(&fixture.dir, &["init", name]);
    let project = fixture.dir.join(name);
    // The units deliver their own AGENTS.md; the one `init` writes, which
    // would be a conflict to resolve first, is covered in tests/compose.rs.
    fs::remove_file(project.join("AGENTS.md")).unwrap();
    write(&project.join("assemblr.jsonc"), DECLARATION);
    project
}

#[test]
fn units_are_authored_published_composed_updated_and_reconciled() {
    let fixture = Fixture::new("reference");
    let app = publish_units(&fixture);
    let listing = fixture.run(&fixture.dir, &["registry", "list"]);
    for unit in ["@ref/app", "@ref/docker", "@ref/neon", "@ref/worker"] {
        assert!(listing.contains(unit), "{listing}");
    }

    // The first composition: one value of `appName` reaches every unit, two
    // units amend `package.json` value by value, and two add a section to
    // `AGENTS.md` at one place, kept in the order of their identities.
    let shop = project(&fixture, "shop");
    let out = fixture.run(&shop, &["compose"]);
    assert!(
        out.contains("@ref/docker, @ref/neon each add lines"),
        "{out}"
    );
    let composed = files(&shop);
    let names: Vec<&str> = composed.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        names,
        [
            ".env.example",
            "AGENTS.md",
            "README.md",
            "assemblr-lock.json",
            "assemblr.jsonc",
            "compose.yaml",
            "package.json",
            "wrangler.jsonc",
        ]
    );
    let package = read(&shop.join("package.json"));
    assert!(package.contains("\"name\": \"shop\""), "{package}");
    assert!(
        package.contains("\"deploy\": \"wrangler deploy\""),
        "{package}"
    );
    assert!(
        package.contains("\"@neondatabase/serverless\": \"^1\""),
        "{package}"
    );
    let agents = read(&shop.join("AGENTS.md"));
    let container = agents.find("## Dev container").unwrap();
    let database = agents.find("## Database").unwrap();
    assert!(container < database, "{agents}");
    assert!(read(&shop.join("wrangler.jsonc")).contains("\"name\": \"shop\""));

    // The same declaration composes to the same files elsewhere.
    let again = project(&fixture, "again");
    fixture.run(&again, &["compose"]);
    assert_eq!(files(&again), composed);

    // The consumer edits two files, and the provider releases changes to
    // both. `update` moves the provider and merges each file: the amenders
    // are not released again.
    edit(
        &shop.join("package.json"),
        "\"deploy\": \"wrangler deploy\"",
        "\"deploy\": \"wrangler deploy\",\n    \"test\": \"vitest\"",
    );
    edit(
        &shop.join("README.md"),
        "Run `npm start`.\n",
        "Run `npm start`.\n\nSee docs/ for more.\n",
    );
    edit(
        &app.dir.join("src/README.md.assemblr.tmpl"),
        "An application.",
        "An Angular application.",
    );
    edit(
        &app.dir.join("src/package.json.assemblr.tmpl"),
        "\"@angular/core\": \"^20\"",
        "\"@angular/core\": \"^20\",\n    \"@angular/router\": \"^20\"",
    );
    app.release("1.0.0", "1.1.0");
    let out = fixture.run(&shop, &["update"]);
    assert!(out.contains("@ref/app 1.0.0 -> 1.1.0"), "{out}");
    assert!(out.contains("merged README.md"), "{out}");
    assert!(out.contains("merged package.json"), "{out}");
    let package = read(&shop.join("package.json"));
    assert!(package.contains("\"test\": \"vitest\""), "{package}");
    assert!(
        package.contains("\"@angular/router\": \"^20\""),
        "{package}"
    );
    assert!(
        package.contains("\"@neondatabase/serverless\""),
        "{package}"
    );
    let readme = read(&shop.join("README.md"));
    assert!(readme.contains("An Angular application."), "{readme}");
    assert!(readme.contains("See docs/ for more."), "{readme}");

    // Both sides change one line: `update` stops with the line marked and
    // the lock waiting, and nothing else runs until the consumer settles it.
    edit(
        &shop.join("README.md"),
        "Run `npm start`.",
        "Run `npm start` in the container.",
    );
    edit(
        &app.dir.join("src/README.md.assemblr.tmpl"),
        "Run `npm start`.",
        "Run `npm run dev`.",
    );
    app.release("1.1.0", "1.2.0");
    let err = stderr(&fails(fixture.assemblr(&shop, &["update"])));
    assert!(err.contains("compose --continue"), "{err}");
    let marked = read(&shop.join("README.md"));
    assert!(marked.contains("<<<<<<< current"), "{marked}");
    assert!(marked.contains("Run `npm run dev`."), "{marked}");
    let err = stderr(&fails(fixture.assemblr(&shop, &["compose"])));
    assert!(err.contains("README.md"), "{err}");
    let start = marked.find("<<<<<<< current").unwrap();
    let end = marked.find(">>>>>>> incoming").unwrap();
    let end = end + marked[end..].find('\n').unwrap() + 1;
    let resolved = format!(
        "{}Run `npm run dev` in the container.\n\nSee docs/ for more.\n{}",
        &marked[..start],
        &marked[end..]
    );
    fs::write(shop.join("README.md"), &resolved).unwrap();
    let out = fixture.run(&shop, &["compose", "--continue"]);
    assert!(out.contains("resolved README.md"), "{out}");
    assert_eq!(read(&shop.join("README.md")), resolved);
    let lock = read(&shop.join("assemblr-lock.json"));
    assert!(lock.contains("\"version\": \"1.2.0\""), "{lock}");

    // Removing a unit takes its file and its section away, and keeps the
    // consumer's edits and the other amender's section.
    let out = fixture.run(&shop, &["remove", "@ref/docker"]);
    assert!(out.contains("removed compose.yaml"), "{out}");
    assert!(!shop.join("compose.yaml").exists());
    let agents = read(&shop.join("AGENTS.md"));
    assert!(!agents.contains("## Dev container"), "{agents}");
    assert!(agents.contains("## Database"), "{agents}");
    assert_eq!(read(&shop.join("README.md")), resolved);
    let declaration = read(&shop.join("assemblr.jsonc"));
    assert!(!declaration.contains("@ref/docker"), "{declaration}");

    // Composing again changes nothing.
    let before = files(&shop);
    let out = fixture.run(&shop, &["compose"]);
    assert!(out.contains("Nothing to compose"), "{out}");
    assert_eq!(files(&shop), before);
}
