//! Several amendments of one text file, composed by the real binary: the
//! guidance file that every unit of a stack wants to extend, in a unit's
//! build and in a consumer's composition.
//!
//! The scenarios come from the reference scenario of the milestone: a base
//! unit that ships `AGENTS.md`, and units for a container and a database
//! that each add a section to it. Every test gives the binary its own
//! registry and home.

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

const GUIDE: &str = "# Agent instructions\n\nRead the README first.\n\n## Rules\n\n- Keep changes small.\n- Run the checks.\n\n## Reporting\n\nSay what you observed.\n";

/// A provider `@acme/guide` 1.0.0 with `AGENTS.md`, a template `NOTES.md`,
/// and a CRLF `WIN.txt`, registered; and one published amender per entry of
/// `amenders`, each depending on it and amending the files given, all
/// registered in the order listed.
fn scenario(fixture: &Fixture, amenders: &[(&str, &[(&str, &str)])]) -> Repo {
    let guide = fixture.repo("guide");
    guide.write(
        "manifest.jsonc",
        &manifest(
            "@acme/guide",
            "1.0.0",
            &[],
            r#", "params": {"project": {"type": "string", "description": "The project.", "default": "demo"}}"#,
        ),
    );
    guide.write("src/AGENTS.md", GUIDE);
    guide.write(
        "src/NOTES.md.assemblr.tmpl",
        "# Notes for {{ project }}\n\nFirst.\n\nSecond.\n\nThird.\n",
    );
    guide.write("src/WIN.txt", "one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n");
    guide.publish("1.0.0");
    fixture.register(&guide);
    for (name, copies) in amenders {
        let unit = fixture.repo(name);
        let files: Vec<&str> = copies.iter().map(|(file, _)| *file).collect();
        unit.write(
            "manifest.jsonc",
            &manifest(
                &format!("@acme/{name}"),
                "1.0.0",
                &[("@acme/guide", "=1.0.0")],
                &amending(&files, "@acme/guide", "1.0.0"),
            ),
        );
        for (file, contents) in *copies {
            unit.write(&format!("amendments/{file}"), contents);
        }
        unit.write(&format!("src/{name}.txt"), name);
        unit.write(
            "assemblr-lock.json",
            &unit.lockfile_for(&guide, "@acme/guide", "1.0.0"),
        );
        unit.publish("1.0.0");
        fixture.register(&unit);
    }
    guide
}

/// An unbuilt `@acme/app` depending on `names` and the provider.
fn app(fixture: &Fixture, names: &[&str]) -> Repo {
    let app = fixture.repo("app");
    let mut dependencies: Vec<(String, &str)> = names
        .iter()
        .map(|name| (format!("@acme/{name}"), "^1"))
        .collect();
    dependencies.push(("@acme/guide".to_owned(), "=1.0.0"));
    let borrowed: Vec<(&str, &str)> = dependencies
        .iter()
        .map(|(identity, range)| (identity.as_str(), *range))
        .collect();
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.1.0", &borrowed, ""),
    );
    app.write("src/app.txt", "app");
    app
}

fn with_section(heading: &str, line: &str) -> String {
    format!("{GUIDE}\n## {heading}\n\n{line}\n")
}

#[test]
fn units_that_each_add_a_section_to_one_guidance_file_compose_in_identity_order() {
    let fixture = Fixture::new("sections");
    scenario(
        &fixture,
        &[
            (
                "neon",
                &[("AGENTS.md", &with_section("Database", "Use DATABASE_URL."))],
            ),
            (
                "docker",
                &[(
                    "AGENTS.md",
                    &with_section("Development container", "Run commands with docker compose."),
                )],
            ),
        ],
    );
    let app = app(&fixture, &["neon", "docker"]);
    let output = succeeds(fixture.build(&app));
    // The build says that it chose the order of the two sections.
    assert!(
        stderr(&output).contains(
            "src/AGENTS.md of @acme/guide: @acme/docker, @acme/neon each add lines after line 12"
        ),
        "{}",
        stderr(&output)
    );
    let expected = format!(
        "{GUIDE}\n## Development container\n\nRun commands with docker compose.\n\
         \n## Database\n\nUse DATABASE_URL.\n"
    );
    assert_eq!(app.read("build/AGENTS.md"), expected);
    succeeds(fixture.assemblr(&app.path, &["unit", "check"]));

    // The order the units were published and registered in changes nothing.
    let other = Fixture::new("sections-reversed");
    scenario(
        &other,
        &[
            (
                "docker",
                &[(
                    "AGENTS.md",
                    &with_section("Development container", "Run commands with docker compose."),
                )],
            ),
            (
                "neon",
                &[("AGENTS.md", &with_section("Database", "Use DATABASE_URL."))],
            ),
        ],
    );
    let again = self::app(&other, &["docker", "neon"]);
    succeeds(other.build(&again));
    assert_eq!(again.read("build/AGENTS.md"), expected);
}

