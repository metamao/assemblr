//! Required parameters with examples and parameter mappings, run through the
//! real binary on units shaped like the reference scenario: an application
//! unit, a Worker unit that depends on it, and a stack that depends on both
//! and exposes the application's name once.
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

use common::{Fixture, git, stderr, stdout, succeeds};

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

    fn build(&self, repo: &Repo) -> Output {
        self.assemblr(&repo.path, &["unit", "build"])
    }

    fn register(&self, repo: &Repo) {
        let path = repo.path.to_str().unwrap();
        succeeds(self.assemblr(&self.dir, &["registry", "add", path]));
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

    /// Commits everything and tags it `v<version>`.
    fn publish(&self, version: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", version]);
        self.git(&["tag", &format!("v{version}")]);
    }
}

const APP_PARAM: &str = r#""appName": {"type": "string", "description": "The application name.", "example": "demo-app"}"#;

/// Publishes `@acme/app` and `@acme/worker`, which take the application's
/// name as a required parameter with an example, the worker mapping it on to
/// the application; both are registered.
fn base_units(fixture: &Fixture) {
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "1.0.0",
            &[],
            &format!(r#", "params": {{{APP_PARAM}}}"#),
        ),
    );
    app.write(
        "src/package.json.assemblr.tmpl",
        "{\n  \"name\": \"{{ appName }}\"\n}\n",
    );
    app.publish("1.0.0");
    fixture.register(&app);
    let worker = fixture.repo("worker");
    worker.write(
        "manifest.jsonc",
        &manifest(
            "@acme/worker",
            "1.0.0",
            &[("@acme/app", "^1")],
            &format!(
                r#", "params": {{{APP_PARAM}}}, "options": {{"@acme/app": {{"appName": {{"from": "appName"}}}}}}"#
            ),
        ),
    );
    worker.write(
        "src/wrangler.jsonc.assemblr.tmpl",
        "{\n  \"name\": \"{{ appName }}\"\n}\n",
    );
    worker.publish("1.0.0");
    fixture.register(&worker);
}

const STACK_EXTRA: &str = r#", "params": {
    "appName": {"type": "string", "description": "The application name.", "example": "demo-app"},
    "database": {"type": "string", "description": "The database.", "default": "app"}
  },
  "options": {
    "@acme/app": {"appName": {"from": "appName"}},
    "@acme/worker": {"appName": {"from": "appName"}}
  }"#;

/// The stack: depends on both, exposes `appName` once and `database`, and
/// maps the first to both dependencies. Not yet built.
fn stack(fixture: &Fixture) -> Repo {
    let stack = fixture.repo("stack");
    stack.write(
        "manifest.jsonc",
        &manifest(
            "@acme/stack",
            "1.0.0",
            &[("@acme/app", "^1"), ("@acme/worker", "^1")],
            STACK_EXTRA,
        ),
    );
    stack.write(
        "src/.dev.vars.example.assemblr.tmpl",
        "DATABASE_URL=postgres://localhost/{{ database }}\n",
    );
    stack
}

fn project(fixture: &Fixture, name: &str, declaration: &str) -> PathBuf {
    succeed(&fixture.assemblr(&fixture.dir, &["init", name]));
    let project = fixture.dir.join(name);
    git(&project, &["init", "-q"]);
    fs::write(project.join("assemblr.jsonc"), declaration).unwrap();
    project
}

fn json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn succeed(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        stdout(output),
        stderr(output)
    );
}

fn failure(output: &Output) -> String {
    assert_eq!(
        output.status.code(),
        Some(1),
        "stdout: {}\nstderr: {}",
        stdout(output),
        stderr(output)
    );
    stderr(output)
}

#[test]
fn a_unit_with_a_required_parameter_is_built_checked_and_published_with_its_example() {
    let fixture = Fixture::new("publish");
    base_units(&fixture);
    let stack = stack(&fixture);
    // Built with the example: `demo-app` reaches the dependencies through
    // the mappings, and `database` takes its default.
    succeed(&fixture.build(&stack));
    assert_eq!(
        json(&stack.path.join("build/package.json"))["name"],
        "demo-app"
    );
    assert_eq!(
        json(&stack.path.join("build/wrangler.jsonc"))["name"],
        "demo-app"
    );
    assert_eq!(
        fs::read_to_string(stack.path.join("build/.dev.vars.example")).unwrap(),
        "DATABASE_URL=postgres://localhost/app\n"
    );
    stack.git(&["add", "-A"]);
    stack.git(&["commit", "-q", "-m", "stack"]);
    succeed(&fixture.assemblr(&stack.path, &["unit", "check"]));
    fixture.register(&stack);
    succeed(&fixture.assemblr(&stack.path, &["unit", "publish"]));
    let info = stdout(&fixture.assemblr(&fixture.dir, &["info", "@acme/stack"]));
    assert!(
        info.contains("appName (string; required; example \"demo-app\"): The application name."),
        "{info}"
    );
    assert!(info.contains("@acme/app: appName = from appName"), "{info}");
    let json: Value = serde_json::from_str(&stdout(
        &fixture.assemblr(&fixture.dir, &["info", "@acme/stack", "--json"]),
    ))
    .unwrap();
    assert_eq!(json["params"]["appName"]["example"], "demo-app");
    assert_eq!(json["params"]["appName"]["required"], true);
    assert_eq!(
        json["options"]["@acme/worker"]["appName"]["from"],
        "appName"
    );
}

