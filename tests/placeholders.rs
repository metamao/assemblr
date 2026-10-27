//! Declared placeholders, run through the real binary: what a unit's build,
//! check, and publication say about them, and what `compose`, `add`, and
//! `status` report to the consumer (see `docs/domain/placeholders.md`).
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

use serde_json::{Value, json};

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
}

/// A manifest with these dependencies and extra top-level fields.
fn manifest(name: &str, dependencies: &[(&str, &str)], extra: &str) -> String {
    let dependencies: Vec<String> = dependencies
        .iter()
        .map(|(identity, range)| format!(r#""{identity}": "{range}""#))
        .collect();
    format!(
        r#"{{"name": "{name}", "version": "1.0.0", "description": "A unit.",
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

    fn commit(&self) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", "unit"]);
    }

    /// Commits everything and tags it `v1.0.0`.
    fn publish(&self) {
        self.commit();
        self.git(&["tag", "v1.0.0"]);
    }
}

/// `@acme/app`, declaring `__APP_NAME__` in two files.
fn app(fixture: &Fixture) -> Repo {
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            &[],
            r#", "placeholders": {"replace": {"__APP_NAME__": "The application's name, in kebab-case."}}"#,
        ),
    );
    app.write("src/package.json", "{\n  \"name\": \"__APP_NAME__\"\n}\n");
    app.write("src/README.md", "# __APP_NAME__\n\nAn app.\n");
    app
}

/// `@acme/worker`, on `@acme/app`: its file holds the application's
/// placeholder and one of its own, and a token that is not a placeholder.
fn worker(fixture: &Fixture) -> Repo {
    let worker = fixture.repo("worker");
    worker.write(
        "manifest.jsonc",
        &manifest(
            "@acme/worker",
            &[("@acme/app", "^1")],
            r#", "placeholders": {"replace": {"__ROUTE__": "The route the worker serves."},
                "ignore": ["__DEV__"]}"#,
        ),
    );
    worker.write(
        "src/wrangler.jsonc",
        "{\n  \"name\": \"__APP_NAME__-worker\",\n  \"route\": \"__ROUTE__\"\n}\n",
    );
    worker.write("src/define.js", "export const dev = __DEV__;\n");
    worker
}

/// `@acme/other`, independent of `@acme/app`, declaring the same token with
/// a meaning of its own.
fn other(fixture: &Fixture) -> Repo {
    let other = fixture.repo("other");
    other.write(
        "manifest.jsonc",
        &manifest(
            "@acme/other",
            &[],
            r#", "placeholders": {"replace": {"__APP_NAME__": "The other service's name."}}"#,
        ),
    );
    other.write("src/other.txt", "service: __APP_NAME__\n");
    other
}

/// The three units, published and registered in `order` (indexes into app,
/// worker, other).
fn publish_units(fixture: &Fixture, order: [usize; 3]) {
    let units = [app(fixture), worker(fixture), other(fixture)];
    for unit in &units {
        unit.publish();
    }
    for index in order {
        fixture.register(&units[index]);
    }
}

fn project(fixture: &Fixture, name: &str, dependencies: &str) -> PathBuf {
    succeeds(fixture.assemblr(&fixture.dir, &["init", name]));
    let project = fixture.dir.join(name);
    fs::write(
        project.join("assemblr.jsonc"),
        format!("{{\n  \"dependencies\": {{{dependencies}}}\n}}\n"),
    )
    .unwrap();
    project
}

fn status_json(fixture: &Fixture, project: &Path) -> Value {
    let output = succeeds(fixture.assemblr(project, &["status", "--json"]));
    serde_json::from_str(&stdout(&output)).unwrap()
}

const REPORT: &str = "2 placeholders to replace, in 4 files:

  __APP_NAME__

    @acme/app: The application's name, in kebab-case.
      README.md:1
      package.json:2
      wrangler.jsonc:2

    @acme/other: The other service's name.
      other.txt:1

  __ROUTE__

    @acme/worker: The route the worker serves.
      wrangler.jsonc:3
";