#[test]
fn edits_in_different_places_and_an_added_section_are_all_kept() {
    let fixture = Fixture::new("places");
    let title = GUIDE.replace("# Agent instructions", "# Instructions for coding agents");
    let rules = GUIDE.replace(
        "- Run the checks.\n",
        "- Run the checks.\n- Use the container.\n",
    );
    scenario(
        &fixture,
        &[
            ("a-title", &[("AGENTS.md", &title)]),
            ("b-rules", &[("AGENTS.md", &rules)]),
            (
                "c-section",
                &[("AGENTS.md", &with_section("Database", "Use DATABASE_URL."))],
            ),
        ],
    );
    let app = app(&fixture, &["a-title", "b-rules", "c-section"]);
    succeeds(fixture.build(&app));
    assert_eq!(
        app.read("build/AGENTS.md"),
        "# Instructions for coding agents\n\nRead the README first.\n\n## Rules\n\n\
         - Keep changes small.\n- Run the checks.\n- Use the container.\n\n## Reporting\n\n\
         Say what you observed.\n\n## Database\n\nUse DATABASE_URL.\n"
    );
}

#[test]
fn the_unit_s_own_amendment_joins_the_published_ones() {
    let fixture = Fixture::new("own");
    scenario(
        &fixture,
        &[(
            "neon",
            &[("AGENTS.md", &with_section("Database", "Use DATABASE_URL."))],
        )],
    );
    let app = app(&fixture, &["neon"]);
    succeeds(fixture.build(&app));
    succeed_amend(&fixture, &app);
    // The copy is the file as the dependency composes it, section included;
    // the unit adds a rule near the top, away from the section at the end.
    assert_eq!(
        app.read("amendments/AGENTS.md"),
        with_section("Database", "Use DATABASE_URL.")
    );
    app.write(
        "amendments/AGENTS.md",
        &app.read("amendments/AGENTS.md").replace(
            "- Keep changes small.\n",
            "- Keep changes small.\n- Ask first.\n",
        ),
    );
    succeeds(fixture.build(&app));
    let built = app.read("build/AGENTS.md");
    assert!(built.contains("- Ask first.\n"), "{built}");
    assert!(
        built.ends_with("## Database\n\nUse DATABASE_URL.\n"),
        "{built}"
    );
}