#[test]
fn a_required_parameter_without_a_default_or_an_example_cannot_be_built() {
    let fixture = Fixture::new("no-example");
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "1.0.0",
            &[],
            r#", "params": {"appName": {"type": "string", "description": "The name."}}"#,
        ),
    );
    app.write("src/a.txt.assemblr.tmpl", "{{ appName }}\n");
    let err = failure(&fixture.build(&app));
    assert!(
        err.contains("declares required parameters without defaults or examples: `appName`"),
        "{err}"
    );
    assert!(
        err.contains("an `example` on the parameter supplies one"),
        "{err}"
    );

    // An example must fit its parameter.
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "1.0.0",
            &[],
            r#", "params": {"mode": {"type": "string", "description": "M.", "allowed": ["a"], "example": "b"}}"#,
        ),
    );
    let err = failure(&fixture.build(&app));
    assert!(err.contains("params.mode.example"), "{err}");
}

#[test]
fn a_consumer_sets_the_name_once_and_every_level_gets_it() {
    let fixture = Fixture::new("consumer");
    base_units(&fixture);
    let stack = stack(&fixture);
    succeed(&fixture.build(&stack));
    stack.git(&["add", "-A"]);
    stack.git(&["commit", "-q", "-m", "stack"]);
    fixture.register(&stack);
    succeed(&fixture.assemblr(&stack.path, &["unit", "publish"]));

    let shop = project(
        &fixture,
        "shop",
        "{\n  \"dependencies\": {\"@acme/stack\": \"^1\"},\n  \"options\": {\"@acme/stack\": {\"appName\": \"acme-portal\", \"database\": \"acme\"}}\n}\n",
    );
    succeed(&fixture.assemblr(&shop, &["compose"]));
    assert_eq!(json(&shop.join("package.json"))["name"], "acme-portal");
    assert_eq!(json(&shop.join("wrangler.jsonc"))["name"], "acme-portal");
    assert_eq!(
        fs::read_to_string(shop.join(".dev.vars.example")).unwrap(),
        "DATABASE_URL=postgres://localhost/acme\n"
    );

    // Changing the name is one edit, reconciled like any change.
    fs::write(
        shop.join("assemblr.jsonc"),
        "{\n  \"dependencies\": {\"@acme/stack\": \"^1\"},\n  \"options\": {\"@acme/stack\": {\"appName\": \"acme-console\", \"database\": \"acme\"}}\n}\n",
    )
    .unwrap();
    succeed(&fixture.assemblr(&shop, &["compose"]));
    assert_eq!(json(&shop.join("package.json"))["name"], "acme-console");
    assert_eq!(json(&shop.join("wrangler.jsonc"))["name"], "acme-console");
}

#[test]
fn add_reads_an_option_value_by_the_parameters_type() {
    let fixture = Fixture::new("typed-option");
    base_units(&fixture);
    let shop = project(
        &fixture,
        "shop",
        "{\n  \"dependencies\": {},\n  \"options\": {}\n}\n",
    );

    // Text that reads as JSON stays a string for a string parameter.
    succeed(&fixture.assemblr(&shop, &["add", "@acme/app", "--option", "appName=true"]));
    assert_eq!(
        json(&shop.join("assemblr.jsonc"))["options"]["@acme/app"]["appName"],
        "true"
    );
    assert_eq!(json(&shop.join("package.json"))["name"], "true");
    succeed(&fixture.assemblr(&shop, &["add", "@acme/app", "--option", "appName=8080"]));
    assert_eq!(json(&shop.join("package.json"))["name"], "8080");

    // A quoted JSON string still gives its content.
    succeed(&fixture.assemblr(&shop, &["add", "@acme/app", "--option", "appName=\"acme\""]));
    assert_eq!(json(&shop.join("package.json"))["name"], "acme");
}

