//! Several amendments of one JSON or JSONC file, composed by the real
//! binary: `unit build` of a unit whose dependencies amend the same provider
//! file, and of the unit together with them.
//!
//! The scenarios are written from the contracts and from the milestone's
//! files: a `package.json` whose `scripts` and `dependencies` several units
//! extend, and a `wrangler.jsonc` with comments and a `d1_databases` list.
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

    fn build(&self, repo: &Repo) -> Output {
        self.assemblr(&repo.path, &["unit", "build"])
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

const PACKAGE: &str = "{\n  \"name\": \"worker\",\n  \"private\": true,\n  \"scripts\": {\n    \
                       \"dev\": \"wrangler dev\",\n    \"deploy\": \"wrangler deploy\"\n  },\n  \
                       \"dependencies\": {\n    \"hono\": \"^4\"\n  },\n  \"files\": [\"dist\"]\n}\n";

const WRANGLER: &str = "{\n  // The worker's name.\n  \"name\": \"{{ name }}\",\n  \
                        \"compatibility_date\": \"2026-01-01\",\n  \"d1_databases\": [\n    { \
                        \"binding\": \"DB\", \"database_name\": \"main\" },\n  ],\n}\n";

/// A provider `@acme/worker` 1.0.0 with `package.json` and the template
/// `wrangler.jsonc`, registered; `@acme/db` and `@acme/api` published with
/// the given copies, registered; and an unbuilt `@acme/app` depending on
/// both. Each amender's copy is a function of the provider's file.
fn scenario(
    fixture: &Fixture,
    db_copies: &[(&str, &str)],
    api_copies: &[(&str, &str)],
) -> (Repo, Repo) {
    scenario_with(fixture, PACKAGE, db_copies, api_copies)
}

/// [`scenario`] with the provider's `package.json` given.
fn scenario_with(
    fixture: &Fixture,
    package: &str,
    db_copies: &[(&str, &str)],
    api_copies: &[(&str, &str)],
) -> (Repo, Repo) {
    let worker = fixture.repo("worker");
    worker.write(
        "manifest.jsonc",
        &manifest(
            "@acme/worker",
            "1.0.0",
            &[],
            r#", "params": {"name": {"type": "string", "description": "The name.", "default": "worker"}}"#,
        ),
    );
    worker.write("src/package.json", package);
    worker.write("src/wrangler.jsonc.assemblr.tmpl", WRANGLER);
    worker.publish("1.0.0");
    fixture.register(&worker);
    for (name, copies) in [("db", db_copies), ("api", api_copies)] {
        let unit = fixture.repo(name);
        let files: Vec<&str> = copies.iter().map(|(file, _)| *file).collect();
        unit.write(
            "manifest.jsonc",
            &manifest(
                &format!("@acme/{name}"),
                "1.0.0",
                &[("@acme/worker", "=1.0.0")],
                &amending(&files, "@acme/worker", "1.0.0"),
            ),
        );
        for (file, contents) in copies {
            unit.write(&format!("amendments/{file}"), contents);
        }
        unit.write(&format!("src/{name}.txt"), name);
        unit.write(
            "assemblr-lock.json",
            &unit.lockfile_for(&worker, "@acme/worker", "1.0.0"),
        );
        unit.publish("1.0.0");
        fixture.register(&unit);
    }
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[
                ("@acme/db", "^1"),
                ("@acme/api", "^1"),
                ("@acme/worker", "=1.0.0"),
            ],
            "",
        ),
    );
    app.write("src/app.txt", "app");
    (worker, app)
}