#[test]
fn a_consumer_sees_each_placeholder_where_it_remains_and_status_follows_it() {
    let fixture = Fixture::new("report");
    publish_units(&fixture, [0, 1, 2]);
    let shop = project(
        &fixture,
        "shop",
        r#""@acme/worker": "^1", "@acme/other": "^1""#,
    );
    let output = succeeds(fixture.assemblr(&shop, &["compose"]));
    let out = stdout(&output);
    assert!(out.contains(REPORT), "{out}");
    assert!(
        stderr(&output).contains(
            "hint: replace each placeholder with your value; `assemblr status` shows what remains"
        ),
        "{}",
        stderr(&output)
    );
    // A token the unit says is not a placeholder is not reported.
    assert!(!out.contains("__DEV__"), "{out}");

    // The consumer replaces some, and writes the token in a file of its own:
    // status reports what remains in the composed files only.
    fs::write(shop.join("package.json"), "{\n  \"name\": \"shop\"\n}\n").unwrap();
    fs::write(
        shop.join("wrangler.jsonc"),
        "{\n  \"name\": \"shop-worker\",\n  \"route\": \"__ROUTE__\"\n}\n",
    )
    .unwrap();
    fs::write(shop.join("notes.txt"), "__APP_NAME__\n").unwrap();
    let out = stdout(&succeeds(fixture.assemblr(&shop, &["status"])));
    assert!(
        out.contains(
            "Placeholders to replace:\n\n  __APP_NAME__\n\n    @acme/app: The application's \
             name, in kebab-case.\n      README.md:1\n\n    @acme/other: The other \
             service's name.\n      other.txt:1\n\n  __ROUTE__\n\n    @acme/worker: The \
             route the worker serves.\n      wrangler.jsonc:3\n"
        ),
        "{out}"
    );
    assert!(!out.contains("notes.txt"), "{out}");
    let status = status_json(&fixture, &shop);
    assert_eq!(
        status["placeholders"][1],
        json!({
            "placeholder": "__ROUTE__",
            "declarations": [{
                "unit": "@acme/worker",
                "description": "The route the worker serves.",
                "locations": [{"path": "wrangler.jsonc", "line": 3, "column": 13}],
            }],
        })
    );
    assert_eq!(
        status["placeholders"][0]["declarations"][0]["locations"],
        json!([{"path": "README.md", "line": 1, "column": 3}])
    );

    // With every one replaced, nothing is reported, and the edits stay.
    fs::write(shop.join("README.md"), "# shop\n\nAn app.\n").unwrap();
    fs::write(shop.join("other.txt"), "service: billing\n").unwrap();
    fs::write(
        shop.join("wrangler.jsonc"),
        "{\n  \"name\": \"shop-worker\",\n  \"route\": \"/api\"\n}\n",
    )
    .unwrap();
    let out = stdout(&succeeds(fixture.assemblr(&shop, &["status"])));
    assert!(!out.contains("Placeholders"), "{out}");
    assert_eq!(status_json(&fixture, &shop)["placeholders"], json!([]));
    let out = stdout(&succeeds(fixture.assemblr(&shop, &["compose"])));
    assert!(!out.contains("placeholder"), "{out}");
}

#[test]
fn add_reports_the_placeholders_of_the_composition_it_writes() {
    let fixture = Fixture::new("add");
    publish_units(&fixture, [0, 1, 2]);
    let shop = project(&fixture, "shop", "");
    let output = succeeds(fixture.assemblr(&shop, &["add", "@acme/app"]));
    let out = stdout(&output);
    assert!(
        out.contains(
            "1 placeholder to replace, in 2 files:\n\n  __APP_NAME__\n\n    @acme/app: The \
             application's name, in kebab-case.\n      README.md:1\n      package.json:2\n"
        ),
        "{out}"
    );
}