#[test]
fn the_example_is_never_a_consumer_value() {
    let fixture = Fixture::new("example-not-used");
    base_units(&fixture);
    let stack = stack(&fixture);
    succeed(&fixture.build(&stack));
    stack.git(&["add", "-A"]);
    stack.git(&["commit", "-q", "-m", "stack"]);
    fixture.register(&stack);
    succeed(&fixture.assemblr(&stack.path, &["unit", "publish"]));

    let shop = project(
        &fixture,
        "shop",
        "{\n  \"dependencies\": {\"@acme/stack\": \"^1\"},\n  \"options\": {}\n}\n",
    );
    let err = failure(&fixture.assemblr(&shop, &["compose"]));
    assert!(
        err.contains("@acme/stack 1.0.0 needs a value for `appName`, which has no default"),
        "{err}"
    );
    assert!(err.contains("(example: \"demo-app\")"), "{err}");
    // Only the stack is asked: the dependencies' parameters come through it.
    assert!(!err.contains("@acme/worker 1.0.0 needs a value"), "{err}");
    assert!(!shop.join("package.json").exists());
}

#[test]
fn a_value_that_disagrees_with_a_mapping_is_a_conflict_that_names_it() {
    let fixture = Fixture::new("conflict");
    base_units(&fixture);
    let stack = stack(&fixture);
    succeed(&fixture.build(&stack));
    stack.git(&["add", "-A"]);
    stack.git(&["commit", "-q", "-m", "stack"]);
    fixture.register(&stack);
    succeed(&fixture.assemblr(&stack.path, &["unit", "publish"]));

    // The consumer also selects the worker and names it differently.
    let shop = project(
        &fixture,
        "shop",
        "{\n  \"dependencies\": {\"@acme/stack\": \"^1\", \"@acme/worker\": \"^1\"},\n  \"options\": {\"@acme/stack\": {\"appName\": \"acme\"}, \"@acme/worker\": {\"appName\": \"other\"}}\n}\n",
    );
    let err = failure(&fixture.assemblr(&shop, &["compose"]));
    assert!(
        err.contains("options conflict on parameter `appName` of @acme/worker 1.0.0"),
        "{err}"
    );
    assert!(
        err.contains("\"acme\" from @acme/stack 1.0.0 (mapped from its `appName`)"),
        "{err}"
    );
    assert!(err.contains("\"other\" from the root"), "{err}");
    assert!(
        err.contains("set `appName` of that unit in assemblr.jsonc `options` rather than this parameter directly"),
        "{err}"
    );
    assert!(!shop.join("package.json").exists());

    // Agreeing is accepted: the two paths carry the same value.
    fs::write(
        shop.join("assemblr.jsonc"),
        "{\n  \"dependencies\": {\"@acme/stack\": \"^1\", \"@acme/worker\": \"^1\"},\n  \"options\": {\"@acme/stack\": {\"appName\": \"acme\"}, \"@acme/worker\": {\"appName\": \"acme\"}}\n}\n",
    )
    .unwrap();
    succeed(&fixture.assemblr(&shop, &["compose"]));
    assert_eq!(json(&shop.join("package.json"))["name"], "acme");
}

#[test]
fn a_project_cannot_write_a_mapping_and_an_incompatible_mapping_is_refused_at_check() {
    let fixture = Fixture::new("refusals");
    base_units(&fixture);
    let shop = project(
        &fixture,
        "shop",
        "{\n  \"dependencies\": {\"@acme/worker\": \"^1\"},\n  \"options\": {\"@acme/worker\": {\"appName\": {\"from\": \"x\"}}}\n}\n",
    );
    let err = failure(&fixture.assemblr(&shop, &["compose"]));
    assert!(
        err.contains("a mapping names a parameter of a unit's own `params`"),
        "{err}"
    );

    // A free string cannot configure an enumerated one.
    let lib = fixture.repo("lib");
    lib.write(
        "manifest.jsonc",
        &manifest(
            "@acme/lib",
            "1.0.0",
            &[],
            r#", "params": {"mode": {"type": "string", "description": "M.", "allowed": ["dev", "prod"], "default": "dev"}}"#,
        ),
    );
    lib.publish("1.0.0");
    fixture.register(&lib);
    let top = fixture.repo("top");
    top.write(
        "manifest.jsonc",
        &manifest(
            "@acme/top",
            "1.0.0",
            &[("@acme/lib", "^1")],
            r#", "params": {"mode": {"type": "string", "description": "M.", "default": "dev"}},
               "options": {"@acme/lib": {"mode": {"from": "mode"}}}"#,
        ),
    );
    top.write("src/top.txt", "top");
    let err = failure(&fixture.build(&top));
    assert!(
        err.contains("@acme/top 0.0.0")
            || err.contains("maps its parameter `mode` to `mode` of @acme/lib"),
        "{err}"
    );
    assert!(
        err.contains("a free string may hold values the dependency's parameter does not allow"),
        "{err}"
    );
}