#[test]
fn independent_amendments_of_package_json_compose_with_the_provider_s_layout() {
    let fixture = Fixture::new("independent");
    let db = PACKAGE
        .replace("    \"hono\": \"^4\"\n", "    \"drizzle-orm\": \"^0.44\",\n    \"hono\": \"^4\"\n")
        .replace(
            "  \"files\": [\"dist\"]\n",
            "  \"files\": [\"dist\"],\n  \"devDependencies\": {\n    \"drizzle-kit\": \"^0.31\"\n  }\n",
        );
    let api = PACKAGE
        .replace(
            "    \"deploy\": \"wrangler deploy\"\n",
            "    \"deploy\": \"wrangler deploy\",\n    \"test\": \"vitest\"\n",
        )
        .replace("  \"private\": true,\n", "");
    let (_worker, app) = scenario(
        &fixture,
        &[("package.json", &db)],
        &[("package.json", &api)],
    );

    let output = succeeds(fixture.build(&app));
    assert!(
        stdout(&output).contains("with 3 dependencies"),
        "{}",
        stdout(&output)
    );
    assert!(!stderr(&output).contains("warning"), "{}", stderr(&output));
    assert_eq!(
        app.read("build/package.json"),
        "{\n  \"name\": \"worker\",\n  \"scripts\": {\n    \"dev\": \"wrangler dev\",\n    \
         \"deploy\": \"wrangler deploy\",\n    \"test\": \"vitest\"\n  },\n  \"dependencies\": {\n    \
         \"drizzle-orm\": \"^0.44\",\n    \"hono\": \"^4\"\n  },\n  \"files\": [\"dist\"],\n  \
         \"devDependencies\": {\n    \"drizzle-kit\": \"^0.31\"\n  }\n}\n"
    );
    // The template is rendered for the provider and delivered once.
    assert!(
        app.read("build/wrangler.jsonc")
            .contains("\"name\": \"worker\"")
    );

    // The unit's own amendment joins the published ones, with its `own`
    // values, and `unit check` agrees with the build.
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[
                ("@acme/db", "^1"),
                ("@acme/api", "^1"),
                ("@acme/worker", "=1.0.0"),
            ],
            &format!(
                "{}, \"params\": {{\"title\": {{\"type\": \"string\", \"description\": \"T.\", \
                 \"default\": \"My App\"}}}}",
                amending(
                    &["package.json", "wrangler.jsonc.assemblr.tmpl"],
                    "@acme/worker",
                    "1.0.0"
                )
            ),
        ),
    );
    let composed = app.read("build/package.json");
    app.write(
        "amendments/package.json",
        &composed.replace("\"name\": \"worker\"", "\"name\": \"app\""),
    );
    app.write(
        "amendments/wrangler.jsonc.assemblr.tmpl",
        &WRANGLER.replace(
            "  \"compatibility_date\": \"2026-01-01\",\n",
            "  \"compatibility_date\": \"2026-01-01\",\n  \"vars\": { \"TITLE\": \"{{ own.title }}\" },\n",
        ),
    );
    succeeds(fixture.build(&app));
    assert_eq!(
        app.read("build/package.json"),
        composed.replace("\"name\": \"worker\"", "\"name\": \"app\"")
    );
    assert!(
        app.read("build/wrangler.jsonc")
            .contains("\"vars\": { \"TITLE\": \"My App\" }")
    );
    succeeds(fixture.assemblr(&app.path, &["unit", "check"]));
}

