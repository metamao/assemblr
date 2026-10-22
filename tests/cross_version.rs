//! Amendments applied to a provider version other than the one they were
//! taken from, composed by the real binary: the release cascade of the
//! reference scenario, where a provider's release stopped every unit that
//! amends it from composing until each had published again.
//!
//! The scenarios use a `package.json`, a guidance file, and a templated
//! Wrangler configuration that a provider changes between releases while
//! two units amend them. Every test gives the binary its own registry and
//! home.

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

/// The `amendments` member for copies of `file` from `provider` at `base`.
fn amending(files: &[&str], provider: &str, base: &str) -> String {
    let entries: Vec<String> = files
        .iter()
        .map(|file| format!(r#""{file}": {{"unit": "{provider}", "base": "{base}"}}"#))
        .collect();
    format!(", \"amendments\": {{{}}}", entries.join(", "))
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

    /// Commits everything and tags it `v<version>`.
    fn publish(&self, version: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", version]);
        self.git(&["tag", &format!("v{version}")]);
    }

    fn lockfile_for(&self, provider: &Repo, identity: &str, version: &str) -> String {
        let tag = format!("v{version}");
        format!(
            "{{\n  \"format\": 1,\n  \"units\": {{\n    \"{identity}\": {{\n      \"commit\": \
             \"{}\",\n      \"tree\": \"{}\",\n      \"version\": \"{version}\"\n    }}\n  }}\n}}\n",
            provider.git(&["rev-parse", &tag]),
            provider.git(&["rev-parse", &format!("{tag}^{{tree}}")])
        )
    }
}

const PACKAGE: &str = "{\n  \"name\": \"worker\",\n  \"scripts\": {\n    \"dev\": \"wrangler dev\",\n    \"deploy\": \"wrangler deploy\"\n  },\n  \"dependencies\": {\n    \"hono\": \"^4\"\n  }\n}\n";

const WRANGLER: &str =
    "{\n  \"name\": \"{{ name }}\",\n  \"compatibility_date\": \"2026-01-01\"\n}\n";

const GUIDE: &str = "# Agent instructions\n\nRead the README first.\n\n## Rules\n\n- Keep changes small.\n- Run the checks.\n\n## Reporting\n\nSay what you observed.\n";

fn worker_manifest(version: &str, params: &str) -> String {
    manifest(
        "@acme/worker",
        version,
        &[],
        &format!(", \"params\": {{{params}}}"),
    )
}

const NAME_PARAM: &str =
    r#""name": {"type": "string", "description": "The name.", "default": "worker"}"#;

/// The provider `@acme/worker` 1.0.0, registered.
fn worker(fixture: &Fixture) -> Repo {
    let worker = fixture.repo("worker");
    worker.write("manifest.jsonc", &worker_manifest("1.0.0", NAME_PARAM));
    worker.write("src/package.json", PACKAGE);
    worker.write("src/wrangler.jsonc.assemblr.tmpl", WRANGLER);
    worker.write("src/AGENTS.md", GUIDE);
    worker.write("src/README.md", "worker\n");
    worker.publish("1.0.0");
    fixture.register(&worker);
    worker
}

/// Publishes `version` of the provider after `change`, and indexes it.
fn release(
    fixture: &Fixture,
    worker: &Repo,
    version: &str,
    params: &str,
    change: impl FnOnce(&Repo),
) {
    worker.write("manifest.jsonc", &worker_manifest(version, params));
    change(worker);
    worker.publish(version);
    succeed(&fixture.assemblr(&fixture.dir, &["registry", "sync"]));
}

/// Publishes the amender `name` 1.0.0, taken from the provider's `base`,
/// amending `copies`, and registers it.
fn amender(
    fixture: &Fixture,
    worker: &Repo,
    name: &str,
    base: &str,
    copies: &[(&str, &str)],
) -> Repo {
    let unit = fixture.repo(name);
    let files: Vec<&str> = copies.iter().map(|(file, _)| *file).collect();
    unit.write(
        "manifest.jsonc",
        &manifest(
            &format!("@acme/{name}"),
            "1.0.0",
            &[("@acme/worker", "^1")],
            &amending(&files, "@acme/worker", base),
        ),
    );
    for (file, contents) in copies {
        unit.write(&format!("amendments/{file}"), contents);
    }
    unit.write(&format!("src/{name}.txt"), name);
    unit.write(
        "assemblr-lock.json",
        &unit.lockfile_for(worker, "@acme/worker", base),
    );
    unit.publish("1.0.0");
    fixture.register(&unit);
    unit
}

fn db_package() -> String {
    PACKAGE
        .replace(
            "    \"deploy\": \"wrangler deploy\"\n",
            "    \"deploy\": \"wrangler deploy\",\n    \"db:push\": \"drizzle-kit push\"\n",
        )
        .replace(
            "    \"hono\": \"^4\"\n",
            "    \"drizzle-orm\": \"^0.44\",\n    \"hono\": \"^4\"\n",
        )
}

fn api_package() -> String {
    PACKAGE.replace(
        "    \"deploy\": \"wrangler deploy\"\n",
        "    \"deploy\": \"wrangler deploy\",\n    \"test\": \"vitest\"\n",
    )
}

fn project(fixture: &Fixture, name: &str, declaration: &str) -> PathBuf {
    succeed(&fixture.assemblr(&fixture.dir, &["init", name]));
    let project = fixture.dir.join(name);
    git(&project, &["init", "-q"]);
    fs::write(project.join("assemblr.jsonc"), declaration).unwrap();
    project
}

const BOTH: &str =
    "{\n  \"dependencies\": {\"@acme/db\": \"^1\", \"@acme/api\": \"^1\"},\n  \"options\": {}\n}\n";

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
fn provider_releases_that_change_other_files_and_values_need_no_new_amender_version() {
    let fixture = Fixture::new("releases");
    let worker = worker(&fixture);
    amender(
        &fixture,
        &worker,
        "db",
        "1.0.0",
        &[("package.json", &db_package())],
    );
    amender(
        &fixture,
        &worker,
        "api",
        "1.0.0",
        &[("package.json", &api_package())],
    );

    // 1.1.0 adds a script and raises a dependency, which nobody amended, and
    // changes a file nobody amends.
    release(&fixture, &worker, "1.1.0", NAME_PARAM, |worker| {
        worker.write(
            "src/package.json",
            &PACKAGE
                .replace(
                    "    \"deploy\": \"wrangler deploy\"\n",
                    "    \"deploy\": \"wrangler deploy\",\n    \"lint\": \"eslint .\"\n",
                )
                .replace("\"hono\": \"^4\"", "\"hono\": \"^4.1\""),
        );
        worker.write("src/README.md", "worker 1.1\n");
    });
    let shop = project(&fixture, "shop", BOTH);
    succeed(&fixture.assemblr(&shop, &["compose"]));
    let package = json(&shop.join("package.json"));
    assert_eq!(package["scripts"]["lint"], "eslint .");
    assert_eq!(package["scripts"]["db:push"], "drizzle-kit push");
    assert_eq!(package["scripts"]["test"], "vitest");
    assert_eq!(package["dependencies"]["hono"], "^4.1");
    assert_eq!(package["dependencies"]["drizzle-orm"], "^0.44");
    assert_eq!(
        fs::read_to_string(shop.join("README.md")).unwrap(),
        "worker 1.1\n"
    );
    let lock = fs::read_to_string(shop.join("assemblr-lock.json")).unwrap();
    assert!(
        lock.contains("\"@acme/worker\"") && lock.contains("\"1.1.0\""),
        "{lock}"
    );

    // 1.2.0 changes only files the amenders did not touch: the guidance
    // file nobody amends, and the template.
    release(&fixture, &worker, "1.2.0", NAME_PARAM, |worker| {
        worker.write("src/AGENTS.md", &GUIDE.replace("# Agent", "# Coding agent"));
        worker.write(
            "src/package.json",
            &fs::read_to_string(worker.path.join("src/package.json")).unwrap(),
        );
    });
    succeed(&fixture.assemblr(&shop, &["add", "@acme/worker@^1.2"]));
    assert!(
        fs::read_to_string(shop.join("AGENTS.md"))
            .unwrap()
            .starts_with("# Coding agent")
    );
    let package = json(&shop.join("package.json"));
    assert_eq!(package["scripts"]["test"], "vitest");
    assert_eq!(package["scripts"]["lint"], "eslint .");
}

#[test]
fn a_provider_change_to_a_value_an_amender_changed_is_a_collision_and_changes_nothing() {
    let fixture = Fixture::new("collision");
    let worker = worker(&fixture);
    let api = PACKAGE.replace(
        "\"dev\": \"wrangler dev\"",
        "\"dev\": \"wrangler dev --remote\"",
    );
    amender(
        &fixture,
        &worker,
        "db",
        "1.0.0",
        &[("package.json", &db_package())],
    );
    amender(&fixture, &worker, "api", "1.0.0", &[("package.json", &api)]);
    let shop = project(&fixture, "shop", BOTH);
    succeed(&fixture.assemblr(&shop, &["compose"]));
    let before = tree(&shop);

    release(&fixture, &worker, "1.1.0", NAME_PARAM, |worker| {
        worker.write(
            "src/package.json",
            &PACKAGE.replace(
                "\"dev\": \"wrangler dev\"",
                "\"dev\": \"wrangler dev --local\"",
            ),
        );
    });
    let err = failure(&fixture.assemblr(&shop, &["add", "@acme/worker@^1.1"]));
    assert!(err.contains("/scripts/dev"), "{err}");
    assert!(err.contains("@acme/api"), "{err}");
    assert!(
        err.contains("the amendment of @acme/api was taken from @acme/worker 1.0.0"),
        "{err}"
    );
    // The way out the hint names: stay on the version the amendment was
    // taken from.
    assert!(err.contains("`assemblr add @acme/worker@=1.0.0`"), "{err}");
    assert_eq!(tree(&shop), before);

    // Following it pins the provider, and an update of every unit then
    // leaves it where it is.
    succeed(&fixture.assemblr(&shop, &["add", "@acme/worker@=1.0.0"]));
    let updated = fixture.assemblr(&shop, &["update"]);
    succeed(&updated);
    assert!(
        stdout(&updated).contains("Nothing to update"),
        "{}",
        stdout(&updated)
    );
    assert!(
        fs::read_to_string(shop.join("assemblr.jsonc"))
            .unwrap()
            .contains("\"@acme/worker\": \"=1.0.0\"")
    );
}

#[test]
fn amenders_taken_from_different_versions_stay_independent_and_the_result_is_deterministic() {
    let build = |reversed: bool| {
        let fixture = Fixture::new(if reversed { "bases-reversed" } else { "bases" });
        let worker = worker(&fixture);
        // The provider releases 1.1.0 before the second amender is taken
        // from it.
        let changed = PACKAGE.replace(
            "    \"deploy\": \"wrangler deploy\"\n",
            "    \"deploy\": \"wrangler deploy\",\n    \"lint\": \"eslint .\"\n",
        );
        let db = db_package();
        let api = changed.replace(
            "    \"lint\": \"eslint .\"\n",
            "    \"lint\": \"eslint .\",\n    \"test\": \"vitest\"\n",
        );
        let order: [&str; 2] = if reversed {
            ["api", "db"]
        } else {
            ["db", "api"]
        };
        for name in order {
            if name == "db" {
                amender(&fixture, &worker, "db", "1.0.0", &[("package.json", &db)]);
            } else {
                release(&fixture, &worker, "1.1.0", NAME_PARAM, |worker| {
                    worker.write("src/package.json", &changed);
                });
                amender(&fixture, &worker, "api", "1.1.0", &[("package.json", &api)]);
            }
        }
        // 1.2.0 is what the consumer resolves; both amendments follow it.
        release(&fixture, &worker, "1.2.0", NAME_PARAM, |worker| {
            worker.write("src/README.md", "worker 1.2\n");
        });
        let shop = project(&fixture, "shop", BOTH);
        succeed(&fixture.assemblr(&shop, &["compose"]));
        fs::read_to_string(shop.join("package.json")).unwrap()
    };
    let one = build(false);
    let package: Value = serde_json::from_str(&one).unwrap();
    assert_eq!(package["scripts"]["lint"], "eslint .");
    assert_eq!(package["scripts"]["db:push"], "drizzle-kit push");
    assert_eq!(package["scripts"]["test"], "vitest");
    assert_eq!(package["dependencies"]["drizzle-orm"], "^0.44");
    // Neither amendment sets the other's base or takes precedence, and the
    // order the units were published in changes nothing.
    assert_eq!(build(true), one);
}

#[test]
fn templated_amendments_follow_the_composition_s_values_and_stop_when_a_parameter_changes_meaning()
{
    let fixture = Fixture::new("templated");
    let worker = worker(&fixture);
    let wrangler = WRANGLER.replace(
        "  \"compatibility_date\"",
        "  \"main\": \"./dist/server.mjs\",\n  \"compatibility_date\"",
    );
    amender(
        &fixture,
        &worker,
        "db",
        "1.0.0",
        &[("wrangler.jsonc.assemblr.tmpl", &wrangler)],
    );
    // 1.1.0 keeps the parameter and changes the template elsewhere.
    release(&fixture, &worker, "1.1.0", NAME_PARAM, |worker| {
        worker.write(
            "src/wrangler.jsonc.assemblr.tmpl",
            &WRANGLER.replace("2026-01-01", "2026-06-01"),
        );
    });
    let shop = project(
        &fixture,
        "shop",
        "{\n  \"dependencies\": {\"@acme/db\": \"^1\", \"@acme/worker\": \"^1\"},\n  \"options\": {\"@acme/worker\": {\"name\": \"acme\"}}\n}\n",
    );
    succeed(&fixture.assemblr(&shop, &["compose"]));
    let wrangler = json(&shop.join("wrangler.jsonc"));
    assert_eq!(wrangler["name"], "acme");
    assert_eq!(wrangler["compatibility_date"], "2026-06-01");
    assert_eq!(wrangler["main"], "./dist/server.mjs");

    // 1.2.0 renames the parameter, so the amendment's template names one the
    // resolved version no longer declares: a failure that says so, not a
    // fallback to an old default.
    release(
        &fixture,
        &worker,
        "1.2.0",
        r#""title": {"type": "string", "description": "The name.", "default": "worker"}"#,
        |worker| {
            worker.write(
                "src/wrangler.jsonc.assemblr.tmpl",
                &WRANGLER
                    .replace("2026-01-01", "2026-06-01")
                    .replace("{{ name }}", "{{ title }}"),
            );
        },
    );
    // The consumer drops the option the renamed parameter replaced.
    fs::write(
        shop.join("assemblr.jsonc"),
        "{\n  \"dependencies\": {\"@acme/db\": \"^1\", \"@acme/worker\": \"^1.2\"},\n  \"options\": {}\n}\n",
    )
    .unwrap();
    let before = tree(&shop);
    let err = failure(&fixture.assemblr(&shop, &["compose"]));
    assert!(
        err.contains("it names parameter `name`, which is declared as a string in @acme/worker 1.0.0 and not declared in @acme/worker 1.2.0"),
        "{err}"
    );
    assert_eq!(tree(&shop), before);

    // A parameter that keeps its name and changes kind fails the same way.
    let retyped = Fixture::new("retyped");
    let worker = self::worker(&retyped);
    amender(
        &retyped,
        &worker,
        "db",
        "1.0.0",
        &[("wrangler.jsonc.assemblr.tmpl", &wrangler_text())],
    );
    release(
        &retyped,
        &worker,
        "1.1.0",
        r#""name": {"type": "integer", "description": "The name.", "default": 1}"#,
        |worker| {
            worker.write(
                "src/wrangler.jsonc.assemblr.tmpl",
                &WRANGLER.replace("2026-01-01", "2026-06-01"),
            );
        },
    );
    let shop = project(
        &retyped,
        "shop",
        "{\n  \"dependencies\": {\"@acme/db\": \"^1\"},\n  \"options\": {}\n}\n",
    );
    let err = failure(&retyped.assemblr(&shop, &["compose"]));
    assert!(
        err.contains("declared as an integer in @acme/worker 1.1.0") || err.contains("integer"),
        "{err}"
    );
    assert!(!shop.join("wrangler.jsonc").exists());
}

fn wrangler_text() -> String {
    WRANGLER.replace(
        "  \"compatibility_date\"",
        "  \"main\": \"./dist/server.mjs\",\n  \"compatibility_date\"",
    )
}

#[test]
fn the_declared_range_decides_which_provider_versions_are_eligible() {
    let fixture = Fixture::new("range");
    let worker = worker(&fixture);
    amender(
        &fixture,
        &worker,
        "db",
        "1.0.0",
        &[("package.json", &db_package())],
    );
    release(&fixture, &worker, "2.0.0", NAME_PARAM, |_| {});
    // The amender accepts ^1, so a consumer that needs the provider at ^2
    // gets a resolution error, not an amendment applied outside its range.
    let shop = project(
        &fixture,
        "shop",
        "{\n  \"dependencies\": {\"@acme/db\": \"^1\", \"@acme/worker\": \"^2\"},\n  \"options\": {}\n}\n",
    );
    let err = failure(&fixture.assemblr(&shop, &["compose"]));
    assert!(err.contains("@acme/worker"), "{err}");
    assert!(!shop.join("package.json").exists());
}

#[test]
fn a_base_the_amender_s_lockfile_misidentifies_is_still_an_integrity_conflict_across_versions() {
    let fixture = Fixture::new("integrity");
    let worker = worker(&fixture);
    let db = amender(
        &fixture,
        &worker,
        "db",
        "1.0.0",
        &[("package.json", &db_package())],
    );
    release(&fixture, &worker, "1.1.0", NAME_PARAM, |worker| {
        worker.write("src/README.md", "worker 1.1\n");
    });
    // The amender is published again with a lockfile that names other
    // content for the base.
    let tree_id = worker.git(&["rev-parse", "v1.0.0^{tree}"]);
    let lock = db
        .read("assemblr-lock.json")
        .replace(&tree_id, &"0".repeat(40));
    db.write("assemblr-lock.json", &lock);
    db.write(
        "manifest.jsonc",
        &db.read("manifest.jsonc")
            .replace("\"1.0.0\",", "\"1.0.1\","),
    );
    db.publish("1.0.1");
    succeed(&fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    let shop = project(
        &fixture,
        "shop",
        "{\n  \"dependencies\": {\"@acme/db\": \"^1.0.1\"},\n  \"options\": {}\n}\n",
    );
    let err = failure(&fixture.assemblr(&shop, &["compose"]));
    assert!(err.contains("integrity conflict"), "{err}");
    assert!(!shop.join("package.json").exists());
}

#[test]
fn a_file_that_changed_without_a_merge_is_refused_explicitly() {
    let fixture = Fixture::new("refused-yaml");
    let worker = worker(&fixture);
    worker.write("src/config.yaml", "a: 1\n");
    worker.write("manifest.jsonc", &worker_manifest("1.0.1", NAME_PARAM));
    worker.publish("1.0.1");
    succeed(&fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    amender(
        &fixture,
        &worker,
        "db",
        "1.0.1",
        &[("config.yaml", "a: 1\nb: 2\n")],
    );
    release(&fixture, &worker, "1.1.0", NAME_PARAM, |worker| {
        worker.write("src/config.yaml", "a: 2\n");
    });
    let shop = project(
        &fixture,
        "shop",
        "{\n  \"dependencies\": {\"@acme/db\": \"^1\"},\n  \"options\": {}\n}\n",
    );
    let err = failure(&fixture.assemblr(&shop, &["compose"]));
    assert!(
        err.contains("its format has no merge (it is applied across versions only to a file that is unchanged)"),
        "{err}"
    );
    assert!(err.contains("`assemblr add @acme/worker@=1.0.1`"), "{err}");
    assert!(!shop.join("config.yaml").exists());
}

#[test]
fn a_file_the_provider_no_longer_has_is_refused_with_a_way_out() {
    let fixture = Fixture::new("refused-removed");
    let worker = worker(&fixture);
    amender(
        &fixture,
        &worker,
        "db",
        "1.0.0",
        &[("AGENTS.md", &GUIDE.replace("# Agent", "# Mine"))],
    );
    release(&fixture, &worker, "1.1.0", NAME_PARAM, |worker| {
        fs::remove_file(worker.path.join("src/AGENTS.md")).unwrap();
    });
    let shop = project(
        &fixture,
        "shop",
        "{\n  \"dependencies\": {\"@acme/db\": \"^1\"},\n  \"options\": {}\n}\n",
    );
    let err = failure(&fixture.assemblr(&shop, &["compose"]));
    assert!(
        err.contains("amends a file that @acme/worker 1.1.0 does not provide"),
        "{err}"
    );
    assert!(
        err.contains("select an earlier version of @acme/worker that provides it"),
        "{err}"
    );
    assert!(!shop.join("AGENTS.md").exists());
}

/// Every file under `dir`, `.git` excepted.
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

#[test]
fn a_provider_section_and_an_amender_section_added_at_one_place_are_both_kept_provider_first() {
    // The guidance-file case of the reference scenario: the provider's
    // release adds a section at the end of `AGENTS.md`, where an amender
    // taken from the earlier version adds its own.
    let fixture = Fixture::new("appended-sections");
    let worker = worker(&fixture);
    let database = format!("{GUIDE}\n## Database\n\nUse DATABASE_URL.\n");
    let db = amender(
        &fixture,
        &worker,
        "db",
        "1.0.0",
        &[("AGENTS.md", &database)],
    );
    let security = format!("{GUIDE}\n## Security\n\nNever commit secrets.\n");
    release(&fixture, &worker, "1.1.0", NAME_PARAM, |worker| {
        worker.write("src/AGENTS.md", &security);
    });
    let expected = format!("{security}\n## Database\n\nUse DATABASE_URL.\n");

    // A consumer composes the release without a new amender version, and
    // nothing is reported for the order the rule fixes.
    let declaration = "{\n  \"dependencies\": {\"@acme/db\": \"^1\"},\n  \"options\": {}\n}\n";
    let shop = project(&fixture, "shop", declaration);
    let output = fixture.assemblr(&shop, &["compose"]);
    succeed(&output);
    assert!(!stderr(&output).contains("warning"), "{}", stderr(&output));
    let crossed = "AGENTS.md: the amendment of @acme/db was taken from @acme/worker 1.0.0 and is \
                   applied to @acme/worker 1.1.0";
    assert!(!stderr(&output).contains(crossed), "{}", stderr(&output));
    // `--verbose` shows it, also when nothing is written.
    let verbose = fixture.assemblr(&shop, &["compose", "--verbose"]);
    succeed(&verbose);
    assert!(
        stdout(&verbose).contains("Nothing to compose"),
        "{}",
        stdout(&verbose)
    );
    assert!(
        stderr(&verbose).contains(&format!("note: {crossed}")),
        "{}",
        stderr(&verbose)
    );
    assert_eq!(
        fs::read_to_string(shop.join("AGENTS.md")).unwrap(),
        expected
    );
    assert!(
        fs::read_to_string(shop.join("assemblr-lock.json"))
            .unwrap()
            .contains("\"1.1.0\"")
    );

    // The amender's author takes the release with `unit update`, which
    // rebases the copy the same way, and the version published from it
    // composes to the same file.
    succeed(&fixture.assemblr(&db.path, &["unit", "update"]));
    assert_eq!(db.read("amendments/AGENTS.md"), expected);
    assert!(db.read("manifest.jsonc").contains("\"base\": \"1.1.0\""));
    let manifest = db.read("manifest.jsonc");
    db.write(
        "manifest.jsonc",
        &manifest.replacen("\"version\": \"1.0.0\"", "\"version\": \"1.0.1\"", 1),
    );
    succeed(&fixture.assemblr(&db.path, &["unit", "check"]));
    db.publish("1.0.1");
    succeed(&fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    let again = project(&fixture, "again", declaration);
    succeed(&fixture.assemblr(&again, &["compose"]));
    assert!(
        fs::read_to_string(again.join("assemblr-lock.json"))
            .unwrap()
            .contains("\"1.0.1\"")
    );
    assert_eq!(
        fs::read_to_string(again.join("AGENTS.md")).unwrap(),
        expected
    );

    // A change to a line the provider changed still collides.
    release(&fixture, &worker, "1.2.0", NAME_PARAM, |worker| {
        worker.write(
            "src/AGENTS.md",
            &security.replace("Keep changes small.", "Keep changes tiny."),
        );
    });
    amender(
        &fixture,
        &worker,
        "editor",
        "1.0.0",
        &[(
            "AGENTS.md",
            &GUIDE.replace("Keep changes small.", "Keep changes focused."),
        )],
    );
    let clash = project(
        &fixture,
        "clash",
        "{\n  \"dependencies\": {\"@acme/editor\": \"^1\"},\n  \"options\": {}\n}\n",
    );
    let err = failure(&fixture.assemblr(&clash, &["compose"]));
    assert!(
        err.contains("the amendment of @acme/editor and the provider change the same lines"),
        "{err}"
    );
    assert!(!clash.join("AGENTS.md").exists());
}
