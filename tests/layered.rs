//! Layered amendments (#100), run through the real binary: a unit amends
//! any file in its dependency graph on the base its dependencies compose,
//! siblings that conflict leave a marked copy to the unit above, and
//! `unit update` reconciles copies with their layered bases.
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

    fn sync(&self) {
        succeeds(self.assemblr(&self.dir, &["registry", "sync"]));
    }

    fn build(&self, repo: &Repo) -> Output {
        self.assemblr(&repo.path, &["unit", "build"])
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
}

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

fn amending(path: &str, unit: &str, base: &str) -> String {
    format!(r#", "amendments": {{"{path}": {{"unit": "{unit}", "base": "{base}"}}}}"#)
}

const PACKAGE: &str = "{\n  \"name\": \"base\",\n  \"scripts\": {\n    \"dev\": \"wrangler dev\"\n  },\n  \
                       \"dependencies\": {\n    \"hono\": \"^4\"\n  }\n}\n";

/// `@acme/base` 1.0.0 with `package.json`, registered.
fn base(fixture: &Fixture) -> Repo {
    base_with(fixture, &[])
}

/// `@acme/base` 1.0.0 with `package.json` and `files`, registered.
fn base_with(fixture: &Fixture, files: &[(&str, &str)]) -> Repo {
    let base = fixture.repo("base");
    base.write("manifest.jsonc", &manifest("@acme/base", "1.0.0", &[], ""));
    base.write("src/package.json", PACKAGE);
    base.write("src/README.md", "# Base\n");
    for (path, contents) in files {
        base.write(&format!("src/{path}"), contents);
    }
    base.publish("1.0.0");
    fixture.register(&base);
    base
}

/// A published amender of `package.json` of `@acme/base` 1.0.0: `name` at
/// `version`, with `copy`, registered.
fn amender(fixture: &Fixture, name: &str, version: &str, copy: &str) -> Repo {
    let unit = fixture.repo(name);
    publish_amender(fixture, &unit, name, version, copy);
    fixture.register(&unit);
    unit
}

fn publish_amender(fixture: &Fixture, unit: &Repo, name: &str, version: &str, copy: &str) {
    unit.write(
        "manifest.jsonc",
        &manifest(
            &format!("@acme/{name}"),
            version,
            &[("@acme/base", "=1.0.0")],
            &amending("package.json", "@acme/base", "1.0.0"),
        ),
    );
    unit.write(&format!("src/{name}.txt"), name);
    unit.write("amendments/package.json", copy);
    succeeds(fixture.build(unit));
    unit.publish(version);
    fixture.sync();
}

fn with_dependency(name: &str, range: &str) -> String {
    PACKAGE.replace(
        "    \"hono\": \"^4\"\n",
        &format!("    \"{name}\": \"{range}\",\n    \"hono\": \"^4\"\n"),
    )
}

fn with_script(name: &str, command: &str) -> String {
    PACKAGE.replace(
        "    \"dev\": \"wrangler dev\"\n",
        &format!("    \"dev\": \"wrangler dev\",\n    \"{name}\": \"{command}\"\n"),
    )
}

fn hono(range: &str) -> String {
    PACKAGE.replace("\"hono\": \"^4\"", &format!("\"hono\": \"{range}\""))
}

/// An unpublished `@acme/app` depending on `dependencies`, with one file.
fn app(fixture: &Fixture, dependencies: &[(&str, &str)]) -> Repo {
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.1.0", dependencies, ""),
    );
    app.write("src/app.txt", "app");
    app
}

#[test]
fn a_transitive_file_is_amended_on_its_composed_base_and_a_value_below_can_be_overridden() {
    let fixture = Fixture::new("transitive");
    base(&fixture);
    amender(
        &fixture,
        "db",
        "1.0.0",
        &with_dependency("drizzle-orm", "^0.44"),
    );
    amender(&fixture, "api", "1.0.0", &with_script("test", "vitest"));
    let app = app(&fixture, &[("@acme/db", "^1"), ("@acme/api", "^1")]);
    succeeds(fixture.build(&app));
    let built = app.read("build/package.json");
    assert!(built.contains("\"drizzle-orm\": \"^0.44\""), "{built}");
    assert!(built.contains("\"test\": \"vitest\""), "{built}");

    // The provider is not a direct dependency; the copy is the composed
    // file, and the declaration names the provider and its locked version.
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "amend", "package.json"]));
    assert!(
        stdout(&output).contains(
            "The copy is composed from the amendments of @acme/api 1.0.0, @acme/db 1.0.0; a \
             change to what they added or changed overrides it in this unit."
        ),
        "{}",
        stdout(&output)
    );
    assert_eq!(app.read("amendments/package.json"), built);
    assert!(
        app.read("manifest.jsonc")
            .contains(r#""package.json": { "unit": "@acme/base", "base": "1.0.0" }"#),
        "{}",
        app.read("manifest.jsonc")
    );
    succeeds(fixture.assemblr(&app.path, &["unit", "check"]));

    // Overriding what a unit below added is an ordinary change.
    app.write(
        "amendments/package.json",
        &built
            .replace("\"drizzle-orm\": \"^0.44\"", "\"drizzle-orm\": \"^0.45\"")
            .replace("\"name\": \"base\"", "\"name\": \"app\""),
    );
    succeeds(fixture.build(&app));
    let built = app.read("build/package.json");
    assert!(built.contains("\"drizzle-orm\": \"^0.45\""), "{built}");
    assert!(built.contains("\"name\": \"app\""), "{built}");
    assert!(built.contains("\"test\": \"vitest\""), "{built}");
    assert_eq!(app.read("amendments/package.json"), built);
}