#[test]
fn identical_contributions_merge_and_different_ones_refuse_the_build_naming_both() {
    let fixture = Fixture::new("conflict");
    let bump = |to: &str| PACKAGE.replace("\"hono\": \"^4\"", &format!("\"hono\": \"{to}\""));
    let (_worker, app) = scenario(
        &fixture,
        &[("package.json", &bump("^4.1"))],
        &[("package.json", &bump("^4.1"))],
    );
    succeeds(fixture.build(&app));
    assert!(
        app.read("build/package.json")
            .contains("\"hono\": \"^4.1\"")
    );

    // Siblings that disagree leave the unit above them a copy with the
    // conflict marked, declared and locked, and `build/` untouched; `unit
    // check` reports the same conflict without writing anything.
    let fixture = Fixture::new("conflict-different");
    let (_worker, app) = scenario(
        &fixture,
        &[("package.json", &bump("^4.2"))],
        &[("package.json", &bump("^4.1"))],
    );
    let check = stderr(&fails(fixture.assemblr(&app.path, &["unit", "check"])));
    assert!(
        check.contains(
            "error: src/package.json of @acme/worker 1.0.0: the amendments of @acme/api and \
             @acme/db conflict, and this unit does not resolve them:\nerror:   \
             /dependencies/hono: @acme/api changes \"^4\" to \"^4.1\"; @acme/db changes \"^4\" \
             to \"^4.2\""
        ),
        "{check}"
    );
    assert!(!app.path.join("amendments").exists());
    let err = stderr(&fails(fixture.build(&app)));
    for expected in [
        "error: amendments/package.json: the amendments of @acme/api and @acme/db of \
         src/package.json of @acme/worker 1.0.0 conflict, and the copy holds each conflict \
         between markers:\nerror:   /dependencies/hono: @acme/api changes \"^4\" to \"^4.1\"; \
         @acme/db changes \"^4\" to \"^4.2\"",
        "note: manifest.jsonc declares the copy, assemblr-lock.json was created, and build/ was \
         not changed",
    ] {
        assert!(err.contains(expected), "{expected:?} in {err}");
    }
    assert!(!app.path.join("build").exists());
    let copy = app.read("amendments/package.json");
    assert!(
        copy.contains(
            "\"hono\": {\n      \"<<<<<<< conflict\": {\n        \"@acme/api\": \"^4.1\",\n        \
             \"@acme/db\": \"^4.2\"\n      }\n    }"
        ),
        "{copy}"
    );
    assert!(
        app.read("manifest.jsonc")
            .contains(r#""package.json": { "unit": "@acme/worker", "base": "1.0.0" }"#)
    );
    // Resolving the conflict is replacing the marker; the copy is then the
    // unit's amendment like any other.
    app.write(
        "amendments/package.json",
        &copy.replace(
            "\"hono\": {\n      \"<<<<<<< conflict\": {\n        \"@acme/api\": \"^4.1\",\n        \
             \"@acme/db\": \"^4.2\"\n      }\n    }",
            "\"hono\": \"^4.2\"",
        ),
    );
    succeeds(fixture.build(&app));
    assert!(
        app.read("build/package.json")
            .contains("\"hono\": \"^4.2\"")
    );
    succeeds(fixture.assemblr(&app.path, &["unit", "check"]));
}

#[test]
fn a_value_one_amender_adds_cannot_be_changed_by_another_and_removals_conflict_with_edits() {
    let fixture = Fixture::new("flat");
    let engines = |node: &str| {
        PACKAGE.replace(
            "  \"files\": [\"dist\"]\n",
            &format!("  \"files\": [\"dist\"],\n  \"engines\": {{ \"node\": \"{node}\" }}\n"),
        )
    };
    let (_worker, app) = scenario(
        &fixture,
        &[("package.json", &engines(">=22"))],
        &[("package.json", &engines(">=24"))],
    );
    let err = stderr(&fails(fixture.build(&app)));
    assert!(
        err.contains("/engines/node: @acme/api adds \">=24\"; @acme/db adds \">=22\""),
        "{err}"
    );

    let fixture = Fixture::new("flat-remove");
    let removed = PACKAGE.replace(
        "  \"scripts\": {\n    \"dev\": \"wrangler dev\",\n    \"deploy\": \"wrangler deploy\"\n  },\n",
        "",
    );
    let changed = PACKAGE.replace("\"wrangler dev\"", "\"wrangler dev --local\"");
    let (_worker, app) = scenario(
        &fixture,
        &[("package.json", &removed)],
        &[("package.json", &changed)],
    );
    let err = stderr(&fails(fixture.build(&app)));
    assert!(
        err.contains(
            "/scripts: @acme/db removes {\"dev\":\"wrangler dev\",\"deploy\":\"wrangler…, while \
             @acme/api edits /scripts/dev inside it"
        ),
        "{err}"
    );
}

#[test]
fn lists_are_whole_values_and_jsonc_comments_and_trailing_commas_survive() {
    let fixture = Fixture::new("lists");
    let audit = WRANGLER.replace(
        "    { \"binding\": \"DB\", \"database_name\": \"main\" },\n",
        "    { \"binding\": \"DB\", \"database_name\": \"main\" },\n    { \"binding\": \"AUDIT\", \
         \"database_name\": \"audit\" },\n",
    );
    let flags = WRANGLER.replace(
        "  \"compatibility_date\": \"2026-01-01\",\n",
        "  \"compatibility_date\": \"2026-01-01\",\n  \"compatibility_flags\": [\"nodejs_compat\"],\n",
    );
    let (_worker, app) = scenario(
        &fixture,
        &[("wrangler.jsonc.assemblr.tmpl", &audit)],
        &[("wrangler.jsonc.assemblr.tmpl", &flags)],
    );
    succeeds(fixture.build(&app));
    // The file has trailing commas, so the list written anew has them too.
    assert_eq!(
        app.read("build/wrangler.jsonc"),
        "{\n  // The worker's name.\n  \"name\": \"worker\",\n  \"compatibility_date\": \
         \"2026-01-01\",\n  \"compatibility_flags\": [\"nodejs_compat\"],\n  \"d1_databases\": [\n    \
         {\n      \"binding\": \"DB\",\n      \"database_name\": \"main\",\n    },\n    {\n      \
         \"binding\": \"AUDIT\",\n      \"database_name\": \"audit\",\n    },\n  ],\n}\n"
    );

    // Two different changes inside one list are not merged.
    let fixture = Fixture::new("lists-conflict");
    let other = WRANGLER.replace(
        "    { \"binding\": \"DB\", \"database_name\": \"main\" },\n",
        "    { \"binding\": \"DB\", \"database_name\": \"main\" },\n    { \"binding\": \"CACHE\", \
         \"database_name\": \"cache\" },\n",
    );
    let (_worker, app) = scenario(
        &fixture,
        &[("wrangler.jsonc.assemblr.tmpl", &audit)],
        &[("wrangler.jsonc.assemblr.tmpl", &other)],
    );
    let err = stderr(&fails(fixture.build(&app)));
    assert!(err.contains("/d1_databases: @acme/api changes"), "{err}");
    assert!(err.contains("@acme/db changes"), "{err}");
}

#[test]
fn appends_of_scalars_to_one_list_are_joined() {
    // Each amender appends to the provider's list, one item shared.
    let fixture = Fixture::new("lists-joined");
    let db = PACKAGE.replace("[\"dist\"]", "[\"dist\", \"migrations\", \"README.md\"]");
    let api = PACKAGE.replace("[\"dist\"]", "[\"dist\", \"README.md\", \"openapi.json\"]");
    // Each amender creates the same list of flags, with its own flag.
    let db_flags = WRANGLER.replace(
        "  \"compatibility_date\": \"2026-01-01\",\n",
        "  \"compatibility_date\": \"2026-01-01\",\n  \"compatibility_flags\": [\"nodejs_compat\"],\n",
    );
    let api_flags = WRANGLER.replace(
        "  \"compatibility_date\": \"2026-01-01\",\n",
        "  \"compatibility_date\": \"2026-01-01\",\n  \"compatibility_flags\": [\"global_fetch\"],\n",
    );
    let (_worker, app) = scenario(
        &fixture,
        &[
            ("package.json", &db),
            ("wrangler.jsonc.assemblr.tmpl", &db_flags),
        ],
        &[
            ("package.json", &api),
            ("wrangler.jsonc.assemblr.tmpl", &api_flags),
        ],
    );
    succeeds(fixture.build(&app));
    // The provider's items first, then each amender's new ones in identity
    // order (@acme/api before @acme/db), each item once.
    let package = app.read("build/package.json");
    assert!(
        package.contains("\"files\": [\"dist\", \"README.md\", \"openapi.json\", \"migrations\"]"),
        "{package}"
    );
    let wrangler = app.read("build/wrangler.jsonc");
    assert!(
        wrangler.contains("\"compatibility_flags\": [\"global_fetch\", \"nodejs_compat\"],"),
        "{wrangler}"
    );
    assert!(wrangler.contains("// The worker's name."), "{wrangler}");
}

#[test]
fn what_a_copy_carries_beyond_values_is_reported_and_the_build_succeeds() {
    let fixture = Fixture::new("notes");
    let commented = PACKAGE.replace(
        "    \"hono\": \"^4\"\n",
        "    // Pinned until the next major.\n    \"hono\": \"^4\",\n    \"zod\": \"^3\"\n",
    );
    let reordered = PACKAGE.replace(
        "    \"dev\": \"wrangler dev\",\n    \"deploy\": \"wrangler deploy\"\n",
        "    \"deploy\": \"wrangler deploy\",\n    \"dev\": \"wrangler dev\"\n",
    );
    let (_worker, app) = scenario(
        &fixture,
        &[("package.json", &commented)],
        &[("package.json", &reordered)],
    );
    let output = succeeds(fixture.build(&app));
    let err = stderr(&output);
    assert!(
        err.contains(
            "warning: src/package.json of @acme/worker: not carried from the copy of @acme/api: the \
             order of the members of /scripts"
        ),
        "{err}"
    );
    assert!(
        err.contains(
            "warning: src/package.json of @acme/worker: not carried from the copy of @acme/db: the \
             comment \"// Pinned until the next major.\""
        ),
        "{err}"
    );
    let built = app.read("build/package.json");
    assert!(built.contains("\"zod\": \"^3\""), "{built}");
    assert!(!built.contains("Pinned"), "{built}");
    assert!(
        built.contains("\"dev\": \"wrangler dev\",\n    \"deploy\""),
        "{built}"
    );
}

#[test]
fn the_result_does_not_depend_on_declaration_or_registration_order() {
    let db = PACKAGE.replace(
        "    \"hono\": \"^4\"\n",
        "    \"drizzle-orm\": \"^0.44\",\n    \"hono\": \"^4\"\n",
    );
    let api = PACKAGE.replace(
        "    \"hono\": \"^4\"\n",
        "    \"hono\": \"^4\",\n    \"zod\": \"^3\"\n",
    );
    let fixture = Fixture::new("order-a");
    let (_worker, app) = scenario(
        &fixture,
        &[("package.json", &db)],
        &[("package.json", &api)],
    );
    succeeds(fixture.build(&app));
    let first = app.read("build/package.json");

    // The dependencies declared in the other order, the amenders registered
    // in the other order, and the copies swapped between amenders.
    let fixture = Fixture::new("order-b");
    let (_worker, app) = scenario(
        &fixture,
        &[("package.json", &api)],
        &[("package.json", &db)],
    );
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[
                ("@acme/worker", "=1.0.0"),
                ("@acme/api", "^1"),
                ("@acme/db", "^1"),
            ],
            "",
        ),
    );
    succeeds(fixture.build(&app));
    assert_eq!(app.read("build/package.json"), first);
    assert!(
        first.contains("\"drizzle-orm\": \"^0.44\",\n    \"hono\": \"^4\",\n    \"zod\": \"^3\""),
        "{first}"
    );
}

