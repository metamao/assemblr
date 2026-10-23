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
    let base = fixture.repo("base");
    base.write("manifest.jsonc", &manifest("@acme/base", "1.0.0", &[], ""));
    base.write("src/package.json", PACKAGE);
    base.write("src/README.md", "# Base\n");
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

#[test]
fn a_consumer_project_fails_on_a_sibling_conflict_that_no_unit_resolves() {
    let fixture = Fixture::new("consumer");
    base(&fixture);
    amender(&fixture, "api", "1.0.0", &hono("^4.1"));
    amender(&fixture, "db", "1.0.0", &hono("^4.2"));
    let project = fixture.dir.join("shop");
    succeeds(fixture.assemblr(&fixture.dir, &["init", "shop"]));
    git(&project, &["init", "-q"]);
    fs::write(
        project.join("assemblr.jsonc"),
        "{\n  \"dependencies\": {\"@acme/api\": \"^1\", \"@acme/db\": \"^1\"},\n  \"options\": {}\n}\n",
    )
    .unwrap();
    let err = stderr(&fails(fixture.assemblr(&project, &["compose"])));
    assert!(
        err.contains(
            "src/package.json of @acme/base 1.0.0: the amendments of @acme/api and @acme/db \
             conflict, and no unit above them resolves the conflict:"
        ),
        "{err}"
    );
    assert!(
        err.contains("/dependencies/hono: @acme/api changes \"^4\" to \"^4.1\""),
        "{err}"
    );
    assert!(!project.join("package.json").exists());

    // A unit above both that resolves the conflict composes for the
    // consumer, with its copy.
    let app = app(&fixture, &[("@acme/api", "^1"), ("@acme/db", "^1")]);
    let err = stderr(&fails(fixture.build(&app)));
    assert!(
        err.contains("the copy holds each conflict between markers"),
        "{err}"
    );
    let marked = app.read("amendments/package.json");
    app.write("amendments/package.json", &resolve_hono(&marked, "^4.2"));
    succeeds(fixture.build(&app));
    app.publish("0.1.0");
    fixture.register(&app);
    fs::write(
        project.join("assemblr.jsonc"),
        "{\n  \"dependencies\": {\"@acme/app\": \"^0.1\"},\n  \"options\": {}\n}\n",
    )
    .unwrap();
    succeeds(fixture.assemblr(&project, &["compose"]));
    let composed = fs::read_to_string(project.join("package.json")).unwrap();
    assert!(composed.contains("\"hono\": \"^4.2\""), "{composed}");
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