#[test]
fn a_pattern_refuses_a_value_that_does_not_match_wherever_it_comes_from() {
    let fixture = Fixture::new("pattern");
    let base = fixture.repo("base");
    base.write(
        "manifest.jsonc",
        &manifest(
            "@acme/base",
            "1.0.0",
            &[],
            r#", "params": {"name": {"type": "string", "description": "The package name.", "pattern": "^[a-z][a-z0-9-]*$", "example": "my-app"}}"#,
        ),
    );
    base.write("src/name.txt.assemblr.tmpl", "{{ name }}\n");
    succeed(&fixture.build(&base));
    base.publish("1.0.0");
    fixture.register(&base);

    // A consumer's value.
    let shop = project(
        &fixture,
        "shop",
        "{\"dependencies\": {}, \"options\": {}}\n",
    );
    let err =
        failure(&fixture.assemblr(&shop, &["add", "@acme/base", "--option", "name=Bob's App"]));
    assert!(
        err.contains("\"Bob's App\" does not match the pattern `^[a-z][a-z0-9-]*$`"),
        "{err}"
    );
    assert!(err.contains("a value that matches the pattern"), "{err}");
    assert!(!shop.join("name.txt").exists());
    succeed(&fixture.assemblr(&shop, &["add", "@acme/base", "--option", "name=bobs-app"]));
    assert_eq!(
        fs::read_to_string(shop.join("name.txt")).unwrap(),
        "bobs-app\n"
    );

    // A literal a unit sets for its dependency.
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[("@acme/base", "^1.0.0")],
            r#", "options": {"@acme/base": {"name": "Not Kebab"}}"#,
        ),
    );
    let err = failure(&fixture.build(&app));
    assert!(
        err.contains("\"Not Kebab\" does not match the pattern"),
        "{err}"
    );
}

#[test]
fn an_inherited_parameter_takes_the_dependency_s_definition_and_passes_the_value_on() {
    let fixture = Fixture::new("inherited");
    let base = fixture.repo("base");
    base.write(
        "manifest.jsonc",
        &manifest(
            "@acme/base",
            "1.0.0",
            &[],
            r#", "params": {"title": {"type": "string", "description": "The site title.", "pattern": "^[A-Z]", "example": "Demo"}}"#,
        ),
    );
    base.write("src/base.txt.assemblr.tmpl", "{{ title }}\n");
    succeed(&fixture.build(&base));
    base.publish("1.0.0");
    fixture.register(&base);

    // The unit re-exposes the parameter without copying its definition, and
    // builds with the dependency's example.
    let web = fixture.repo("web");
    web.write(
        "manifest.jsonc",
        &manifest(
            "@acme/web",
            "1.0.0",
            &[("@acme/base", "^1.0.0")],
            r#", "params": {"title": {"from": "@acme/base"}}"#,
        ),
    );
    web.write("src/web.html.assemblr.tmpl", "<h1>{{ title | e }}</h1>\n");
    succeed(&fixture.build(&web));
    assert_eq!(
        fs::read_to_string(web.path.join("build/web.html")).unwrap(),
        "<h1>Demo</h1>\n"
    );
    web.publish("1.0.0");
    fixture.register(&web);

    let info = fixture.assemblr(&fixture.dir, &["info", "@acme/web"]);
    succeed(&info);
    let out = String::from_utf8_lossy(&info.stdout).into_owned();
    assert!(
        out.contains("title (inherited from @acme/base; `assemblr info @acme/base` describes it)"),
        "{out}"
    );
    let info = fixture.assemblr(&fixture.dir, &["info", "@acme/web", "--json"]);
    let info: Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(info["params"]["title"]["from"], "@acme/base");

    // The consumer sets it once, on the unit it selected, checked against
    // the dependency's definition.
    let shop = project(
        &fixture,
        "shop",
        "{\"dependencies\": {}, \"options\": {}}\n",
    );
    let err = failure(&fixture.assemblr(&shop, &["add", "@acme/web", "--option", "title=shop"]));
    assert!(err.contains("does not match the pattern `^[A-Z]`"), "{err}");
    succeed(&fixture.assemblr(
        &shop,
        &["add", "@acme/web", "--option", "title=Bob's <Shop>"],
    ));
    assert_eq!(
        fs::read_to_string(shop.join("web.html")).unwrap(),
        "<h1>Bob&#39;s &lt;Shop&gt;</h1>\n"
    );
    assert_eq!(
        fs::read_to_string(shop.join("base.txt")).unwrap(),
        "Bob's <Shop>\n"
    );
}