#[test]
fn unsupported_cases_keep_refusing_with_their_reason() {
    // A copy that is not valid JSON is refused naming its amender, and a
    // copy with a duplicate key too.
    let fixture = Fixture::new("invalid-copy");
    let (_worker, app) = scenario(
        &fixture,
        &[("package.json", "{ \"name\": \"worker\", ")],
        &[(
            "package.json",
            &PACKAGE.replace(
                "\"private\": true,",
                "\"private\": true,\n  \"private\": false,",
            ),
        )],
    );
    let err = stderr(&fails(fixture.build(&app)));
    assert!(err.contains("the copy of @acme/api cannot be read as JSON: the object at / has the key \"private\" twice"), "{err}");

    let fixture = Fixture::new("invalid-copy-syntax");
    let (_worker, app) = scenario(
        &fixture,
        &[("package.json", "{ \"name\": \"worker\", ")],
        &[("package.json", PACKAGE)],
    );
    let err = stderr(&fails(fixture.build(&app)));
    assert!(
        err.contains("the copy of @acme/db cannot be read as JSON: it is not valid JSONC"),
        "{err}"
    );
}

#[test]
fn crlf_providers_keep_their_line_endings_and_single_amenders_are_unchanged() {
    let fixture = Fixture::new("crlf");
    let crlf = PACKAGE.replace('\n', "\r\n");
    let db = crlf.replace(
        "    \"hono\": \"^4\"\r\n",
        "    \"drizzle-orm\": \"^0.44\",\r\n    \"hono\": \"^4\"\r\n",
    );
    let api = crlf.replace("\"wrangler dev\"", "\"wrangler dev --local\"");
    let (_worker, app) = scenario_with(
        &fixture,
        &crlf,
        &[("package.json", &db)],
        &[("package.json", &api)],
    );
    succeeds(fixture.build(&app));
    assert_eq!(
        app.read("build/package.json"),
        crlf.replace(
            "    \"hono\": \"^4\"\r\n",
            "    \"drizzle-orm\": \"^0.44\",\r\n    \"hono\": \"^4\"\r\n",
        )
        .replace("\"wrangler dev\"", "\"wrangler dev --local\"")
    );

    // One amender of a file is the copy itself, byte for byte.
    let fixture = Fixture::new("single");
    let copy = PACKAGE.replace("\"private\": true", "\"private\": false");
    let (_worker, app) = scenario(&fixture, &[("package.json", &copy)], &[]);
    succeeds(fixture.build(&app));
    assert_eq!(app.read("build/package.json"), copy);
}