fn succeed_amend(fixture: &Fixture, app: &Repo) {
    let output = succeeds(fixture.assemblr(&app.path, &["unit", "amend", "AGENTS.md"]));
    assert!(
        stdout(&output).contains("The copy is composed from the amendments of @acme/neon 1.0.0"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn changes_to_the_same_lines_are_a_conflict_that_names_the_lines_and_the_units() {
    let fixture = Fixture::new("conflict");
    scenario(
        &fixture,
        &[
            (
                "neon",
                &[(
                    "AGENTS.md",
                    &GUIDE.replace("- Run the checks.", "- Run `npm run verify`."),
                )],
            ),
            (
                "docker",
                &[(
                    "AGENTS.md",
                    &GUIDE.replace("- Run the checks.", "- Run the checks in the container."),
                )],
            ),
        ],
    );
    let app = app(&fixture, &["neon", "docker"]);
    // The unit above the siblings gets the copy with the conflict marked,
    // and resolves it by editing the copy.
    let err = stderr(&fails(fixture.build(&app)));
    for expected in [
        "amendments/AGENTS.md: the amendments of @acme/docker and @acme/neon of src/AGENTS.md of \
         @acme/guide 1.0.0 conflict, and the copy holds each conflict between markers",
        "line 8: @acme/docker sets \"- Run the checks in the container.\"; @acme/neon sets \
         \"- Run `npm run verify`.\"",
        "note: manifest.jsonc declares the copy, assemblr-lock.json was created, and build/ was \
         not changed",
    ] {
        assert!(err.contains(expected), "{expected:?} in {err}");
    }
    assert!(!app.path.join("build").exists());
    let marked = "<<<<<<< @acme/docker\n- Run the checks in the container.\n||||||| base\n- Run the \
                  checks.\n=======\n- Run `npm run verify`.\n>>>>>>> @acme/neon\n";
    assert_eq!(
        app.read("amendments/AGENTS.md"),
        GUIDE.replace("- Run the checks.\n", marked)
    );
    app.write(
        "amendments/AGENTS.md",
        &GUIDE.replace(
            "- Run the checks.",
            "- Run `npm run verify` in the container.",
        ),
    );
    succeeds(fixture.build(&app));
    assert_eq!(
        app.read("build/AGENTS.md"),
        GUIDE.replace(
            "- Run the checks.",
            "- Run `npm run verify` in the container."
        )
    );

    // The same change by both is one change.
    let same = Fixture::new("same");
    let line = GUIDE.replace("- Run the checks.", "- Run `npm run verify`.");
    scenario(
        &same,
        &[
            ("neon", &[("AGENTS.md", &line)]),
            ("docker", &[("AGENTS.md", &line)]),
        ],
    );
    let both = self::app(&same, &["neon", "docker"]);
    succeeds(same.build(&both));
    assert_eq!(both.read("build/AGENTS.md"), line);
}

#[test]
fn rendered_templates_are_merged_after_rendering_and_crlf_files_keep_their_endings() {
    let fixture = Fixture::new("rendered");
    let notes_a = "# Notes for {{ project }}\n\nFirst, as amended by a.\n\nSecond.\n\nThird.\n";
    let notes_b = "# Notes for {{ project }}\n\nFirst.\n\nSecond.\n\nThird, as amended by b.\n";
    let win_a = "ONE\r\ntwo\r\nthree\r\nfour\r\nfive\r\n";
    let win_b = "one\r\ntwo\r\nthree\r\nfour\r\nFIVE\r\n";
    scenario(
        &fixture,
        &[
            (
                "a",
                &[("NOTES.md.assemblr.tmpl", notes_a), ("WIN.txt", win_a)],
            ),
            (
                "b",
                &[("NOTES.md.assemblr.tmpl", notes_b), ("WIN.txt", win_b)],
            ),
        ],
    );
    let app = app(&fixture, &["a", "b"]);
    succeeds(fixture.build(&app));
    assert_eq!(
        app.read("build/NOTES.md"),
        "# Notes for demo\n\nFirst, as amended by a.\n\nSecond.\n\nThird, as amended by b.\n"
    );
    assert_eq!(
        fs::read(app.path.join("build/WIN.txt")).unwrap(),
        b"ONE\r\ntwo\r\nthree\r\nfour\r\nFIVE\r\n"
    );
}

#[test]
fn a_file_that_is_not_text_is_still_refused_with_several_amenders() {
    let fixture = Fixture::new("binary");
    scenario(
        &fixture,
        &[
            ("a", &[("AGENTS.md", &with_section("A", "a"))]),
            ("b", &[("AGENTS.md", "binary\0 copy\n")]),
        ],
    );
    let app = app(&fixture, &["a", "b"]);
    let err = stderr(&fails(fixture.build(&app)));
    assert!(err.contains("or a file that is not text"), "{err}");
}

#[test]
fn a_consumer_composes_the_same_guidance_file_and_a_conflict_is_left_for_the_consumer() {
    let fixture = Fixture::new("consumer");
    scenario(
        &fixture,
        &[
            (
                "neon",
                &[("AGENTS.md", &with_section("Database", "Use DATABASE_URL."))],
            ),
            (
                "docker",
                &[(
                    "AGENTS.md",
                    &with_section("Development container", "Run commands with docker compose."),
                )],
            ),
        ],
    );
    let project = fixture.dir.join("shop");
    succeeds(fixture.assemblr(&fixture.dir, &["init", "shop"]));
    // The units deliver their own AGENTS.md; the one `init` writes, which
    // would be a conflict to resolve first, is covered in tests/compose.rs.
    fs::remove_file(project.join("AGENTS.md")).unwrap();
    git(&project, &["init", "-q"]);
    fs::write(
        project.join("assemblr.jsonc"),
        "{\n  \"dependencies\": {\"@acme/neon\": \"^1\", \"@acme/docker\": \"^1\"},\n  \"options\": {}\n}\n",
    )
    .unwrap();
    succeeds(fixture.assemblr(&project, &["compose"]));
    let composed = fs::read_to_string(project.join("AGENTS.md")).unwrap();
    assert_eq!(
        composed,
        format!(
            "{GUIDE}\n## Development container\n\nRun commands with docker compose.\n\
             \n## Database\n\nUse DATABASE_URL.\n"
        )
    );

    // A conflicting pair is marked in the file with the units named, the
    // rest is composed, and the composition waits (#101).
    let clash = Fixture::new("consumer-clash");
    scenario(
        &clash,
        &[
            (
                "neon",
                &[(
                    "AGENTS.md",
                    &GUIDE.replace("Keep changes small", "Keep it tiny"),
                )],
            ),
            (
                "docker",
                &[(
                    "AGENTS.md",
                    &GUIDE.replace("Keep changes small", "Keep it huge"),
                )],
            ),
        ],
    );
    let project = clash.dir.join("shop");
    succeeds(clash.assemblr(&clash.dir, &["init", "shop"]));
    // The units deliver their own AGENTS.md; the one `init` writes, which
    // would be a conflict to resolve first, is covered in tests/compose.rs.
    fs::remove_file(project.join("AGENTS.md")).unwrap();
    fs::write(
        project.join("assemblr.jsonc"),
        "{\n  \"dependencies\": {\"@acme/neon\": \"^1\", \"@acme/docker\": \"^1\"},\n  \"options\": {}\n}\n",
    )
    .unwrap();
    let output = fails(clash.assemblr(&project, &["compose"]));
    let out = stdout(&output);
    assert!(out.contains("  conflict AGENTS.md"), "{out}");
    assert!(out.contains("line 7: @acme/docker sets"), "{out}");
    assert!(out.contains("@acme/neon sets"), "{out}");
    assert!(
        stderr(&output).contains("1 file holds conflicts between units"),
        "{}",
        stderr(&output)
    );
    let marked = fs::read_to_string(project.join("AGENTS.md")).unwrap();
    assert!(
        marked.contains(
            "<<<<<<< @acme/docker\n- Keep it huge.\n||||||| base\n- Keep changes small.\n\
             =======\n- Keep it tiny.\n>>>>>>> @acme/neon\n"
        ),
        "{marked}"
    );
    assert!(project.join("NOTES.md").exists());
    assert!(!project.join("assemblr-lock.json").exists());
    let err = stderr(&fails(clash.assemblr(&project, &["compose", "--continue"])));
    assert!(
        err.contains("AGENTS.md: it still holds conflict markers"),
        "{err}"
    );
}

/// A provider `@acme/worker` 1.0.0 with `file` holding `base`, and the
/// amenders `db` and `api`, each adding its own port line after the last
/// line, and an unbuilt `@acme/app` depending on both.
fn port_scenario(fixture: &Fixture, file: &str, base: &str, line: fn(&str) -> String) -> Repo {
    amenders_scenario(fixture, file, base, |port| format!("{base}{}", line(port)))
}

/// As [`port_scenario`], with each amender's copy `copy(port)` whole.
fn amenders_scenario(
    fixture: &Fixture,
    file: &str,
    base: &str,
    copy: impl Fn(&str) -> String,
) -> Repo {
    let worker = fixture.repo("worker");
    worker.write(
        "manifest.jsonc",
        &manifest("@acme/worker", "1.0.0", &[], ""),
    );
    worker.write(&format!("src/{file}"), base);
    worker.publish("1.0.0");
    fixture.register(&worker);
    for (name, port) in [("db", "5432"), ("api", "8080")] {
        let unit = fixture.repo(name);
        unit.write(
            "manifest.jsonc",
            &manifest(
                &format!("@acme/{name}"),
                "1.0.0",
                &[("@acme/worker", "=1.0.0")],
                &amending(&[file], "@acme/worker", "1.0.0"),
            ),
        );
        unit.write(&format!("amendments/{file}"), &copy(port));
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
            &[("@acme/db", "^1"), ("@acme/api", "^1")],
            "",
        ),
    );
    app.write("src/app.txt", "app");
    app
}

#[test]
fn insertions_at_one_position_are_kept_in_identity_order_and_reported() {
    let fixture = Fixture::new("chained");
    let app = port_scenario(&fixture, "ports.txt", "web\nworker\n", |port| {
        format!("port {port}\n")
    });
    let output = succeeds(fixture.build(&app));
    // Both are kept, by identity, and the build says that it chose the order.
    assert_eq!(
        app.read("build/ports.txt"),
        "web\nworker\nport 8080\nport 5432\n"
    );
    let notes = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        notes.contains(
            "src/ports.txt of @acme/worker: @acme/api, @acme/db each add lines after line 2; \
             all are kept, in the order of the units' identities"
        ),
        "{notes}"
    );
}

#[test]
fn yaml_toml_and_ini_files_are_merged_by_line_like_text() {
    // Two units each add a service to one `compose.yaml`: merged by line.
    let fixture = Fixture::new("yaml");
    let app = port_scenario(
        &fixture,
        "compose.yaml",
        "services:\n  web:\n    image: app\n",
        |port| format!("  svc{port}:\n    ports: [\"{port}\"]\n"),
    );
    succeeds(fixture.build(&app));
    assert_eq!(
        app.read("build/compose.yaml"),
        "services:\n  web:\n    image: app\n  svc8080:\n    ports: [\"8080\"]\n  svc5432:\n    \
         ports: [\"5432\"]\n"
    );

    // The limit: a clean line merge does not check what the lines mean, so
    // two units that each add the same key leave it twice.
    let fixture = Fixture::new("toml");
    let app = port_scenario(&fixture, "config.toml", "[server]\n", |port| {
        format!("port = {port}\n")
    });
    succeeds(fixture.build(&app));
    assert_eq!(
        app.read("build/config.toml"),
        "[server]\nport = 8080\nport = 5432\n"
    );

    // Changes to the same line still conflict.
    let fixture = Fixture::new("ini");
    let app = port_scenario(&fixture, "app.ini", "[server]\nport = 80\n", |port| {
        format!("timeout = {port}\n")
    });
    succeeds(fixture.build(&app));
    let fixture = Fixture::new("ini-conflict");
    let app = amenders_scenario(&fixture, "app.ini", "[server]\nport = 80\n", |port| {
        format!("[server]\nport = {port}\n")
    });
    let err = stderr(&fails(fixture.build(&app)));
    assert!(
        err.contains("line 2: @acme/api sets \"port = 8080\"; @acme/db sets \"port = 5432\""),
        "{err}"
    );
    assert!(!app.path.join("build").exists());
    assert_eq!(
        app.read("amendments/app.ini"),
        "[server]\n<<<<<<< @acme/api\nport = 8080\n||||||| base\nport = 80\n=======\nport = \
         5432\n>>>>>>> @acme/db\n"
    );
}

#[test]
fn xml_files_stay_refused_with_several_amenders() {
    let fixture = Fixture::new("xml");
    let app = port_scenario(&fixture, "pom.xml", "<project>\n</project>\n", |port| {
        format!("<port>{port}</port>\n")
    });
    let err = stderr(&fails(fixture.build(&app)));
    assert!(
        err.contains(
            "src/pom.xml of @acme/worker 1.0.0 is amended by @acme/api 1.0.0, @acme/db 1.0.0; \
             several amendments of one file are combined for JSON, JSONC, and UTF-8 text files, \
             but not for XML files"
        ),
        "{err}"
    );
    assert!(err.contains("remove one of the amending units"), "{err}");
    assert!(!app.path.join("build").exists());
}

#[test]
fn unit_amend_copies_a_file_another_unit_amends_as_composed_whatever_its_format() {
    // Above the amending dependency, the unit's copy is taken on top of
    // the dependency's, so even an XML file, which siblings cannot share,
    // can be amended.
    for (name, file, base) in [
        (
            "amend-yaml",
            "compose.yaml",
            "services:\n  web:\n    image: app\n",
        ),
        ("amend-xml", "pom.xml", "<project>\n</project>\n"),
    ] {
        let fixture = Fixture::new(name);
        let _ = port_scenario(&fixture, file, base, |port| format!("# {port}\n"));
        let solo = fixture.repo("solo");
        solo.write(
            "manifest.jsonc",
            &manifest(
                "@acme/solo",
                "0.1.0",
                &[("@acme/db", "^1"), ("@acme/worker", "=1.0.0")],
                "",
            ),
        );
        solo.write("src/solo.txt", "solo");
        succeeds(fixture.build(&solo));
        let output = succeeds(fixture.assemblr(&solo.path, &["unit", "amend", file]));
        assert!(
            stdout(&output).contains("The copy is composed from the amendments of @acme/db 1.0.0"),
            "{}",
            stdout(&output)
        );
        let composed = format!("{base}# 5432\n");
        assert_eq!(solo.read(&format!("amendments/{file}")), composed);
        solo.write(
            &format!("amendments/{file}"),
            &format!("{composed}# solo\n"),
        );
        succeeds(fixture.build(&solo));
        assert_eq!(
            solo.read(&format!("build/{file}")),
            format!("{composed}# solo\n")
        );
    }
}

#[test]
fn a_consumer_is_warned_about_chained_insertions_only_when_the_file_is_written() {
    let fixture = Fixture::new("consumer-chained");
    scenario(
        &fixture,
        &[
            (
                "neon",
                &[("AGENTS.md", &with_section("Database", "Use DATABASE_URL."))],
            ),
            (
                "docker",
                &[(
                    "AGENTS.md",
                    &with_section("Development container", "Run commands with docker compose."),
                )],
            ),
            ("plain", &[]),
        ],
    );
    let project = fixture.dir.join("shop");
    succeeds(fixture.assemblr(&fixture.dir, &["init", "shop"]));
    // The units deliver their own AGENTS.md; the one `init` writes, which
    // would be a conflict to resolve first, is covered in tests/compose.rs.
    fs::remove_file(project.join("AGENTS.md")).unwrap();
    git(&project, &["init", "-q"]);
    fs::write(
        project.join("assemblr.jsonc"),
        "{\n  \"dependencies\": {\"@acme/neon\": \"^1\", \"@acme/docker\": \"^1\"},\n  \"options\": {}\n}\n",
    )
    .unwrap();
    let chained = "AGENTS.md of @acme/guide: @acme/docker, @acme/neon each add lines after line 12";
    // The composition that writes the file says that it chose the order.
    let first = succeeds(fixture.assemblr(&project, &["compose"]));
    assert!(stderr(&first).contains(chained), "{}", stderr(&first));

    // One that leaves the file as it is does not repeat it, whether it
    // changes nothing or only other files.
    let again = succeeds(fixture.assemblr(&project, &["compose"]));
    assert!(
        stdout(&again).contains("Nothing to compose"),
        "{}",
        stdout(&again)
    );
    assert!(
        !stderr(&again).contains("each add lines"),
        "{}",
        stderr(&again)
    );
    let added = succeeds(fixture.assemblr(&project, &["add", "@acme/plain"]));
    assert!(
        stdout(&added).contains("created plain.txt"),
        "{}",
        stdout(&added)
    );
    assert!(
        !stderr(&added).contains("each add lines"),
        "{}",
        stderr(&added)
    );

    // `--verbose` reports it whatever the composition writes.
    let verbose = succeeds(fixture.assemblr(&project, &["compose", "--verbose"]));
    assert!(stderr(&verbose).contains(chained), "{}", stderr(&verbose));
}