#[test]
fn the_report_waits_for_the_conflicts_and_comes_with_continue() {
    let fixture = Fixture::new("continue");
    publish_units(&fixture, [0, 1, 2]);
    let shop = project(&fixture, "shop", "");
    // A README the consumer had already: the unit's conflicts with it.
    fs::write(shop.join("README.md"), "# Notes\n").unwrap();
    let output = fixture.assemblr(&shop, &["add", "@acme/app"]);
    let out = stdout(&output);
    assert!(out.contains("README.md"), "{out}");
    assert!(!out.contains("placeholder"), "{out}");
    assert_eq!(status_json(&fixture, &shop)["placeholders"], json!(null));

    fs::write(shop.join("README.md"), "# __APP_NAME__\n\nNotes.\n").unwrap();
    let output = succeeds(fixture.assemblr(&shop, &["compose", "--continue"]));
    let out = stdout(&output);
    assert!(
        out.contains(
            "1 placeholder to replace, in 2 files:\n\n  __APP_NAME__\n\n    @acme/app: The \
             application's name, in kebab-case.\n      README.md:1\n      package.json:2\n"
        ),
        "{out}"
    );
    assert!(
        stderr(&output).contains("hint: replace each placeholder"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn the_report_does_not_depend_on_registration_or_declaration_order() {
    let outputs: Vec<(String, Value)> = [([0, 1, 2], true), ([2, 1, 0], false)]
        .into_iter()
        .map(|(order, worker_first)| {
            let fixture = Fixture::new(if worker_first {
                "order"
            } else {
                "order-reversed"
            });
            publish_units(&fixture, order);
            let dependencies = if worker_first {
                r#""@acme/worker": "^1", "@acme/other": "^1""#
            } else {
                r#""@acme/other": "^1", "@acme/worker": "^1""#
            };
            let shop = project(&fixture, "shop", dependencies);
            let out = stdout(&succeeds(fixture.assemblr(&shop, &["compose"])));
            (out, status_json(&fixture, &shop)["placeholders"].clone())
        })
        .collect();
    assert!(outputs[0].0.contains(REPORT), "{}", outputs[0].0);
    assert_eq!(outputs[0], outputs[1]);
}

#[test]
fn an_amendment_that_replaces_a_placeholder_takes_it_out_of_the_report() {
    let fixture = Fixture::new("amended");
    let app = app(&fixture);
    app.publish();
    fixture.register(&app);
    let site = fixture.repo("site");
    site.write(
        "manifest.jsonc",
        &manifest("@acme/site", &[("@acme/app", "^1")], ""),
    );
    site.write("src/site.txt", "site\n");
    succeeds(fixture.assemblr(&site.path, &["unit", "build"]));
    succeeds(fixture.assemblr(&site.path, &["unit", "amend", "README.md"]));
    site.write("amendments/README.md", "# Site\n\nAn app.\n");
    let out = stdout(&succeeds(fixture.assemblr(&site.path, &["unit", "build"])));
    assert!(
        out.contains("Placeholders: __APP_NAME__ in 1 file."),
        "{out}"
    );
    succeeds(fixture.assemblr(&site.path, &["unit", "check"]));
    site.publish();
    fixture.register(&site);

    let shop = project(&fixture, "shop", r#""@acme/site": "^1""#);
    let out = stdout(&succeeds(fixture.assemblr(&shop, &["compose"])));
    assert!(
        out.contains("1 placeholder to replace, in 1 file:"),
        "{out}"
    );
    assert!(out.contains("      package.json:2\n"), "{out}");
    assert!(!out.contains("README.md"), "{out}");
}

#[test]
fn every_placeholder_problem_of_a_unit_is_reported_and_nothing_is_built() {
    let fixture = Fixture::new("problems");
    let app = app(&fixture);
    app.publish();
    fixture.register(&app);
    let worker = worker(&fixture);
    worker.publish();
    fixture.register(&worker);
    let stack = fixture.repo("stack");
    stack.write(
        "manifest.jsonc",
        &manifest(
            "@acme/stack",
            &[("@acme/worker", "^1")],
            r#", "placeholders": {
                "replace": {"__APP_NAME__": "Again.", "__UNUSED__": "Never written."},
                "ignore": ["__ROUTE__", "__NEVER__"]
            }"#,
        ),
    );
    stack.write("src/__APP_NAME__.txt", "__APP_NAME__ and __ROUTE__\n");
    let err = stderr(&fails(fixture.assemblr(&stack.path, &["unit", "build"])));
    for expected in [
        "error: src/__APP_NAME__.txt has the placeholder __APP_NAME__ in its path; Assemblr \
         never renames files, so a placeholder belongs in a file's content",
        "error: placeholders.replace declares __APP_NAME__, which @acme/app declares already",
        "remove it from \"placeholders.replace\": the dependency's declaration and \
         description apply to the files of this unit too",
        "error: placeholders.ignore lists __ROUTE__, which @acme/worker declares as a placeholder",
        "error: placeholders.replace declares __UNUSED__, but no file of the unit contains it",
        "error: placeholders.ignore lists __NEVER__, but no file of the unit contains it",
        "remove it from \"placeholders.ignore\";",
        "note: build/ and assemblr-lock.json were not changed",
    ] {
        assert!(err.contains(expected), "{expected:?} in {err}");
    }
    assert!(!stack.path.join("build").exists());
    // `unit check` reports the same problems, and that the unit has no lock
    // yet.
    let check = stderr(&fails(fixture.assemblr(&stack.path, &["unit", "check"])));
    let errors = |text: &str| -> Vec<String> {
        text.lines()
            .filter(|line| line.starts_with("error: "))
            .map(str::to_owned)
            .collect()
    };
    let (check, build) = (errors(&check), errors(&err));
    assert_eq!(check[..build.len()], build[..], "{check:?}");
    assert!(
        check[build.len()].contains("there is no assemblr-lock.json"),
        "{check:?}"
    );
}

#[test]
fn undeclared_tokens_warn_at_build_and_refuse_check_and_publication() {
    let fixture = Fixture::new("undeclared");
    let app = app(&fixture);
    app.publish();
    fixture.register(&app);
    // A dependency's placeholder is declared for the files of its
    // dependents: building the worker warns about nothing.
    let worker = worker(&fixture);
    let output = succeeds(fixture.assemblr(&worker.path, &["unit", "build"]));
    assert!(!stderr(&output).contains("warning"), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("Placeholders: __APP_NAME__ in 3 files, __ROUTE__ in 1 file."),
        "{}",
        stdout(&output)
    );

    worker.write("src/extra.txt", "__TOKEN__ and __TOKEN__\n");
    let output = succeeds(fixture.assemblr(&worker.path, &["unit", "build"]));
    let err = stderr(&output);
    for expected in [
        "warning: 1 token looks like a placeholder but is not declared:",
        "  __TOKEN__\n      src/extra.txt:1\n\nhint:",
        "hint: add it to \"placeholders.replace\" with a description, or to \
         \"placeholders.ignore\" if it is not a placeholder",
    ] {
        assert!(err.contains(expected), "{expected:?} in {err}");
    }
    worker.commit();
    let err = stderr(&fails(fixture.assemblr(&worker.path, &["unit", "check"])));
    assert!(err.contains("__TOKEN__"), "{err}");
    fixture.register(&worker);
    let err = stderr(&fails(fixture.assemblr(&worker.path, &["unit", "publish"])));
    assert!(err.contains("__TOKEN__"), "{err}");
    assert_eq!(worker.git(&["tag"]), "");

    // Listed as not a placeholder, it no longer stops anything.
    worker.write(
        "manifest.jsonc",
        &manifest(
            "@acme/worker",
            &[("@acme/app", "^1")],
            r#", "placeholders": {"replace": {"__ROUTE__": "The route the worker serves."},
                "ignore": ["__DEV__", "__TOKEN__"]}"#,
        ),
    );
    let output = succeeds(fixture.assemblr(&worker.path, &["unit", "build"]));
    assert!(!stderr(&output).contains("warning"), "{}", stderr(&output));
    worker.commit();
    succeeds(fixture.assemblr(&worker.path, &["unit", "check"]));
    succeeds(fixture.assemblr(&worker.path, &["unit", "publish"]));
}