#[test]
fn unit_amend_creates_a_copy_of_a_json_file_another_amender_amends() {
    let fixture = Fixture::new("amend-shared");
    let db = PACKAGE.replace(
        "    \"hono\": \"^4\"\n",
        "    \"drizzle-orm\": \"^0.44\",\n    \"hono\": \"^4\"\n",
    );
    let (_worker, app) = scenario(&fixture, &[("package.json", &db)], &[]);
    succeeds(fixture.build(&app));

    let output = succeeds(fixture.assemblr(&app.path, &["unit", "amend", "package.json"]));
    assert!(
        stdout(&output).contains(
            "The copy is composed from the amendments of @acme/db 1.0.0; a change to what they \
             added or changed overrides it in this unit."
        ),
        "{}",
        stdout(&output)
    );
    // The copy is the file as the unit's dependencies compose it.
    assert_eq!(app.read("amendments/package.json"), db);
    assert_eq!(
        app.read("amendments/package.json"),
        app.read("build/package.json")
    );
    app.write(
        "amendments/package.json",
        &db.replace(
            "    \"deploy\": \"wrangler deploy\"\n",
            "    \"deploy\": \"wrangler deploy\",\n    \"test\": \"vitest\"\n",
        ),
    );
    succeeds(fixture.build(&app));
    let built = app.read("build/package.json");
    assert!(built.contains("\"drizzle-orm\": \"^0.44\""), "{built}");
    assert!(built.contains("\"test\": \"vitest\""), "{built}");
}