#[test]
fn a_published_amender_above_another_composes_on_top_of_it_in_dependents() {
    let fixture = Fixture::new("chain");
    base(&fixture);
    let db = amender(
        &fixture,
        "db",
        "1.0.0",
        &with_dependency("drizzle-orm", "^0.44"),
    );
    // `top` amends on top of `db`: it renames the package and keeps the
    // dependency `db` added.
    let top = fixture.repo("top");
    top.write(
        "manifest.jsonc",
        &manifest("@acme/top", "1.0.0", &[("@acme/db", "^1")], ""),
    );
    top.write("src/top.txt", "top");
    succeeds(fixture.build(&top));
    succeeds(fixture.assemblr(&top.path, &["unit", "amend", "package.json"]));
    let composed = top.read("amendments/package.json");
    assert!(
        composed.contains("\"drizzle-orm\": \"^0.44\""),
        "{composed}"
    );
    top.write(
        "amendments/package.json",
        &composed.replace("\"name\": \"base\"", "\"name\": \"top\""),
    );
    succeeds(fixture.build(&top));
    top.publish("1.0.0");
    fixture.register(&top);

    // A dependent that resolves the same versions gets `top`'s copy as it
    // is.
    let app = app(&fixture, &[("@acme/top", "^1")]);
    succeeds(fixture.build(&app));
    assert_eq!(
        app.read("build/package.json"),
        top.read("build/package.json")
    );

    // When `db` moves below `top`, `top`'s base is reconstructed at its
    // locked versions, and the change below flows through its copy.
    publish_amender(
        &fixture,
        &db,
        "db",
        "1.1.0",
        &with_dependency("drizzle-orm", "^0.45"),
    );
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "update"]));
    assert!(
        stdout(&output).contains("@acme/db 1.0.0 -> 1.1.0"),
        "{}",
        stdout(&output)
    );
    succeeds(fixture.build(&app));
    let built = app.read("build/package.json");
    assert!(built.contains("\"drizzle-orm\": \"^0.45\""), "{built}");
    assert!(built.contains("\"name\": \"top\""), "{built}");
}

#[test]
fn unit_add_of_a_conflicting_sibling_creates_the_marked_copy_and_the_unit_resolves_it() {
    let fixture = Fixture::new("add-sibling");
    base(&fixture);
    amender(&fixture, "api", "1.0.0", &hono("^4.1"));
    amender(&fixture, "db", "1.0.0", &hono("^4.2"));
    let app = app(&fixture, &[("@acme/api", "^1")]);
    succeeds(fixture.build(&app));
    let built_before = app.read("build/package.json");

    let output = fails(fixture.assemblr(&app.path, &["unit", "add", "@acme/db"]));
    let err = stderr(&output);
    for expected in [
        "amendments/package.json: the amendments of @acme/api and @acme/db of src/package.json \
         of @acme/base 1.0.0 conflict, and the copy holds each conflict between markers:",
        "/dependencies/hono: @acme/api changes \"^4\" to \"^4.1\"; @acme/db changes \"^4\" to \
         \"^4.2\"",
        "manifest.jsonc declares the copy, assemblr-lock.json was updated, and build/ was not \
         changed",
        "hint: edit the copy under amendments/ to the content you want, remove every conflict \
         marker, and run `assemblr unit build`",
    ] {
        assert!(err.contains(expected), "{expected:?} in {err}");
    }
    let copy = app.read("amendments/package.json");
    assert!(copy.contains("\"<<<<<<< conflict\""), "{copy}");
    assert!(copy.contains("\"@acme/api\": \"^4.1\""), "{copy}");
    assert!(copy.contains("\"@acme/db\": \"^4.2\""), "{copy}");
    let manifest = app.read("manifest.jsonc");
    assert!(manifest.contains("\"@acme/db\": \"^1.0.0\""), "{manifest}");
    assert!(
        manifest.contains(r#""package.json": { "unit": "@acme/base", "base": "1.0.0" }"#),
        "{manifest}"
    );
    assert!(app.read("assemblr-lock.json").contains("@acme/db"));
    assert_eq!(app.read("build/package.json"), built_before);

    // Nothing composes until the marker is gone.
    app.git(&["add", "-A"]);
    app.git(&["commit", "-q", "-m", "pending"]);
    for command in [
        &["unit", "build"][..],
        &["unit", "check"],
        &["unit", "publish"],
    ] {
        let err = stderr(&fails(fixture.assemblr(&app.path, command)));
        assert!(
            err.contains(
                "amendments/package.json still holds conflict markers; the build composes \
                 nothing until they are resolved"
            ),
            "{command:?}: {err}"
        );
    }
    assert_eq!(app.read("build/package.json"), built_before);
    // Another file can still be amended meanwhile.
    succeeds(fixture.assemblr(&app.path, &["unit", "amend", "README.md"]));
    assert_eq!(app.read("amendments/README.md"), "# Base\n");
    app.write("amendments/README.md", "# App\n");

    // Resolving is replacing the marker.
    let resolved = resolve_hono(&copy, "^4.2");
    app.write("amendments/package.json", &resolved);
    succeeds(fixture.build(&app));
    assert!(
        app.read("build/package.json")
            .contains("\"hono\": \"^4.2\"")
    );
    assert_eq!(app.read("build/README.md"), "# App\n");
    succeeds(fixture.assemblr(&app.path, &["unit", "check"]));
}

/// `copy` with the marker object at `/dependencies/hono` replaced by
/// `range`.
fn resolve_hono(copy: &str, range: &str) -> String {
    let start = copy.find("\"hono\": {").unwrap();
    let end = start + copy[start..].find("\n    }").unwrap() + "\n    }".len();
    format!("{}\"hono\": \"{range}\"{}", &copy[..start], &copy[end..])
}

#[test]
fn an_update_rebases_the_copy_on_the_base_below_and_marks_what_both_changed() {
    let fixture = Fixture::new("update-below");
    base(&fixture);
    let db = amender(
        &fixture,
        "db",
        "1.0.0",
        &with_dependency("drizzle-orm", "^0.44"),
    );
    let app = app(&fixture, &[("@acme/db", "^1")]);
    succeeds(fixture.build(&app));
    succeeds(fixture.assemblr(&app.path, &["unit", "amend", "package.json"]));
    let composed = app.read("amendments/package.json");
    app.write(
        "amendments/package.json",
        &composed.replace("\"dev\": \"wrangler dev\"", "\"dev\": \"app dev\""),
    );
    succeeds(fixture.build(&app));

    // A change below that the copy did not touch flows through.
    publish_amender(
        &fixture,
        &db,
        "db",
        "1.1.0",
        &with_dependency("drizzle-orm", "^0.45"),
    );
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "update"]));
    assert!(
        stdout(&output).contains(
            "Updated assemblr-lock.json: @acme/db 1.0.0 -> 1.1.0. Rewritten to follow the \
             changes below: amendments/package.json."
        ),
        "{}",
        stdout(&output)
    );
    let copy = app.read("amendments/package.json");
    assert!(copy.contains("\"drizzle-orm\": \"^0.45\""), "{copy}");
    assert!(copy.contains("\"dev\": \"app dev\""), "{copy}");
    // The provider did not move, so the declared base stands.
    assert!(app.read("manifest.jsonc").contains("\"base\": \"1.0.0\""));
    succeeds(fixture.build(&app));
    assert_eq!(app.read("build/package.json"), copy);

    // A change below to a value the copy changed too is marked, not
    // applied silently.
    publish_amender(
        &fixture,
        &db,
        "db",
        "1.2.0",
        &with_dependency("drizzle-orm", "^0.45").replace("\"wrangler dev\"", "\"db dev\""),
    );
    let output = fails(fixture.assemblr(&app.path, &["unit", "update"]));
    assert!(
        stdout(&output).contains("@acme/db 1.1.0 -> 1.2.0"),
        "{}",
        stdout(&output)
    );
    let err = stderr(&output);
    for expected in [
        "error: amendments/package.json: the copy and the update change the same parts of the \
         file, each left between markers:",
        "error:   /scripts/dev: the update changes \"wrangler dev\" to \"db dev\"; the copy \
         changes \"wrangler dev\" to \"app dev\"",
        "note: the update was applied; build/ was not changed",
        "hint: edit the copy under amendments/ to the content you want, remove every conflict \
         marker, and run `assemblr unit build`",
    ] {
        assert!(err.contains(expected), "{expected:?} in {err}");
    }
    let copy = app.read("amendments/package.json");
    assert!(copy.contains("\"<<<<<<< conflict\""), "{copy}");
    assert!(copy.contains("\"current\": \"app dev\""), "{copy}");
    assert!(copy.contains("\"incoming\": \"db dev\""), "{copy}");
    assert!(app.read("assemblr-lock.json").contains("\"1.2.0\""));
    assert!(!app.exists(".git/assemblr-update.json"));
    let err = stderr(&fails(fixture.build(&app)));
    assert!(
        err.contains("amendments/package.json still holds conflict markers"),
        "{err}"
    );
    // The same update again has nothing to do; the copy is the author's.
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "update"]));
    assert!(
        stdout(&output).contains("Nothing to update"),
        "{}",
        stdout(&output)
    );
    assert_eq!(app.read("amendments/package.json"), copy);
    let start = copy.find("\"dev\": {").unwrap();
    let end = start + copy[start..].find("\n    }").unwrap() + "\n    }".len();
    app.write(
        "amendments/package.json",
        &format!(
            "{}\"dev\": \"app and db dev\"{}",
            &copy[..start],
            &copy[end..]
        ),
    );
    succeeds(fixture.build(&app));
    assert!(
        app.read("build/package.json")
            .contains("\"dev\": \"app and db dev\"")
    );
}