#[test]
fn two_amenders_removing_the_same_member_agree() {
    let fixture = Fixture::new("shared-removal");
    let without_private = PACKAGE.replace("  \"private\": true,\n", "");
    let (_worker, app) = scenario(
        &fixture,
        &[("package.json", &without_private)],
        &[(
            "package.json",
            &without_private.replace("\"wrangler dev\"", "\"wrangler dev --local\""),
        )],
    );
    succeeds(fixture.build(&app));
    let built = app.read("build/package.json");
    assert!(!built.contains("private"), "{built}");
    assert!(built.contains("\"wrangler dev --local\""), "{built}");
    assert!(
        built.starts_with("{\n  \"name\": \"worker\",\n  \"scripts\": {"),
        "{built}"
    );
}

/// A library whose `package.json` has a conditional `exports`, where Node
/// reads the conditions in order and the first one that matches wins.
const LIBRARY: &str = "{\n  \"name\": \"lib\",\n  \"exports\": {\n    \".\": {\n      \
                       \"import\": \"./dist/index.mjs\",\n      \"require\": \"./dist/index.cjs\",\n      \
                       \"types\": \"./dist/index.d.ts\"\n    }\n  }\n}\n";

/// The conditions of `exports["."]` in the order the file lists them.
fn conditions(package: &str) -> Vec<String> {
    // Read from the text: a parsed map would not keep the order.
    let start = package.find("\"exports\"").unwrap();
    package[start..]
        .lines()
        .filter_map(|line| line.trim().split_once("\": "))
        .map(|(key, _)| key.trim_start_matches('"').to_owned())
        .filter(|key| !["exports", "."].contains(&key.as_str()))
        .collect()
}

#[test]
fn a_reordered_exports_is_kept_alone_and_dropped_with_a_warning_once_another_unit_amends() {
    // The `types` condition belongs first in `exports`: after the others it
    // is never reached. An amender fixes that by moving it, which is a
    // change of order and of nothing else.
    let fixture = Fixture::new("exports-reorder");
    let fixed = LIBRARY.replace(
        "      \"import\": \"./dist/index.mjs\",\n      \"require\": \"./dist/index.cjs\",\n      \
         \"types\": \"./dist/index.d.ts\"\n",
        "      \"types\": \"./dist/index.d.ts\",\n      \"import\": \"./dist/index.mjs\",\n      \
         \"require\": \"./dist/index.cjs\"\n",
    );
    assert_ne!(fixed, LIBRARY);
    let described = LIBRARY.replace(
        "\"name\": \"lib\"",
        "\"name\": \"lib\",\n  \"sideEffects\": false",
    );

    // Alone, the copy takes the file's place and the order applies.
    let (_worker, app) = scenario_with(&fixture, LIBRARY, &[], &[("package.json", &fixed)]);
    succeeds(fixture.build(&app));
    assert_eq!(
        conditions(&app.read("build/package.json")),
        ["types", "import", "require"]
    );

    // With a second amender the merge applies operations, and a move is not
    // one: the provider's order stands, and the build says the copy's order
    // was not carried, naming the path. The result is unchanged where the
    // order matters, and the author is told, never left to find out.
    let fixture = Fixture::new("exports-reorder-shared");
    let (_worker, app) = scenario_with(
        &fixture,
        LIBRARY,
        &[("package.json", &described)],
        &[("package.json", &fixed)],
    );
    let output = succeeds(fixture.build(&app));
    let built = app.read("build/package.json");
    assert_eq!(conditions(&built), ["import", "require", "types"]);
    assert!(built.contains("\"sideEffects\": false"), "{built}");
    let err = stderr(&output);
    assert!(
        err.contains(
            "not carried from the copy of @acme/api: the order of the members of /exports/."
        ),
        "{err}"
    );
}

#[test]
fn the_unit_s_author_hears_early_what_a_second_amender_would_not_carry() {
    let fixture = Fixture::new("exports-early");
    let fixed = LIBRARY.replace(
        "      \"import\": \"./dist/index.mjs\",\n      \"require\": \"./dist/index.cjs\",\n      \
         \"types\": \"./dist/index.d.ts\"\n",
        "      \"types\": \"./dist/index.d.ts\",\n      // Types first.\n      \"import\": \
         \"./dist/index.mjs\",\n      \"require\": \"./dist/index.cjs\"\n",
    );
    let (_worker, _app) = scenario_with(&fixture, LIBRARY, &[], &[("package.json", &fixed)]);
    let api = Repo {
        path: fixture.dir.join("api"),
    };

    // `unit check` and `unit build` of the amending unit itself say so,
    // although its copy is the only one and composes whole.
    for command in ["check", "build"] {
        let output = succeeds(fixture.assemblr(&api.path, &["unit", command]));
        let err = stderr(&output);
        assert!(
            err.contains(
                "amendments/package.json of @acme/api: if another unit amends src/package.json of \
                 @acme/worker too, the order of the members of /exports/. of this copy is not \
                 carried"
            ),
            "{command}: {err}"
        );
        assert!(
            err.contains("the comment \"// Types first.\" of this copy is not carried"),
            "{command}: {err}"
        );
    }
    // A copy that only changes values has nothing to say.
    let plain = LIBRARY.replace("./dist/index.mjs", "./dist/main.mjs");
    let fixture = Fixture::new("exports-early-plain");
    let (_worker, _app) = scenario_with(&fixture, LIBRARY, &[], &[("package.json", &plain)]);
    let output = succeeds(fixture.assemblr(&fixture.dir.join("api"), &["unit", "check"]));
    assert!(
        !stderr(&output).contains("not carried"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn conditions_two_units_add_at_one_anchor_are_ordered_by_amender_identity() {
    // Two units add a condition after the same one, and the merge places
    // them by amender identity. For `exports` that order can decide which
    // condition Node picks, so this pins today's behavior while the
    // question of making it a conflict is open, and changes with the answer.
    let fixture = Fixture::new("exports-same-anchor");
    let with = |condition: &str| {
        LIBRARY.replace(
            "      \"import\": \"./dist/index.mjs\",\n",
            &format!("      \"import\": \"./dist/index.mjs\",\n      {condition},\n"),
        )
    };
    let (_worker, app) = scenario_with(
        &fixture,
        LIBRARY,
        &[("package.json", &with("\"browser\": \"./dist/browser.js\""))],
        &[("package.json", &with("\"node\": \"./dist/node.js\""))],
    );
    let output = succeeds(fixture.build(&app));
    // `@acme/api` sorts before `@acme/db`: its condition comes first.
    assert_eq!(
        conditions(&app.read("build/package.json")),
        ["import", "node", "browser", "require", "types"]
    );
    assert!(!stderr(&output).contains("conflict"), "{}", stderr(&output));
}