#[test]
fn an_update_that_brings_conflicting_siblings_creates_the_marked_copy() {
    let fixture = Fixture::new("update-siblings");
    base(&fixture);
    amender(&fixture, "api", "1.0.0", &hono("^4.1"));
    let db = amender(
        &fixture,
        "db",
        "1.0.0",
        &with_dependency("drizzle-orm", "^0.44"),
    );
    let app = app(&fixture, &[("@acme/api", "^1"), ("@acme/db", "^1")]);
    succeeds(fixture.build(&app));
    let built_before = app.read("build/package.json");

    publish_amender(&fixture, &db, "db", "1.1.0", &hono("^4.2"));
    let output = fails(fixture.assemblr(&app.path, &["unit", "update"]));
    assert!(
        stdout(&output).contains(
            "Updated assemblr-lock.json: @acme/db 1.0.0 -> 1.1.0. Created, declared in \
             manifest.jsonc: amendments/package.json."
        ),
        "{}",
        stdout(&output)
    );
    let err = stderr(&output);
    for expected in [
        "error: amendments/package.json: the amendments of @acme/api and @acme/db of \
         src/package.json of @acme/base 1.0.0 conflict, and the copy holds each conflict between \
         markers:",
        "error:   /dependencies/hono: @acme/api changes \"^4\" to \"^4.1\"; @acme/db changes \
         \"^4\" to \"^4.2\"",
        "note: the update was applied; build/ was not changed",
    ] {
        assert!(err.contains(expected), "{expected:?} in {err}");
    }
    let copy = app.read("amendments/package.json");
    assert!(copy.contains("\"<<<<<<< conflict\""), "{copy}");
    assert!(
        app.read("manifest.jsonc")
            .contains(r#""package.json": { "unit": "@acme/base", "base": "1.0.0" }"#)
    );
    assert!(app.read("assemblr-lock.json").contains("\"1.1.0\""));
    assert_eq!(app.read("build/package.json"), built_before);
    assert!(!app.exists(".git/assemblr-update.json"));
    app.write("amendments/package.json", &resolve_hono(&copy, "^4.2"));
    succeeds(fixture.build(&app));
    assert!(
        app.read("build/package.json")
            .contains("\"hono\": \"^4.2\"")
    );
}

/// A consumer project `name` declaring `dependencies`, with a Git
/// repository.
fn project(fixture: &Fixture, name: &str, dependencies: &[(&str, &str)]) -> PathBuf {
    let project = fixture.dir.join(name);
    succeeds(fixture.assemblr(&fixture.dir, &["init", name]));
    git(&project, &["init", "-q"]);
    declare(&project, dependencies);
    project
}

fn declare(project: &Path, dependencies: &[(&str, &str)]) {
    let dependencies: Vec<String> = dependencies
        .iter()
        .map(|(identity, range)| format!("\"{identity}\": \"{range}\""))
        .collect();
    fs::write(
        project.join("assemblr.jsonc"),
        format!(
            "{{\n  \"dependencies\": {{{}}}\n}}\n",
            dependencies.join(", ")
        ),
    )
    .unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

/// Every file under `dir`, by `/`-separated path, with its bytes, `.git`
/// excepted.
fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(next).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
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
    found.sort();
    found
}

fn locked_units(project: &Path) -> Vec<String> {
    let lock: serde_json::Value =
        serde_json::from_str(&read(&project.join("assemblr-lock.json"))).unwrap();
    lock["units"].as_object().unwrap().keys().cloned().collect()
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Publishes `@acme/db` at `version` amending `package.json` with `copy`
/// and delivering `db.txt` holding `text`.
fn publish_db(fixture: &Fixture, db: &Repo, version: &str, copy: &str, text: &str) {
    db.write(
        "manifest.jsonc",
        &manifest(
            "@acme/db",
            version,
            &[("@acme/base", "=1.0.0")],
            &amending("package.json", "@acme/base", "1.0.0"),
        ),
    );
    db.write("src/db.txt", text);
    db.write("amendments/package.json", copy);
    succeeds(fixture.build(db));
    db.publish(version);
    fixture.sync();
}

const UNIT_CONFLICT: &str = "src/package.json of @acme/base 1.0.0: the amendments of @acme/api \
                             and @acme/db conflict, and no unit above them resolves the \
                             conflict:";

#[test]
fn a_consumer_resolves_a_conflict_between_siblings_and_the_resolution_is_kept() {
    let fixture = Fixture::new("consumer");
    base(&fixture);
    amender(&fixture, "api", "1.0.0", &hono("^4.1"));
    let db = amender(&fixture, "db", "1.0.0", &hono("^4.2"));
    let project = project(&fixture, "shop", &[("@acme/api", "^1")]);
    succeeds(fixture.assemblr(&project, &["compose"]));
    let declaration = read(&project.join("assemblr.jsonc"));

    // The sibling's conflict is left in the file, the rest is applied,
    // and the composition waits.
    let output = fails(fixture.assemblr(&project, &["add", "@acme/db"]));
    let out = stdout(&output);
    for expected in [
        "Selected @acme/db (^1.0.0) in assemblr.jsonc.",
        "but 1 file has conflicts; the rest is applied, and the lock is written when you finish:",
        "  conflict package.json",
        "  created db.txt",
        UNIT_CONFLICT,
        "  /dependencies/hono: @acme/api changes \"^4\" to \"^4.1\"; @acme/db changes \"^4\" to \
         \"^4.2\"",
        "A conflict between units shows each unit's content between <<<<<<< and >>>>>>> lines \
         that name the units, or, in a JSON file, as a \"<<<<<<< conflict\" object naming each \
         unit's value; replace it with the content you want.",
    ] {
        assert!(out.contains(expected), "{expected:?} in {out}");
    }
    let err = stderr(&output);
    assert!(
        err.contains("1 file holds conflicts between units"),
        "{err}"
    );
    assert!(err.contains("compose --continue"), "{err}");
    let marked = read(&project.join("package.json"));
    assert!(marked.contains("\"<<<<<<< conflict\""), "{marked}");
    assert!(marked.contains("\"@acme/api\": \"^4.1\""), "{marked}");
    assert!(marked.contains("\"@acme/db\": \"^4.2\""), "{marked}");
    assert!(read(&project.join("assemblr.jsonc")).contains("@acme/db"));
    assert!(!locked_units(&project).contains(&"@acme/db".to_owned()));

    // Nothing else runs while the resolution is pending.
    let waiting = tree(&project);
    for args in [
        &["compose"][..],
        &["add", "@acme/api@^1"],
        &["remove", "@acme/db"],
        &["update"],
    ] {
        let err = stderr(&fails(fixture.assemblr(&project, args)));
        assert!(
            err.contains("waiting for you to resolve its conflicts in 1 file(s)"),
            "{args:?}: {err}"
        );
        assert_eq!(tree(&project), waiting, "{args:?}");
    }
    let err = stderr(&fails(
        fixture.assemblr(&project, &["compose", "--continue"]),
    ));
    assert!(
        err.contains("package.json: it still holds conflict markers"),
        "{err}"
    );
    assert!(err.contains("every \"<<<<<<< conflict\" object"), "{err}");

    // The declaration must be the one the composition was planned from.
    let resolved = resolve_hono(&marked, "^4.2");
    fs::write(project.join("package.json"), &resolved).unwrap();
    let edited = read(&project.join("assemblr.jsonc")).replace("\"^1.0.0\"", "\"^1.0.0\" ");
    fs::write(project.join("assemblr.jsonc"), &edited).unwrap();
    let before = tree(&project);
    let err = stderr(&fails(
        fixture.assemblr(&project, &["compose", "--continue"]),
    ));
    assert!(
        err.contains("assemblr.jsonc changed since the composition that waits"),
        "{err}"
    );
    assert!(err.contains("compose --abort"), "{err}");
    assert_eq!(tree(&project), before);
    fs::write(
        project.join("assemblr.jsonc"),
        edited.replace("\"^1.0.0\" ", "\"^1.0.0\""),
    )
    .unwrap();
    let output = succeeds(fixture.assemblr(&project, &["compose", "--continue"]));
    assert!(
        stdout(&output).contains("Resolved 1 file and finished the composition"),
        "{}",
        stdout(&output)
    );
    assert!(locked_units(&project).contains(&"@acme/db".to_owned()));
    assert_eq!(read(&project.join("package.json")), resolved);
    assert!(!project.join(".assemblr/journal.json").exists());

    // The resolution is the project's: composing again changes nothing.
    let output = succeeds(fixture.assemblr(&project, &["compose"]));
    assert!(
        stdout(&output).contains("Nothing to compose"),
        "{}",
        stdout(&output)
    );
    assert_eq!(read(&project.join("package.json")), resolved);

    // A sibling that leaves the contested value alone keeps the
    // resolution, and a conflict of the consumer's own elsewhere is
    // reported as theirs alone: the resolved conflict is not listed.
    publish_db(&fixture, &db, "1.1.0", &hono("^4.2"), "db, second edition");
    fs::write(project.join("db.txt"), "db, my edition").unwrap();
    let output = fails(fixture.assemblr(&project, &["update"]));
    let out = stdout(&output);
    assert!(
        out.contains("  conflict db.txt"),
        "{out}\n{}\n{}",
        stderr(&output),
        read(&project.join("package.json"))
    );
    assert!(!out.contains("conflict package.json"), "{out}");
    assert!(!out.contains("A conflict between units"), "{out}");
    assert!(out.contains("A conflict with your changes"), "{out}");
    assert_eq!(read(&project.join("package.json")), resolved);
    let err = stderr(&output);
    assert!(
        err.contains("1 file holds conflicts between your changes and the composition"),
        "{err}"
    );
    fs::write(project.join("db.txt"), "db, my edition").unwrap();
    succeeds(fixture.assemblr(&project, &["compose", "--continue"]));

    // A sibling that changes the contested value marks the file again,
    // with the resolution as the project's side, and the file holds both
    // kinds of conflict.
    publish_db(&fixture, &db, "1.2.0", &hono("^4.3"), "db, second edition");
    let output = fails(fixture.assemblr(&project, &["update"]));
    let out = stdout(&output);
    assert!(out.contains("  conflict package.json"), "{out}");
    assert!(out.contains("A conflict between units"), "{out}");
    assert!(out.contains("A conflict with your changes"), "{out}");
    let err = stderr(&output);
    assert!(
        err.contains(
            "1 file holds conflicts between units, and between your changes and the composition"
        ),
        "{err}"
    );
    let again = read(&project.join("package.json"));
    assert!(again.contains("<<<<<<< current (your file)"), "{again}");
    assert!(again.contains("\"hono\": \"^4.2\""), "{again}");
    assert!(again.contains("\"@acme/db\": \"^4.3\""), "{again}");
    assert!(
        again.contains(">>>>>>> incoming (the declared composition)"),
        "{again}"
    );
    // Giving up restores the resolution; the declaration was not touched,
    // and the summary does not say it was.
    let output = succeeds(fixture.assemblr(&project, &["compose", "--abort"]));
    let out = stdout(&output);
    assert!(out.contains("Aborted the composition"), "{out}");
    assert!(!out.contains("assemblr.jsonc included"), "{out}");
    assert_eq!(read(&project.join("package.json")), resolved);
    let after_add = read(&project.join("assemblr.jsonc"));
    assert_ne!(after_add, declaration);
    assert!(
        after_add.contains("\"@acme/db\": \"^1.0.0\""),
        "{after_add}"
    );
}

#[test]
fn siblings_conflict_the_same_whichever_comes_first_and_abort_restores_the_declaration() {
    let fixture = Fixture::new("consumer-order");
    base(&fixture);
    amender(&fixture, "api", "1.0.0", &hono("^4.1"));
    amender(&fixture, "db", "1.0.0", &hono("^4.2"));
    let one = project(&fixture, "one", &[("@acme/api", "^1")]);
    succeeds(fixture.assemblr(&one, &["compose"]));
    fails(fixture.assemblr(&one, &["add", "@acme/db"]));
    let other = project(&fixture, "other", &[("@acme/db", "^1")]);
    succeeds(fixture.assemblr(&other, &["compose"]));
    let before = tree(&other);
    let declaration = read(&other.join("assemblr.jsonc"));
    fails(fixture.assemblr(&other, &["add", "@acme/api"]));
    assert_eq!(
        read(&one.join("package.json")),
        read(&other.join("package.json"))
    );

    // An edit to the declaration while the composition waits is given up
    // with the resolution: the project is as it was before `add`.
    fs::write(other.join("package.json"), "{}\n").unwrap();
    fs::write(
        other.join("assemblr.jsonc"),
        read(&other.join("assemblr.jsonc")).replace("^1.0.0", "^1.0.1"),
    )
    .unwrap();
    let output = succeeds(fixture.assemblr(&other, &["compose", "--abort"]));
    let out = stdout(&output);
    assert!(
        out.contains("restored 2 it changed or removed, assemblr.jsonc included."),
        "{out}"
    );
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    assert_eq!(read(&other.join("assemblr.jsonc")), declaration);
    assert_eq!(tree(&other), before);
}

#[test]
fn a_clone_of_a_project_waiting_on_a_conflict_between_units_waits_again() {
    let fixture = Fixture::new("consumer-clone");
    base(&fixture);
    amender(&fixture, "api", "1.0.0", &hono("^4.1"));
    // `@acme/db` delivers nothing of its own: a clone of the waiting
    // project then holds no file the lock does not record.
    let db = fixture.repo("db");
    db.write(
        "manifest.jsonc",
        &manifest(
            "@acme/db",
            "1.0.0",
            &[("@acme/base", "=1.0.0")],
            &amending("package.json", "@acme/base", "1.0.0"),
        ),
    );
    db.write("amendments/package.json", &hono("^4.2"));
    succeeds(fixture.build(&db));
    db.publish("1.0.0");
    fixture.register(&db);
    let project = project(&fixture, "shop", &[("@acme/api", "^1")]);
    succeeds(fixture.assemblr(&project, &["compose"]));
    let output = fails(fixture.assemblr(&project, &["add", "@acme/db"]));
    assert!(
        stdout(&output).contains("  conflict package.json"),
        "{}\n{}",
        stdout(&output),
        stderr(&output)
    );
    let marked = read(&project.join("package.json"));
    assert!(marked.contains("\"<<<<<<< conflict\""), "{marked}");

    // A clone carries the tracked files, not the git-ignored journal: its
    // lock records the previous composition, and the file holds the
    // markers. The composition finds the same conflict, whose marked
    // content is the file's, and waits again without marking it twice.
    let clone = fixture.dir.join("clone");
    copy_tree(&project, &clone);
    fs::remove_file(clone.join(".assemblr/journal.json")).unwrap();
    fs::remove_dir_all(clone.join(".assemblr/previous")).unwrap();
    let output = fails(fixture.assemblr(&clone, &["compose"]));
    let out = stdout(&output);
    assert!(
        out.contains("  conflict package.json"),
        "{out}\n{}",
        stderr(&output)
    );
    assert!(out.contains(UNIT_CONFLICT), "{out}");
    assert_eq!(read(&clone.join("package.json")), marked);
    let err = stderr(&fails(fixture.assemblr(&clone, &["compose", "--continue"])));
    assert!(err.contains("still holds conflict markers"), "{err}");
    // Another tool's markers in the resolution are content.
    let resolved = resolve_hono(&marked, "^4.2")
        .replace("\"name\"", "\"<<<<<<< HEAD\": \">>>>>>> x\",\n  \"name\"");
    fs::write(clone.join("package.json"), &resolved).unwrap();
    succeeds(fixture.assemblr(&clone, &["compose", "--continue"]));
    assert_eq!(read(&clone.join("package.json")), resolved);
    let lock: serde_json::Value =
        serde_json::from_str(&read(&clone.join("assemblr-lock.json"))).unwrap();
    assert_eq!(lock["units"]["@acme/db"]["version"], "1.0.0");
    assert!(read(&clone.join("package.json")).contains("\"hono\": \"^4.2\""));
}

#[test]
fn a_text_conflict_between_siblings_is_marked_for_the_consumer_with_the_units_named() {
    let fixture = Fixture::new("consumer-text");
    base_with(&fixture, &[("README.md", "# Base\n\nRun it.\n")]);
    let api = fixture.repo("api");
    publish_unit(
        &fixture,
        &api,
        "api",
        "1.0.0",
        &[("@acme/base", "=1.0.0")],
        &[("README.md", "# Base\n\nRun it (api).\n")],
    );
    fixture.register(&api);
    let db = fixture.repo("db");
    publish_unit(
        &fixture,
        &db,
        "db",
        "1.0.0",
        &[("@acme/base", "=1.0.0")],
        &[("README.md", "# Base\n\nRun it (db).\n")],
    );
    fixture.register(&db);
    let project = project(&fixture, "shop", &[("@acme/api", "^1"), ("@acme/db", "^1")]);
    let output = fails(fixture.assemblr(&project, &["compose"]));
    assert!(
        stdout(&output).contains("  conflict README.md"),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        read(&project.join("README.md")),
        "# Base\n\n<<<<<<< @acme/api\nRun it (api).\n||||||| base\nRun it.\n=======\nRun it \
         (db).\n>>>>>>> @acme/db\n"
    );
    // The rest of the composition is in place, and the lock waits.
    assert_eq!(read(&project.join("api.txt")), "api");
    assert!(!project.join("assemblr-lock.json").exists());
    // Giving up removes the created file whatever the consumer made of it.
    fs::write(
        project.join("README.md"),
        "# Base\n\nRun it (api,\n>>>>>>> @acme/db\n",
    )
    .unwrap();
    let output = succeeds(fixture.assemblr(&project, &["compose", "--abort"]));
    assert!(
        stdout(&output).contains("Aborted the composition: removed"),
        "{}",
        stdout(&output)
    );
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    assert!(!project.join("README.md").exists());
    assert!(!project.join("api.txt").exists());
    fails(fixture.assemblr(&project, &["compose"]));
    fs::write(project.join("README.md"), "# Base\n\nRun it (api, db).\n").unwrap();
    succeeds(fixture.assemblr(&project, &["compose", "--continue"]));
    assert!(project.join("assemblr-lock.json").exists());
    let output = succeeds(fixture.assemblr(&project, &["compose"]));
    assert!(
        stdout(&output).contains("Nothing to compose"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn the_marked_copy_does_not_depend_on_the_order_of_declarations_or_registrations() {
    let one = Fixture::new("order-one");
    base(&one);
    amender(&one, "api", "1.0.0", &hono("^4.1"));
    amender(&one, "db", "1.0.0", &hono("^4.2"));
    let first = app(&one, &[("@acme/api", "^1"), ("@acme/db", "^1")]);
    let err_one = stderr(&fails(one.build(&first)));

    let other = Fixture::new("order-other");
    base(&other);
    amender(&other, "db", "1.0.0", &hono("^4.2"));
    amender(&other, "api", "1.0.0", &hono("^4.1"));
    let second = app(&other, &[("@acme/db", "^1"), ("@acme/api", "^1")]);
    let err_other = stderr(&fails(other.build(&second)));

    assert_eq!(
        first.read("amendments/package.json"),
        second.read("amendments/package.json")
    );
    assert_eq!(err_one, err_other);
    assert!(err_one.contains("\"^4.1\"; @acme/db changes"), "{err_one}");
}

#[test]
fn every_lockfile_entry_a_layered_base_depends_on_is_verified() {
    let fixture = Fixture::new("verify");
    base(&fixture);
    let db = amender(
        &fixture,
        "db",
        "1.0.0",
        &with_dependency("drizzle-orm", "^0.44"),
    );
    let top = fixture.repo("top");
    top.write(
        "manifest.jsonc",
        &manifest("@acme/top", "1.0.0", &[("@acme/db", "^1")], ""),
    );
    top.write("src/top.txt", "top");
    succeeds(fixture.build(&top));
    succeeds(fixture.assemblr(&top.path, &["unit", "amend", "package.json"]));
    top.write(
        "amendments/package.json",
        &top.read("amendments/package.json")
            .replace("\"name\": \"base\"", "\"name\": \"top\""),
    );
    succeeds(fixture.build(&top));
    // The published lockfile identifies `db` 1.0.0, part of the base, by
    // other content than the registry records.
    let tree = db.git(&["rev-parse", "v1.0.0^{tree}"]);
    let forged: String = tree
        .chars()
        .map(|c| if c == 'a' { 'b' } else { 'a' })
        .collect();
    top.write(
        "assemblr-lock.json",
        &top.read("assemblr-lock.json").replace(&tree, &forged),
    );
    top.publish("1.0.0");
    fixture.register(&top);

    let app = app(&fixture, &[("@acme/top", "^1")]);
    let err = stderr(&fails(fixture.build(&app)));
    assert!(
        err.contains(
            "integrity conflict: @acme/top 1.0.0 amends package.json of @acme/base 1.0.0, a base \
             that includes the amendment of @acme/db 1.0.0, whose content its lockfile \
             identifies as commit"
        ),
        "{err}"
    );
    assert!(
        err.contains("for @acme/db 1.0.0; a published version's content must not change"),
        "{err}"
    );
    assert!(!app.exists("build"));
}

/// Publishes `repo` as `@acme/<name>` at `version` with `dependencies`,
/// one source file, and `amendments` of files of `@acme/base` 1.0.0.
fn publish_unit(
    fixture: &Fixture,
    repo: &Repo,
    name: &str,
    version: &str,
    dependencies: &[(&str, &str)],
    amendments: &[(&str, &str)],
) {
    let declared: Vec<String> = amendments
        .iter()
        .map(|(path, _)| format!(r#""{path}": {{"unit": "@acme/base", "base": "1.0.0"}}"#))
        .collect();
    let extra = if declared.is_empty() {
        String::new()
    } else {
        format!(r#", "amendments": {{{}}}"#, declared.join(", "))
    };
    repo.write(
        "manifest.jsonc",
        &manifest(&format!("@acme/{name}"), version, dependencies, &extra),
    );
    repo.write(&format!("src/{name}.txt"), name);
    let _ = fs::remove_dir_all(repo.path.join("amendments"));
    for (path, copy) in amendments {
        repo.write(&format!("amendments/{path}"), copy);
    }
    succeeds(fixture.build(repo));
    repo.publish(version);
    fixture.sync();
}

#[test]
fn a_file_without_a_merge_is_still_amended_across_versions_when_it_is_unchanged() {
    let fixture = Fixture::new("xml-across");
    let base = base(&fixture);
    base.write("manifest.jsonc", &manifest("@acme/base", "1.0.1", &[], ""));
    base.write(
        "src/pom.xml",
        "<project>\n  <name>base</name>\n</project>\n",
    );
    base.publish("1.0.1");
    fixture.sync();
    let xml = fixture.repo("xml");
    xml.write(
        "manifest.jsonc",
        &manifest(
            "@acme/xml",
            "1.0.0",
            &[("@acme/base", "^1")],
            r#", "amendments": {"pom.xml": {"unit": "@acme/base", "base": "1.0.1"}}"#,
        ),
    );
    xml.write("src/xml.txt", "xml");
    xml.write(
        "amendments/pom.xml",
        "<project>\n  <name>xml</name>\n</project>\n",
    );
    succeeds(fixture.build(&xml));
    xml.publish("1.0.0");
    fixture.register(&xml);
    // The provider moves without touching the file: the copy applies.
    base.write("manifest.jsonc", &manifest("@acme/base", "1.1.0", &[], ""));
    base.write("src/README.md", "# Base 1.1\n");
    base.publish("1.1.0");
    fixture.sync();
    let app = app(&fixture, &[("@acme/xml", "^1"), ("@acme/base", "^1.1")]);
    succeeds(fixture.build(&app));
    assert!(app.read("assemblr-lock.json").contains("\"1.1.0\""));
    assert_eq!(
        app.read("build/pom.xml"),
        "<project>\n  <name>xml</name>\n</project>\n"
    );
    // The provider changes the file: there is no merge to apply the copy.
    base.write("manifest.jsonc", &manifest("@acme/base", "1.2.0", &[], ""));
    base.write(
        "src/pom.xml",
        "<project>\n  <name>base 1.2</name>\n</project>\n",
    );
    base.publish("1.2.0");
    fixture.sync();
    succeeds(fixture.assemblr(&app.path, &["unit", "update"]));
    let err = stderr(&fails(fixture.build(&app)));
    assert!(
        err.contains("applied across versions only to a file that is unchanged"),
        "{err}"
    );
}

#[test]
fn pending_copies_that_share_a_new_directory_are_all_created() {
    let fixture = Fixture::new("shared-directory");
    base_with(
        &fixture,
        &[
            ("config/a.json", "{\n  \"x\": 1\n}\n"),
            ("config/b.json", "{\n  \"x\": 1\n}\n"),
        ],
    );
    let api = fixture.repo("api");
    publish_unit(
        &fixture,
        &api,
        "api",
        "1.0.0",
        &[("@acme/base", "=1.0.0")],
        &[
            ("config/a.json", "{\n  \"x\": 2\n}\n"),
            ("config/b.json", "{\n  \"x\": 2\n}\n"),
        ],
    );
    fixture.register(&api);
    let db = fixture.repo("db");
    publish_unit(
        &fixture,
        &db,
        "db",
        "1.0.0",
        &[("@acme/base", "=1.0.0")],
        &[],
    );
    fixture.register(&db);

    // `unit update` brings two conflicts into one directory that does not
    // exist yet, and creates both copies.
    let app = app(&fixture, &[("@acme/api", "^1"), ("@acme/db", "^1")]);
    succeeds(fixture.build(&app));
    assert!(!app.exists("amendments"));
    publish_unit(
        &fixture,
        &db,
        "db",
        "1.1.0",
        &[("@acme/base", "=1.0.0")],
        &[
            ("config/a.json", "{\n  \"x\": 3\n}\n"),
            ("config/b.json", "{\n  \"x\": 3\n}\n"),
        ],
    );
    let output = fails(fixture.assemblr(&app.path, &["unit", "update"]));
    assert!(
        stdout(&output).contains(
            "Created, declared in manifest.jsonc: amendments/config/a.json, \
             amendments/config/b.json."
        ),
        "{}\n{}",
        stdout(&output),
        stderr(&output)
    );
    for path in ["amendments/config/a.json", "amendments/config/b.json"] {
        assert!(app.read(path).contains("\"<<<<<<< conflict\""), "{path}");
    }
    assert!(app.read("assemblr-lock.json").contains("\"1.1.0\""));

    // `unit add` of the sibling does the same from a build.
    let other = fixture.repo("other");
    other.write(
        "manifest.jsonc",
        &manifest("@acme/other", "0.1.0", &[("@acme/api", "^1")], ""),
    );
    other.write("src/other.txt", "other");
    succeeds(fixture.build(&other));
    let output = fails(fixture.assemblr(&other.path, &["unit", "add", "@acme/db"]));
    let err = stderr(&output);
    for path in ["amendments/config/a.json", "amendments/config/b.json"] {
        assert!(err.contains(&format!("{path}: the amendments of")), "{err}");
        assert!(other.read(path).contains("\"<<<<<<< conflict\""), "{path}");
    }
    let manifest = other.read("manifest.jsonc");
    assert!(manifest.contains("\"config/a.json\""), "{manifest}");
    assert!(manifest.contains("\"config/b.json\""), "{manifest}");
    assert!(other.read("assemblr-lock.json").contains("@acme/db"));
}

#[test]
fn a_lower_amendment_dropped_below_a_published_amender_leaves_its_copy() {
    let fixture = Fixture::new("dropped-below");
    base_with(&fixture, &[("README.md", "# Base\n\nIntro.\n\nEnd.\n")]);
    let low = fixture.repo("low");
    publish_unit(
        &fixture,
        &low,
        "low",
        "1.0.0",
        &[("@acme/base", "=1.0.0")],
        &[("README.md", "# Base\n\nIntro.\n\nEnd.\nlow\n")],
    );
    fixture.register(&low);
    let mid = fixture.repo("mid");
    mid.write(
        "manifest.jsonc",
        &manifest("@acme/mid", "1.0.0", &[("@acme/low", "^1")], ""),
    );
    mid.write("src/mid.txt", "mid");
    succeeds(fixture.build(&mid));
    succeeds(fixture.assemblr(&mid.path, &["unit", "amend", "README.md"]));
    assert_eq!(
        mid.read("amendments/README.md"),
        "# Base\n\nIntro.\n\nEnd.\nlow\n"
    );
    mid.write(
        "amendments/README.md",
        "# Base\nmid\n\nIntro.\n\nEnd.\nlow\n",
    );
    succeeds(fixture.build(&mid));
    mid.publish("1.0.0");
    fixture.register(&mid);
    // `low` 1.1.0 no longer amends the file. A dependent on it gets
    // `mid`'s own contribution only: its copy is moved off the base it
    // was taken on, not delivered as it is.
    publish_unit(
        &fixture,
        &low,
        "low",
        "1.1.0",
        &[("@acme/base", "=1.0.0")],
        &[],
    );
    let app = app(&fixture, &[("@acme/mid", "^1"), ("@acme/low", "^1.1")]);
    succeeds(fixture.build(&app));
    assert!(app.read("assemblr-lock.json").contains("\"1.1.0\""));
    assert_eq!(
        app.read("build/README.md"),
        "# Base\nmid\n\nIntro.\n\nEnd.\n"
    );
}

#[test]
fn a_json_file_with_a_placeholder_is_amended_and_rebased() {
    let fixture = Fixture::new("json-placeholder");
    let base = fixture.repo("base");
    base.write(
        "manifest.jsonc",
        &manifest(
            "@acme/base",
            "1.0.0",
            &[],
            r#", "placeholders": {"replace": {"__PORT__": "The port the server listens on."}}"#,
        ),
    );
    base.write(
        "src/config.json",
        "{\n  \"port\": \"__PORT__\",\n  \"name\": \"base\"\n}\n",
    );
    base.publish("1.0.0");
    fixture.register(&base);
    let api = fixture.repo("api");
    publish_unit(
        &fixture,
        &api,
        "api",
        "1.0.0",
        &[("@acme/base", "=1.0.0")],
        &[(
            "config.json",
            "{\n  \"api\": true,\n  \"port\": \"__PORT__\",\n  \"name\": \"base\"\n}\n",
        )],
    );
    fixture.register(&api);
    let db = fixture.repo("db");
    publish_unit(
        &fixture,
        &db,
        "db",
        "1.0.0",
        &[("@acme/base", "=1.0.0")],
        &[(
            "config.json",
            "{\n  \"port\": \"__PORT__\",\n  \"name\": \"base\",\n  \"db\": true\n}\n",
        )],
    );
    fixture.register(&db);
    let app = app(&fixture, &[("@acme/api", "^1"), ("@acme/db", "^1")]);
    succeeds(fixture.build(&app));
    assert_eq!(
        app.read("build/config.json"),
        "{\n  \"api\": true,\n  \"port\": \"__PORT__\",\n  \"name\": \"base\",\n  \"db\": true\n}\n"
    );
    // The copy is the sources layered, placeholder and all.
    succeeds(fixture.assemblr(&app.path, &["unit", "amend", "config.json"]));
    let copy = "{\n  \"api\": true,\n  \"port\": \"__PORT__\",\n  \"name\": \"base\",\n  \"db\": true\n}\n";
    assert_eq!(app.read("amendments/config.json"), copy);
    app.write(
        "amendments/config.json",
        &copy.replace("\"api\": true", "\"api\": false"),
    );
    succeeds(fixture.build(&app));
    assert!(app.read("build/config.json").contains("\"api\": false"));
    // A change below flows into the copy by line too.
    publish_unit(
        &fixture,
        &db,
        "db",
        "1.1.0",
        &[("@acme/base", "=1.0.0")],
        &[(
            "config.json",
            "{\n  \"port\": \"__PORT__\",\n  \"name\": \"base\",\n  \"db\": 2\n}\n",
        )],
    );
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "update"]));
    assert!(
        stdout(&output).contains("Rewritten to follow the changes below"),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        app.read("amendments/config.json"),
        "{\n  \"api\": false,\n  \"port\": \"__PORT__\",\n  \"name\": \"base\",\n  \"db\": 2\n}\n"
    );
    succeeds(fixture.build(&app));
    assert_eq!(
        app.read("build/config.json"),
        "{\n  \"api\": false,\n  \"port\": \"__PORT__\",\n  \"name\": \"base\",\n  \"db\": 2\n}\n"
    );
}
