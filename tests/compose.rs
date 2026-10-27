//! `assemblr init` and `assemblr compose`, run as a real process against
//! consumer projects and registries built in temporary directories.
//!
//! Every test gives the binary its own registry and home, in the system's
//! temporary directory rather than under `target/`, because a project's
//! ignore rules come from whatever repository encloses it. Units are
//! published by hand, as the earlier increments' tests do.

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

use common::{Fixture, NULL_DEVICE, fails, git, stderr, stdout, succeeds};

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

    /// Runs `assemblr` with `global` as the user's Git configuration.
    fn assemblr_with_global_config(&self, dir: &Path, global: &Path, args: &[&str]) -> Output {
        self.command(dir, args)
            .env("GIT_CONFIG_GLOBAL", global)
            .output()
            .unwrap()
    }

    fn register(&self, repo: &Repo) {
        let path = repo.path.to_str().unwrap();
        succeeds(self.assemblr(&self.dir, &["registry", "add", path]));
    }

    fn compose(&self, project: &Path) -> Output {
        self.assemblr(project, &["compose"])
    }

    /// A project directory with `assemblr.jsonc` declaring `dependencies`,
    /// created by `assemblr init`.
    fn project(&self, name: &str, dependencies: &[(&str, &str)]) -> PathBuf {
        succeeds(self.assemblr(&self.dir, &["init", name]));
        let project = self.dir.join(name);
        declare(&project, dependencies);
        project
    }
}

fn declare(project: &Path, dependencies: &[(&str, &str)]) {
    let dependencies: Vec<String> = dependencies
        .iter()
        .map(|(identity, range)| format!("    \"{identity}\": \"{range}\""))
        .collect();
    fs::write(
        project.join("assemblr.jsonc"),
        format!(
            "{{\n  \"dependencies\": {{\n{}\n  }}\n}}\n",
            dependencies.join(",\n")
        ),
    )
    .unwrap();
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

    fn publish(&self, version: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", version]);
        self.git(&["tag", &format!("v{version}")]);
    }
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

fn lockfile(project: &Path) -> Value {
    serde_json::from_str(&read(&project.join("assemblr-lock.json"))).unwrap()
}

/// A registered `@acme/node` 1.0.0 with a JSON file and plain files, and
/// `@acme/web` 1.0.0 depending on it, amending the JSON file.
fn registry(fixture: &Fixture) -> (Repo, Repo) {
    let node = fixture.repo("node");
    node.write("manifest.jsonc", &manifest("@acme/node", "1.0.0", &[], ""));
    node.write("src/config/app.json", "{\n  \"port\": 3000\n}\n");
    node.write("src/README.md", "node\n");
    node.write("src/bin/run", "#!/bin/sh\necho run\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            node.path.join("src/bin/run"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    node.publish("1.0.0");
    fixture.register(&node);
    let web = fixture.repo("web");
    web.write(
        "manifest.jsonc",
        &manifest(
            "@acme/web",
            "1.0.0",
            &[("@acme/node", "=1.0.0")],
            r#", "amendments": {"config/app.json": {"unit": "@acme/node", "base": "1.0.0"}}"#,
        ),
    );
    web.write(
        "amendments/config/app.json",
        "{\n  \"port\": 8080,\n  \"web\": true\n}\n",
    );
    web.write("src/web.txt", "web\n");
    web.write(
        "assemblr-lock.json",
        &format!(
            "{{\n  \"format\": 1,\n  \"units\": {{\n    \"@acme/node\": {{\n      \"commit\": \
             \"{}\",\n      \"tree\": \"{}\",\n      \"version\": \"1.0.0\"\n    }}\n  }}\n}}\n",
            node.git(&["rev-parse", "v1.0.0"]),
            node.git(&["rev-parse", "v1.0.0^{tree}"])
        ),
    );
    web.publish("1.0.0");
    fixture.register(&web);
    (node, web)
}

/// How many files `init` leaves in a new project: `assemblr.jsonc` and
/// `AGENTS.md`.
const INITIALIZED: usize = 2;

#[test]
fn init_creates_the_declaration_and_agents_md_and_nothing_else() {
    let fixture = Fixture::new("init");
    let output = succeeds(fixture.assemblr(&fixture.dir, &["init", "apps/shop"]));
    let project = fixture.dir.join("apps").join("shop");
    assert!(
        stdout(&output).contains("created the directory, assemblr.jsonc, and AGENTS.md"),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        read(&project.join("assemblr.jsonc")),
        "{\n  \"dependencies\": {}\n}\n"
    );
    let agents = read(&project.join("AGENTS.md"));
    assert!(agents.contains("run `assemblr guide`"), "{agents}");

    // A command run below the project points at it.
    fs::create_dir_all(project.join("src/pages")).unwrap();
    let err = stderr(&fails(
        fixture.assemblr(&project.join("src/pages"), &["compose"]),
    ));
    assert!(err.contains("error: no assemblr.jsonc in "), "{err}");
    assert!(
        err.contains(&format!(
            "hint: the project is in {}; run the command there",
            project.display()
        )),
        "{err}"
    );
    // Outside any project, the hint is to create one.
    let err = stderr(&fails(fixture.assemblr(&fixture.dir, &["compose"])));
    assert!(
        err.contains("run `assemblr init` to create a project here"),
        "{err}"
    );
    fs::remove_dir_all(project.join("src")).unwrap();
    assert_eq!(tree(&project).len(), INITIALIZED);
    assert!(!project.join(".git").exists());

    // An AGENTS.md of the project's own is kept as it is.
    let other = fixture.dir.join("other");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("AGENTS.md"), "Our rules.\n").unwrap();
    let output = succeeds(fixture.assemblr(&other, &["init"]));
    assert!(
        stdout(&output).contains("created assemblr.jsonc; AGENTS.md was already there"),
        "{}",
        stdout(&output)
    );
    assert_eq!(read(&other.join("AGENTS.md")), "Our rules.\n");

    // A dangling link there is kept and not followed.
    #[cfg(unix)]
    {
        let linked = fixture.dir.join("linked");
        fs::create_dir(&linked).unwrap();
        let target = fixture.dir.join("elsewhere.md");
        std::os::unix::fs::symlink(&target, linked.join("AGENTS.md")).unwrap();
        let output = succeeds(fixture.assemblr(&linked, &["init"]));
        assert!(
            stdout(&output).contains("AGENTS.md was already there"),
            "{}",
            stdout(&output)
        );
        assert!(!target.exists());
    }

    // Again: nothing changes, and an edited declaration is never overwritten.
    fs::write(
        project.join("assemblr.jsonc"),
        "{ \"dependencies\": { \"@acme/web\": \"^1\" } }",
    )
    .unwrap();
    let output = succeeds(fixture.assemblr(&project, &["init"]));
    assert!(
        stdout(&output)
            .contains("is already a project: assemblr.jsonc is in place; nothing changed."),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        read(&project.join("assemblr.jsonc")),
        "{ \"dependencies\": { \"@acme/web\": \"^1\" } }"
    );

    // Not in a unit's repository, and not over an unreadable declaration.
    let unit = fixture.dir.join("unit");
    fs::create_dir(&unit).unwrap();
    fs::write(unit.join("manifest.jsonc"), "{}").unwrap();
    let err = stderr(&fails(fixture.assemblr(&unit, &["init"])));
    assert!(
        err.contains("is a unit's repository, not a consumer project"),
        "{err}"
    );
    assert!(!unit.join("assemblr.jsonc").exists());
    fs::write(project.join("assemblr.jsonc"), "{").unwrap();
    let err = stderr(&fails(fixture.assemblr(&project, &["init"])));
    assert!(
        err.contains("is not a valid declaration, so it was left as it is"),
        "{err}"
    );
}

#[test]
fn the_first_compose_materializes_the_files_the_lock_and_the_base() {
    let fixture = Fixture::new("first");
    let (node, _web) = registry(&fixture);
    let project = fixture.project("shop", &[("@acme/web", "^1")]);

    let output = succeeds(fixture.compose(&project));
    assert!(
        stdout(&output).contains("Composed 2 units into 4 files."),
        "{}",
        stdout(&output)
    );
    assert!(
        stderr(&output).contains(
            "note: created assemblr-lock.json: @acme/node 1.0.0 (new), @acme/web 1.0.0 (new)"
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        read(&project.join("config/app.json")),
        "{\n  \"port\": 8080,\n  \"web\": true\n}\n"
    );
    assert_eq!(read(&project.join("README.md")), "node\n");
    assert_eq!(read(&project.join("web.txt")), "web\n");
    assert_eq!(read(&project.join("bin/run")), "#!/bin/sh\necho run\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(project.join("bin/run"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0o111
        );
    }

    // The lock: the units as a unit's lockfile records them, plus the
    // project's reconciliation state.
    let lock = lockfile(&project);
    assert_eq!(lock["format"], 1);
    assert_eq!(
        lock["units"]["@acme/node"]["commit"],
        node.git(&["rev-parse", "v1.0.0"])
    );
    assert_eq!(lock["units"]["@acme/web"]["version"], "1.0.0");
    assert_eq!(lock["project"]["format"], 1);
    let base = lock["project"]["base"].as_str().unwrap().to_owned();
    assert!(base.starts_with("sha256-v1:"), "{base}");
    assert_eq!(
        lock["project"]["managed"],
        serde_json::json!(["README.md", "bin/run", "config/app.json", "web.txt"])
    );
    assert_eq!(lock["project"]["unbased"], serde_json::json!([]));

    // The base: a listing named by the identity, one blob per file, and the
    // attributes that keep Git from converting any of it.
    let state = project.join(".assemblr");
    assert_eq!(read(&state.join(".gitattributes")), "* -text\n");
    let listing = read(&state.join("bases").join(&base["sha256-v1:".len()..]));
    assert!(
        listing.starts_with("assemblr-base-v1\n100644 "),
        "{listing}"
    );
    assert!(listing.contains(" README.md\n"), "{listing}");
    // The fixture marks `bin/run` executable where the file system can.
    #[cfg(unix)]
    assert!(
        listing
            .lines()
            .any(|line| line.starts_with("100755 ") && line.ends_with(" bin/run")),
        "{listing}"
    );
    let blobs: Vec<_> = tree(&state.join("objects"));
    assert_eq!(blobs.len(), 4);
    assert!(blobs.iter().any(|(_, bytes)| bytes == b"node\n"));
    assert!(!state.join("journal.json").exists());

    // A second compose changes nothing.
    let before = tree(&project);
    let output = succeeds(fixture.compose(&project));
    assert!(
        stdout(&output).contains("Nothing to compose: the project already matches assemblr.jsonc."),
        "{}",
        stdout(&output)
    );
    assert_eq!(tree(&project), before);
}

#[test]
fn the_declaration_s_order_does_not_change_the_result() {
    let fixture = Fixture::new("order");
    registry(&fixture);
    let project = fixture.project("one", &[("@acme/node", "^1"), ("@acme/web", "^1")]);
    succeeds(fixture.compose(&project));
    let first = tree(&project);

    // The same declaration in another order composes the same bytes, lock
    // and base included.
    let other = fixture.dir.join("two");
    succeeds(fixture.assemblr(&fixture.dir, &["init", "two"]));
    fs::write(
        other.join("assemblr.jsonc"),
        "{\n  \"dependencies\": {\n    \"@acme/web\": \"^1\",\n    \"@acme/node\": \"^1\"\n  }\n}\n",
    )
    .unwrap();
    succeeds(fixture.compose(&other));
    let second = tree(&other);
    let without_declaration = |files: &[(String, Vec<u8>)]| -> Vec<(String, Vec<u8>)> {
        files
            .iter()
            .filter(|(path, _)| path != "assemblr.jsonc")
            .cloned()
            .collect()
    };
    assert_eq!(without_declaration(&first), without_declaration(&second));
}

#[test]
fn a_declaration_with_options_is_refused_with_the_way_forward() {
    let fixture = Fixture::new("options");
    registry(&fixture);
    succeeds(fixture.assemblr(&fixture.dir, &["init", "shop"]));
    let project = fixture.dir.join("shop");
    fs::write(
        project.join("assemblr.jsonc"),
        "{\n  \"dependencies\": {\"@acme/node\": \"^1\"},\n  \"options\": {\"@acme/node\": {\"port\": 1}}\n}\n",
    )
    .unwrap();
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(err.contains("options were removed"), "{err}");
    assert!(err.contains("assemblr status"), "{err}");
    assert_eq!(tree(&project).len(), INITIALIZED);
}

#[test]
fn a_rejected_composition_leaves_the_project_unmodified() {
    let fixture = Fixture::new("rejected");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/missing", "^1")]);
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("no registered source provides @acme/missing"),
        "{err}"
    );
    assert!(err.contains("register the unit's repository"), "{err}");
    assert_eq!(tree(&project).len(), INITIALIZED);
    assert!(!project.join(".assemblr").exists());

    // Something that cannot hold conflict markers on the way to a
    // delivered path: the whole composition is refused, and a file of the
    // project's own at another delivered path is not touched either.
    declare(&project, &[("@acme/web", "^1")]);
    fs::write(project.join("README.md"), "mine").unwrap();
    fs::write(project.join("config"), "a file where a directory goes").unwrap();
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("cannot mark as a conflict:\nerror:   config\n"),
        "{err}"
    );
    assert!(
        err.contains("cannot hold conflict markers; move it away"),
        "{err}"
    );
    assert_eq!(read(&project.join("README.md")), "mine");
    assert!(!project.join("web.txt").exists());
    assert!(!project.join("assemblr-lock.json").exists());

    // A unit's lockfile in the project, and a unit's manifest.
    fs::remove_file(project.join("README.md")).unwrap();
    fs::remove_file(project.join("config")).unwrap();
    fs::write(
        project.join("assemblr-lock.json"),
        "{\"format\": 1, \"units\": {}}",
    )
    .unwrap();
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("is a unit's lockfile, without a project's reconciliation state"),
        "{err}"
    );
    fs::remove_file(project.join("assemblr-lock.json")).unwrap();
    fs::write(project.join("manifest.jsonc"), "{}").unwrap();
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("is a unit's repository, not a consumer project"),
        "{err}"
    );
}

#[test]
fn reserved_paths_cannot_be_delivered() {
    let fixture = Fixture::new("reserved");
    let unit = fixture.repo("bad");
    unit.write("manifest.jsonc", &manifest("@acme/bad", "1.0.0", &[], ""));
    unit.write("src/assemblr.jsonc", "{}");
    unit.write("src/.assemblr/x", "x");
    unit.write("src/ok.txt", "ok");
    unit.publish("1.0.0");
    fixture.register(&unit);
    let project = fixture.project("shop", &[("@acme/bad", "^1")]);
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("path(s) Assemblr reserves for its own files:\nerror:   .assemblr/x\nerror:   assemblr.jsonc"),
        "{err}"
    );
    assert_eq!(tree(&project).len(), INITIALIZED);
}

/// What follows `hint:` in a failed command's output.
fn hint_of(err: &str) -> &str {
    err.lines()
        .find_map(|line| line.strip_prefix("hint: "))
        .unwrap_or_else(|| panic!("no hint in {err}"))
}

#[test]
fn a_collision_across_versions_points_at_the_version_that_can_meet() {
    let fixture = Fixture::new("hint-base");
    let (node, _web) = registry(&fixture);
    // 1.1.0 sets the key the amender sets, to another value.
    node.write(
        "src/config/app.json",
        "{\n  \"port\": 3000,\n  \"shell\": false\n}\n",
    );
    publish_node(&fixture, &node, "1.1.0", "node 1.1\n", "#!/bin/sh\n");
    // An amender that accepts any 1.x but is based on 1.0.0.
    let shell = fixture.repo("shell");
    shell.write(
        "manifest.jsonc",
        &manifest(
            "@acme/shell",
            "1.0.0",
            &[("@acme/node", "^1")],
            r#", "amendments": {"config/app.json": {"unit": "@acme/node", "base": "1.0.0"}}"#,
        ),
    );
    shell.write(
        "amendments/config/app.json",
        "{\n  \"port\": 3000,\n  \"shell\": true\n}\n",
    );
    shell.write(
        "assemblr-lock.json",
        &format!(
            "{{\n  \"format\": 1,\n  \"units\": {{\n    \"@acme/node\": {{\n      \"commit\": \
             \"{}\",\n      \"tree\": \"{}\",\n      \"version\": \"1.0.0\"\n    }}\n  }}\n}}\n",
            node.git(&["rev-parse", "v1.0.0"]),
            node.git(&["rev-parse", "v1.0.0^{tree}"])
        ),
    );
    shell.publish("1.0.0");
    fixture.register(&shell);
    let project = fixture.project("shop", &[("@acme/shell", "^1")]);
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("the amendment of @acme/shell was taken from @acme/node 1.0.0"),
        "{err}"
    );
    let hint = hint_of(&err);
    assert!(hint.contains("@acme/node@=1.0.0"), "{hint}");
    assert_eq!(tree(&project).len(), INITIALIZED);
}

#[test]
fn problems_of_different_kinds_each_get_their_own_advice() {
    let fixture = Fixture::new("hint-kinds");
    let (node, _web) = registry(&fixture);
    // A path collision between two units, and two units amending a file
    // whose amendments cannot be combined, reported together.
    let other = fixture.repo("other");
    other.write("manifest.jsonc", &manifest("@acme/other", "1.0.0", &[], ""));
    other.write("src/README.md", "other\n");
    other.publish("1.0.0");
    fixture.register(&other);
    for name in ["left", "right"] {
        let repo = fixture.repo(name);
        repo.write(
            "manifest.jsonc",
            &manifest(
                &format!("@acme/{name}"),
                "1.0.0",
                &[("@acme/node", "=1.0.0")],
                r#", "amendments": {"bin/run": {"unit": "@acme/node", "base": "1.0.0"}}"#,
            ),
        );
        repo.write("amendments/bin/run", &format!("\0{name}\n"));
        repo.write(
            "assemblr-lock.json",
            &format!(
                "{{\n  \"format\": 1,\n  \"units\": {{\n    \"@acme/node\": {{\n      \"commit\": \
                 \"{}\",\n      \"tree\": \"{}\",\n      \"version\": \"1.0.0\"\n    }}\n  }}\n}}\n",
                node.git(&["rev-parse", "v1.0.0"]),
                node.git(&["rev-parse", "v1.0.0^{tree}"])
            ),
        );
        repo.publish("1.0.0");
        fixture.register(&repo);
    }
    let project = fixture.project(
        "shop",
        &[
            ("@acme/node", "^1"),
            ("@acme/other", "^1"),
            ("@acme/left", "^1"),
            ("@acme/right", "^1"),
        ],
    );
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(err.contains("README.md"), "{err}");
    let hint = hint_of(&err);
    assert!(hint.contains("remove one of the units"), "{hint}");
    assert!(hint.contains("remove one of the amending units"), "{hint}");
}

/// Publishes `version` of the registered `node` with `readme` and `run` as
/// its plain files, and indexes it.
fn publish_node(fixture: &Fixture, node: &Repo, version: &str, readme: &str, run: &str) {
    node.write("manifest.jsonc", &manifest("@acme/node", version, &[], ""));
    node.write("src/README.md", readme);
    node.write("src/bin/run", run);
    node.publish(version);
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
}

#[test]
fn a_changed_declaration_is_reconciled_three_way_and_a_bad_base_is_detected() {
    let fixture = Fixture::new("changed");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    succeeds(fixture.compose(&project));
    let first_base = lockfile(&project)["project"]["base"]
        .as_str()
        .unwrap()
        .to_owned();

    // The consumer edits a file the composition will not change, adds a
    // member to one it will, and deletes one it leaves alone.
    fs::write(project.join("README.md"), "node\nmine\n").unwrap();
    fs::write(
        project.join("config/app.json"),
        "{\n  \"port\": 3000,\n  \"mine\": 1\n}\n",
    )
    .unwrap();
    fs::remove_file(project.join("bin/run")).unwrap();

    // Selecting `web` instead brings its file and its amendment of the JSON
    // file: the consumer's member survives the merge, the edit survives
    // untouched, and the deletion stands.
    declare(&project, &[("@acme/web", "^1")]);
    let output = succeeds(fixture.compose(&project));
    let out = stdout(&output);
    assert!(
        out.contains("Composed 2 units into 4 files: 1 created, 1 merged."),
        "{out}"
    );
    assert!(
        out.contains("\n  merged config/app.json\n  created web.txt\n"),
        "{out}"
    );
    let err = stderr(&output);
    assert!(
        err.contains("note: updated assemblr-lock.json: @acme/web 1.0.0 (new)"),
        "{err}"
    );
    assert!(
        err.contains(
            "warning: bin/run was deleted in the project and the composition does not change \
             it; it stays deleted"
        ),
        "{err}"
    );
    assert_eq!(read(&project.join("README.md")), "node\nmine\n");
    assert_eq!(
        read(&project.join("config/app.json")),
        "{\n  \"port\": 8080,\n  \"web\": true,\n  \"mine\": 1\n}\n"
    );
    assert_eq!(read(&project.join("web.txt")), "web\n");
    assert!(!project.join("bin/run").exists());
    let lock = lockfile(&project);
    let second_base = lock["project"]["base"].as_str().unwrap().to_owned();
    assert_ne!(second_base, first_base);
    assert_eq!(
        lock["project"]["managed"],
        serde_json::json!(["README.md", "bin/run", "config/app.json", "web.txt"])
    );
    // The new base is stored beside the old one, which stays.
    let bases = project.join(".assemblr").join("bases");
    assert!(bases.join(&first_base["sha256-v1:".len()..]).exists());
    assert!(bases.join(&second_base["sha256-v1:".len()..]).exists());
    assert!(!project.join(".assemblr").join("journal.json").exists());
    assert!(!project.join(".assemblr").join("previous").exists());
    assert_eq!(
        read(&project.join(".assemblr").join(".gitignore")),
        "/journal.json\n/journal.json.new\n/previous/\n"
    );
    let output = succeeds(fixture.compose(&project));
    assert!(
        stdout(&output).contains("Nothing to compose"),
        "{}",
        stdout(&output)
    );

    // Back to `node` alone: the amendment's member and its port go, the
    // consumer's member stays, and the unedited file is removed.
    declare(&project, &[("@acme/node", "^1")]);
    let output = succeeds(fixture.compose(&project));
    let out = stdout(&output);
    assert!(
        out.contains("Composed 1 unit into 3 files: 1 merged, 1 removed."),
        "{out}"
    );
    assert!(
        out.contains("\n  merged config/app.json\n  removed web.txt\n"),
        "{out}"
    );
    assert!(
        stderr(&output).contains("@acme/web 1.0.0 (no longer required)"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        read(&project.join("config/app.json")),
        "{\n  \"port\": 3000,\n  \"mine\": 1\n}\n"
    );
    assert!(!project.join("web.txt").exists());
    assert_eq!(lockfile(&project)["project"]["base"], first_base);

    // No units at all: every file would go, but one is edited, so nothing
    // happens.
    declare(&project, &[]);
    let before = tree(&project);
    let output = fails(fixture.compose(&project));
    let err = stderr(&output);
    assert!(
        err.contains("disagree at 2 path(s)")
            && err.contains(
                "error:   README.md: the composition no longer delivers it, and the project \
                 edited it\nerror:   config/app.json: the composition no longer delivers it, \
                 and the project edited it"
            ),
        "{err}"
    );
    assert!(
        err.contains("only conflicts between text files are left in the files to resolve"),
        "{err}"
    );
    assert_eq!(tree(&project), before);
    declare(&project, &[("@acme/node", "^1")]);

    // A tampered blob, a tampered listing, and a missing store each fail
    // verification by name, before anything else.
    let objects = project.join(".assemblr").join("objects");
    let readme_blob = assemblr::domain::BlobId::of(b"node\n").to_string();
    let blob_path = objects.join(&readme_blob[..2]).join(&readme_blob[2..]);
    let original = fs::read(&blob_path).unwrap();
    fs::write(&blob_path, b"tampered").unwrap();
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("does not verify: its bytes are not the content of"),
        "{err}"
    );
    assert!(
        err.contains("restore .assemblr/ from version control"),
        "{err}"
    );
    fs::write(&blob_path, original).unwrap();
    succeeds(fixture.compose(&project));
    let listing = bases.join(&first_base["sha256-v1:".len()..]);
    let text = fs::read(&listing).unwrap();
    fs::write(&listing, [text, b"x".to_vec()].concat()).unwrap();
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("its bytes do not hash to the identity the lockfile records"),
        "{err}"
    );
    fs::remove_dir_all(project.join(".assemblr")).unwrap();
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("the reconciliation base the lockfile records is not stored"),
        "{err}"
    );
    assert!(
        err.contains("restore .assemblr/ from version control"),
        "{err}"
    );
}

#[test]
fn conflicts_are_refused_before_anything_is_written() {
    let fixture = Fixture::new("conflicts");
    let (node, _) = registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    succeeds(fixture.compose(&project));

    // The consumer changes the port the new version changes, edits the
    // text file it changes, and deletes the script it changes.
    fs::write(project.join("config/app.json"), "{\n  \"port\": 9000\n}\n").unwrap();
    fs::write(project.join("README.md"), "mine\n").unwrap();
    fs::remove_file(project.join("bin/run")).unwrap();
    node.write("src/config/app.json", "{\n  \"port\": 8080\n}\n");
    publish_node(
        &fixture,
        &node,
        "1.1.0",
        "node v2\n",
        "#!/bin/sh\necho run v2\n",
    );
    declare(&project, &[("@acme/node", "^1.1")]);
    let before = tree(&project);

    let output = fails(fixture.compose(&project));
    let err = stderr(&output);
    assert!(
        err.contains("disagree at 3 path(s)")
            && err.contains(
                "error:   README.md: the project and the composition change the same lines, in 1 \
             place(s)\nerror:   bin/run: the project deleted it, and the composition \
             changes it\nerror:   config/app.json: the project and the composition change the \
             same values:\nerror:     /port: the project changes 3000 to 9000; the composition \
             changes 3000 to 8080"
            ),
        "{err}"
    );
    assert_eq!(tree(&project), before);
    assert_eq!(
        lockfile(&project)["units"]["@acme/node"]["version"],
        "1.0.0"
    );

    // A managed path that is no longer a regular file is a conflict too.
    fs::write(project.join("config/app.json"), "{\n  \"port\": 3000\n}\n").unwrap();
    fs::write(project.join("README.md"), "node\n").unwrap();
    fs::create_dir(project.join("bin/run")).unwrap();
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(err.contains("disagree at 1 path(s)"), "{err}");
    assert!(
        err.contains("bin/run: it is not a regular file in the project"),
        "{err}"
    );

    // With every file as composed, the new version applies.
    fs::remove_dir(project.join("bin/run")).unwrap();
    fs::write(project.join("bin/run"), "#!/bin/sh\necho run\n").unwrap();
    let output = succeeds(fixture.compose(&project));
    let out = stdout(&output);
    assert!(
        out.contains("Composed 1 unit into 3 files: 3 modified."),
        "{out}"
    );
    assert_eq!(read(&project.join("README.md")), "node v2\n");
    assert_eq!(read(&project.join("bin/run")), "#!/bin/sh\necho run v2\n");
    assert_eq!(
        read(&project.join("config/app.json")),
        "{\n  \"port\": 8080\n}\n"
    );
    assert_eq!(
        lockfile(&project)["units"]["@acme/node"]["version"],
        "1.1.0"
    );
}

#[test]
fn crlf_line_endings_in_the_project_count_as_unchanged_and_are_kept() {
    let fixture = Fixture::new("crlf");
    let (node, _) = registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    succeeds(fixture.compose(&project));
    let crlf = |name: &str| {
        let path = project.join(name);
        let text = read(&path).replace('\n', "\r\n");
        fs::write(&path, text).unwrap();
    };
    crlf("README.md");
    crlf("config/app.json");

    // A new version changes the JSON file: the file is replaced, with its
    // line endings; the other file is unchanged and stays as it is.
    node.write("src/config/app.json", "{\n  \"port\": 8080\n}\n");
    publish_node(&fixture, &node, "1.1.0", "node\n", "#!/bin/sh\necho run\n");
    declare(&project, &[("@acme/node", "^1.1")]);
    let output = succeeds(fixture.compose(&project));
    let out = stdout(&output);
    assert!(out.contains(": 1 modified."), "{out}");
    assert!(
        stderr(&output).contains(
            "warning: config/app.json has CRLF line endings in the project, so the composed \
             content was written with CRLF line endings too"
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        read(&project.join("config/app.json")),
        "{\r\n  \"port\": 8080\r\n}\r\n"
    );
    assert_eq!(read(&project.join("README.md")), "node\r\n");

    // A merge into a file with CRLF line endings keeps them.
    fs::write(
        project.join("config/app.json"),
        "{\r\n  \"port\": 8080,\r\n  \"mine\": 1\r\n}\r\n",
    )
    .unwrap();
    declare(&project, &[("@acme/web", "^1")]);
    let output = succeeds(fixture.compose(&project));
    assert!(
        stdout(&output).contains("  merged config/app.json\n"),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        read(&project.join("config/app.json")),
        "{\r\n  \"port\": 8080,\r\n  \"web\": true,\r\n  \"mine\": 1\r\n}\r\n"
    );
}

#[test]
fn unbased_files_are_compared_by_identity_and_the_ignore_rules_are_read_again() {
    let fixture = Fixture::new("unbased");
    let unit = fixture.repo("env");
    let publish = |version: &str, env: &str, app: &str| {
        unit.write("manifest.jsonc", &manifest("@acme/env", version, &[], ""));
        unit.write("src/.env", env);
        unit.write("src/app.txt", app);
        unit.publish(version);
    };
    publish("1.0.0", "SECRET=1\n", "app\n");
    fixture.register(&unit);
    let project = fixture.project("shop", &[("@acme/env", "^1")]);
    git(&project, &["init", "-q"]);
    fs::write(project.join(".gitignore"), ".env\n").unwrap();
    succeeds(fixture.compose(&project));
    let blobs = || tree(&project.join(".assemblr").join("objects")).len();
    assert_eq!(blobs(), 1);

    // The consumer edits the ignored file; a new version changes the other
    // one only, so the edit is left alone.
    fs::write(project.join(".env"), "SECRET=mine\n").unwrap();
    publish("1.1.0", "SECRET=1\n", "app v2\n");
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    declare(&project, &[("@acme/env", "^1.1")]);
    let output = succeeds(fixture.compose(&project));
    let out = stdout(&output);
    assert!(out.contains(": 1 modified."), "{out}");
    assert!(out.contains("  modified app.txt\n"), "{out}");
    assert!(
        !stderr(&output).contains("warning: .env is ignored"),
        "an unbased file is announced once: {}",
        stderr(&output)
    );
    assert_eq!(read(&project.join(".env")), "SECRET=mine\n");
    assert_eq!(read(&project.join("app.txt")), "app v2\n");

    // A version that changes the ignored file too: both sides changed it,
    // and without a stored base the changes cannot be merged.
    publish("1.2.0", "SECRET=2\n", "app v2\n");
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    declare(&project, &[("@acme/env", "^1.2")]);
    let before = tree(&project);
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains(
            ".env: the project and the composition both changed it, and its base is not stored \
             (the file was ignored by version control when it was composed), so the changes \
             cannot be told apart"
        ),
        "{err}"
    );
    assert_eq!(tree(&project), before);

    // Back at the composed content, the change applies; and once the file
    // is no longer ignored, its base is stored from then on.
    fs::write(project.join(".env"), "SECRET=1\n").unwrap();
    fs::write(project.join(".gitignore"), "").unwrap();
    let output = succeeds(fixture.compose(&project));
    assert!(
        stdout(&output).contains("  modified .env\n"),
        "{}",
        stdout(&output)
    );
    assert!(
        stderr(&output).contains(
            ".env is no longer ignored by the project's .gitignore, so its composed content is \
             stored in the tracked base from now on"
        ),
        "a file entering the tracked base is announced: {}",
        stderr(&output)
    );
    assert_eq!(read(&project.join(".env")), "SECRET=2\n");
    assert_eq!(
        lockfile(&project)["project"]["unbased"],
        serde_json::json!([])
    );
    assert_eq!(blobs(), 3);

    // Ignoring a file later keeps its new content out of the store.
    fs::write(project.join(".gitignore"), "app.txt\n").unwrap();
    publish("1.3.0", "SECRET=2\n", "app v3\n");
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    declare(&project, &[("@acme/env", "^1.3")]);
    let output = succeeds(fixture.compose(&project));
    assert!(
        stderr(&output).contains("warning: app.txt is ignored by the project's .gitignore"),
        "{}",
        stderr(&output)
    );
    assert_eq!(read(&project.join("app.txt")), "app v3\n");
    assert_eq!(
        lockfile(&project)["project"]["unbased"],
        serde_json::json!(["app.txt"])
    );
    assert_eq!(blobs(), 3);
}

#[test]
fn an_interrupted_composition_is_reverted_by_the_next_one() {
    let fixture = Fixture::new("interrupted");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    succeeds(fixture.compose(&project));
    let complete = tree(&project);

    // The state an interruption after some files and before the lock
    // leaves: the journal, the store, and part of the files.
    let base = lockfile(&project)["project"]["base"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::remove_file(project.join("assemblr-lock.json")).unwrap();
    fs::remove_file(project.join("README.md")).unwrap();
    fs::write(project.join("bin/run"), "edited meanwhile").unwrap();
    let blob = |bytes: &[u8]| assemblr::domain::BlobId::of(bytes).to_string();
    let journal = format!(
        "{{\n  \"format\": 1,\n  \"base\": \"{base}\",\n  \"files\": [\n    {{\"path\": \"README.md\", \
         \"blob\": \"{}\"}},\n    {{\"path\": \"bin/run\", \"blob\": \"{}\"}},\n    {{\"path\": \
         \"config/app.json\", \"blob\": \"{}\"}}\n  ],\n  \"directories\": [\"bin\", \"config\"]\n}}\n",
        blob(b"node\n"),
        blob(b"#!/bin/sh\necho run\n"),
        blob(b"{\n  \"port\": 3000\n}\n")
    );
    fs::write(project.join(".assemblr").join("journal.json"), &journal).unwrap();

    // The next compose reverts first: the files still holding what was
    // written go, the edited one stays, the journal goes. The edited file
    // is then the project's own, which the fresh composition leaves marked
    // against its own content, as any existing content is.
    let output = fails(fixture.compose(&project));
    assert!(
        stderr(&output).contains(
            "note: reverted a composition that was interrupted earlier: removed 1 file it created, \
             restored 0 it changed or removed, kept 1 whose content changed since (bin/run)"
        ),
        "{}",
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("  conflict bin/run"),
        "{}",
        stdout(&output)
    );
    let run = read(&project.join("bin/run"));
    assert!(
        run.contains("<<<<<<< current (your file)\nedited meanwhile\n"),
        "{run}"
    );
    assert!(
        run.contains("||||||| base (none: no composition wrote this file)\n=======\n"),
        "{run}"
    );

    // Taking the composition's side, the project reaches the same state as
    // before the interruption.
    fs::write(project.join("bin/run"), "#!/bin/sh\necho run\n").unwrap();
    succeeds(fixture.assemblr(&project, &["compose", "--continue"]));
    assert_eq!(tree(&project), complete);

    // An interruption after the lock was written: the composition is
    // complete, so the next run removes only the journal and keeps the
    // files and the lock.
    fs::write(project.join(".assemblr").join("journal.json"), &journal).unwrap();
    let output = succeeds(fixture.compose(&project));
    assert!(
        stderr(&output).contains(
            "note: completed a composition that was interrupted earlier, after its files and lock \
             were written: removed its journal"
        ),
        "{}",
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("Nothing to compose"),
        "{}",
        stdout(&output)
    );
    assert_eq!(tree(&project), complete);
}

#[test]
fn ignored_delivered_files_get_no_stored_base_and_a_warning() {
    let fixture = Fixture::new("ignored");
    let unit = fixture.repo("env");
    unit.write("manifest.jsonc", &manifest("@acme/env", "1.0.0", &[], ""));
    unit.write("src/.env", "SECRET=1\n");
    unit.write("src/app.txt", "app\n");
    unit.publish("1.0.0");
    fixture.register(&unit);
    // The project is a Git repository that ignores `.env`.
    let project = fixture.project("shop", &[("@acme/env", "^1")]);
    git(&project, &["init", "-q"]);
    fs::write(project.join(".gitignore"), ".env\n").unwrap();

    let output = succeeds(fixture.compose(&project));
    assert!(
        stderr(&output).contains(
            "warning: .env is ignored by the project's .gitignore, so its reconciliation base is \
             not stored"
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(read(&project.join(".env")), "SECRET=1\n");
    let blobs = tree(&project.join(".assemblr").join("objects"));
    assert_eq!(blobs.len(), 1);
    assert_eq!(blobs[0].1, b"app\n");
    assert_eq!(
        lockfile(&project)["project"]["unbased"],
        serde_json::json!([".env"])
    );
    // Nothing was staged.
    assert!(git(&project, &["diff", "--cached", "--name-only"]).is_empty());

    // A project that ignores `.assemblr/`, or one of its directories, is
    // warned that the base will not travel; a project outside any
    // repository ignores nothing.
    let other = fixture.project("other", &[("@acme/env", "^1")]);
    git(&other, &["init", "-q"]);
    fs::write(other.join(".gitignore"), ".assemblr/\n").unwrap();
    let output = succeeds(fixture.compose(&other));
    assert!(
        stderr(&output).contains(".assemblr/ is ignored by the project's version control"),
        "{}",
        stderr(&output)
    );
    let objects = fixture.project("objects", &[("@acme/env", "^1")]);
    git(&objects, &["init", "-q"]);
    fs::write(objects.join(".gitignore"), ".assemblr/objects/\n").unwrap();
    let output = succeeds(fixture.compose(&objects));
    assert!(
        stderr(&output).contains(".assemblr/objects/ is ignored by the project's version control"),
        "{}",
        stderr(&output)
    );

    // An edited attributes file is restored when the base is written.
    let edited = fixture.project("edited", &[("@acme/env", "^1")]);
    fs::create_dir(edited.join(".assemblr")).unwrap();
    fs::write(edited.join(".assemblr/.gitattributes"), "* text=auto\n").unwrap();
    let output = succeeds(fixture.compose(&edited));
    assert!(
        stderr(&output)
            .contains("warning: .assemblr/.gitattributes had been edited and was restored"),
        "{}",
        stderr(&output)
    );
    assert_eq!(read(&edited.join(".assemblr/.gitattributes")), "* -text\n");
    let plain = fixture.project("plain", &[("@acme/env", "^1")]);
    let output = succeeds(fixture.compose(&plain));
    assert!(!stderr(&output).contains("warning"), "{}", stderr(&output));
    assert_eq!(tree(&plain.join(".assemblr").join("objects")).len(), 2);
}

#[test]
fn a_checkout_with_line_ending_conversion_leaves_the_stored_base_intact() {
    let fixture = Fixture::new("checkout");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/web", "^1")]);
    git(&project, &["init", "-q"]);
    succeeds(fixture.compose(&project));
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-q", "-m", "composed"]);

    // A clone that converts line endings on checkout.
    git(
        &fixture.dir,
        &["clone", "-q", "-c", "core.autocrlf=true", "shop", "clone"],
    );
    let clone = fixture.dir.join("clone");
    assert_eq!(
        tree(&clone.join(".assemblr")),
        tree(&project.join(".assemblr"))
    );
    // The clone verifies its base and has nothing to compose.
    let output = succeeds(fixture.compose(&clone));
    assert!(
        stdout(&output).contains("Nothing to compose"),
        "{}",
        stdout(&output)
    );
}

/// A unit that delivers `a.secret` and `b.txt` at `version`, registered.
fn secret_unit(fixture: &Fixture, version: &str, a: &str) -> Repo {
    let secret = fixture.repo("secret");
    secret.write(
        "manifest.jsonc",
        &manifest("@acme/secret", version, &[], ""),
    );
    secret.write("src/a.secret", a);
    secret.write("src/b.txt", "b\n");
    secret.publish(version);
    fixture.register(&secret);
    secret
}

/// A global Git configuration that excludes `patterns`, in a file of the
/// fixture.
fn global_excludes(fixture: &Fixture, name: &str, patterns: &str) -> PathBuf {
    let excludes = fixture.dir.join(format!("{name}-ignores"));
    fs::write(&excludes, patterns).unwrap();
    let global = fixture.dir.join(format!("{name}-gitconfig"));
    fs::write(
        &global,
        format!(
            "[core]\n\texcludesFile = {}\n",
            excludes.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    global
}

fn compose_with(fixture: &Fixture, project: &Path, global: &Path) -> Output {
    fixture.assemblr_with_global_config(project, global, &["compose"])
}

#[test]
fn a_file_only_the_consumer_s_own_rules_ignore_is_refused_not_stored_or_left_out() {
    let fixture = Fixture::new("local-rules");
    secret_unit(&fixture, "1.0.0", "s\n");
    let global = global_excludes(&fixture, "mine", "*.secret\n");

    // The user's global excludes ignore `a.secret` on this machine only.
    // The lock cannot record it as unbased, since another clone would lock
    // it as based, and the tracked base must not receive it either.
    let project = fixture.project("shop", &[("@acme/secret", "^1")]);
    git(&project, &["init", "-q"]);
    let before = tree(&project);
    let err = stderr(&fails(compose_with(&fixture, &project, &global)));
    assert!(
        err.contains("only your own Git rules ignore, not the project's .gitignore"),
        "{err}"
    );
    assert!(err.contains("a.secret (pattern \"*.secret\" in "), "{err}");
    assert!(
        err.contains("add it to the project's .gitignore"),
        "the way out is named: {err}"
    );
    assert!(!err.contains("b.txt"), "{err}");
    assert_eq!(tree(&project), before, "nothing was written");

    // `.git/info/exclude` is the same kind of rule.
    let exclude = project.join(".git/info/exclude");
    fs::write(&exclude, "a.secret\n").unwrap();
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("a.secret (pattern \"a.secret\" in .git/info/exclude)"),
        "{err}"
    );
    assert_eq!(tree(&project), before);
    fs::write(&exclude, "").unwrap();

    // The project's own rule decides, whatever the machine ignores: the
    // file is unbased, as it is for every clone.
    fs::write(project.join(".gitignore"), "*.secret\n").unwrap();
    let output = succeeds(compose_with(&fixture, &project, &global));
    assert_eq!(
        lockfile(&project)["project"]["unbased"],
        serde_json::json!(["a.secret"])
    );
    assert!(
        stderr(&output).contains("a.secret is ignored by the project's .gitignore"),
        "{}",
        stderr(&output)
    );
    assert_eq!(tree(&project.join(".assemblr").join("objects")).len(), 1);

    // A project that re-includes the file settles it the other way, over
    // the machine's rule: it is stored.
    let kept = fixture.project("kept", &[("@acme/secret", "^1")]);
    git(&kept, &["init", "-q"]);
    fs::write(kept.join(".gitignore"), "!a.secret\n").unwrap();
    succeeds(compose_with(&fixture, &kept, &global));
    assert_eq!(lockfile(&kept)["project"]["unbased"], serde_json::json!([]));
    assert_eq!(tree(&kept.join(".assemblr").join("objects")).len(), 2);
}

#[test]
fn the_lock_does_not_depend_on_the_machine_that_composed() {
    let fixture = Fixture::new("same-lock");
    let unit = fixture.repo("env");
    unit.write("manifest.jsonc", &manifest("@acme/env", "1.0.0", &[], ""));
    unit.write("src/.env", "SECRET=1\n");
    unit.write("src/app.txt", "app\n");
    unit.publish("1.0.0");
    fixture.register(&unit);

    // Two clones of one project, whose `.gitignore` is committed, on
    // machines with different personal rules.
    let compose_clone = |name: &str, global: &Path| {
        let project = fixture.project(name, &[("@acme/env", "^1")]);
        git(&project, &["init", "-q"]);
        fs::write(project.join(".gitignore"), ".env\n").unwrap();
        succeeds(compose_with(&fixture, &project, global));
        project
    };
    let plain = compose_clone("plain", Path::new(NULL_DEVICE));
    let personal = global_excludes(&fixture, "personal", "*.bak\n.env\n.idea/\n");
    let other = compose_clone("other", &personal);

    assert_eq!(
        read(&plain.join("assemblr-lock.json")),
        read(&other.join("assemblr-lock.json"))
    );
    assert_eq!(
        tree(&plain.join(".assemblr")),
        tree(&other.join(".assemblr"))
    );
}

#[test]
fn a_stored_file_stays_stored_when_a_local_rule_appears_and_a_new_entry_is_never_silent() {
    let fixture = Fixture::new("local-later");
    let unit = fixture.repo("env");
    let publish = |version: &str, env: &str| {
        unit.write("manifest.jsonc", &manifest("@acme/env", version, &[], ""));
        unit.write("src/.env", env);
        unit.write("src/app.txt", "app\n");
        unit.publish(version);
    };
    publish("1.0.0", "SECRET=1\n");
    fixture.register(&unit);
    let project = fixture.project("shop", &[("@acme/env", "^1")]);
    git(&project, &["init", "-q"]);
    fs::write(project.join(".gitignore"), ".env\n").unwrap();
    succeeds(fixture.compose(&project));
    assert_eq!(
        lockfile(&project)["project"]["unbased"],
        serde_json::json!([".env"])
    );

    // The project stops ignoring `.env` while this machine still does. The
    // file would enter the tracked base because of a project change, but
    // against the consumer's own rule: refused, with the project untouched.
    fs::write(project.join(".gitignore"), "").unwrap();
    let mine = global_excludes(&fixture, "mine", ".env\n");
    publish("1.1.0", "SECRET=2\n");
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    declare(&project, &[("@acme/env", "^1.1")]);
    let before = tree(&project);
    let err = stderr(&fails(compose_with(&fixture, &project, &mine)));
    assert!(err.contains(".env (pattern \".env\" in "), "{err}");
    assert_eq!(tree(&project), before);

    // Without the personal rule it composes, and the file is announced as
    // entering the base. From then on a personal rule changes nothing: the
    // file is already stored.
    let output = succeeds(fixture.compose(&project));
    assert!(
        stderr(&output).contains(".env is no longer ignored by the project's .gitignore"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        lockfile(&project)["project"]["unbased"],
        serde_json::json!([])
    );
    publish("1.2.0", "SECRET=3\n");
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    declare(&project, &[("@acme/env", "^1.2")]);
    let output = succeeds(compose_with(&fixture, &project, &mine));
    assert!(
        stdout(&output).contains("  modified .env\n"),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        lockfile(&project)["project"]["unbased"],
        serde_json::json!([])
    );
}

#[test]
fn what_a_new_version_changes_beyond_values_is_reported_when_it_is_merged_into_an_edited_file() {
    let fixture = Fixture::new("merge-notes");
    let unit = fixture.repo("cfg");
    let publish = |version: &str, config: &str| {
        unit.write("manifest.jsonc", &manifest("@acme/cfg", version, &[], ""));
        unit.write("src/config.json", config);
        unit.publish(version);
    };
    publish("1.0.0", "{\n  \"a\": 1,\n  \"b\": 2\n}\n");
    fixture.register(&unit);
    let project = fixture.project("shop", &[("@acme/cfg", "^1")]);
    succeeds(fixture.compose(&project));

    // The consumer edits `a`; version 1.1 changes `b`, moves it first, and
    // adds a comment. The values merge into the consumer's text, whose
    // order and comments stay, and the build says the composition's order
    // and comment are not in the file, rather than leaving the project to
    // differ from the composition without a word.
    fs::write(
        project.join("config.json"),
        "{\n  \"a\": 5,\n  \"b\": 2\n}\n",
    )
    .unwrap();
    publish("1.1.0", "{\n  // Raised.\n  \"b\": 3,\n  \"a\": 1\n}\n");
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    declare(&project, &[("@acme/cfg", "^1.1")]);
    let output = succeeds(fixture.compose(&project));
    assert_eq!(
        read(&project.join("config.json")),
        "{\n  \"a\": 5,\n  \"b\": 3\n}\n"
    );
    let err = stderr(&output);
    assert!(
        err.contains(
            "config.json: the order of the members of / in the composition is not in your file; the \
             merge carries members and values, not comments or order"
        ),
        "{err}"
    );
    assert!(
        err.contains(
            "config.json: the comment \"// Raised.\" in the composition is not in your file"
        ),
        "{err}"
    );
}

/// A project created by `assemblr init`, with its pristine declaration.
fn initialized(fixture: &Fixture, name: &str) -> PathBuf {
    succeeds(fixture.assemblr(&fixture.dir, &["init", name]));
    fixture.dir.join(name)
}

fn add(fixture: &Fixture, project: &Path, unit: &str) -> Output {
    fixture.assemblr(project, &["add", unit])
}

fn remove(fixture: &Fixture, project: &Path, unit: &str) -> Output {
    fixture.assemblr(project, &["remove", unit])
}

fn declaration(project: &Path) -> String {
    read(&project.join("assemblr.jsonc"))
}

#[test]
fn add_selects_a_unit_records_a_caret_requirement_and_composes() {
    let fixture = Fixture::new("add");
    registry(&fixture);
    let project = initialized(&fixture, "shop");

    let output = succeeds(add(&fixture, &project, "@acme/node"));
    let out = stdout(&output);
    assert!(
        out.contains("Selected @acme/node (^1.0.0) in assemblr.jsonc."),
        "{out}"
    );
    assert!(out.contains("Composed 1 unit into 3 files."), "{out}");
    assert_eq!(read(&project.join("README.md")), "node\n");
    assert!(declaration(&project).contains("\"@acme/node\": \"^1.0.0\""));
    assert_eq!(
        lockfile(&project)["units"]["@acme/node"]["version"],
        "1.0.0"
    );

    // The next unit joins the first, the declaration's comments stay, and
    // the composition is reconciled with what is already there.
    fs::write(
        project.join("assemblr.jsonc"),
        declaration(&project).replacen("{\n", "{\n  // What this shop is made of.\n", 1),
    )
    .unwrap();
    let output = succeeds(add(&fixture, &project, "@acme/web"));
    let out = stdout(&output);
    assert!(
        out.contains("Selected @acme/web (^1.0.0) in assemblr.jsonc."),
        "{out}"
    );
    assert!(out.contains("  created web.txt\n"), "{out}");
    assert!(out.contains("  modified config/app.json\n"), "{out}");
    let text = declaration(&project);
    assert!(text.contains("// What this shop is made of."), "{text}");
    assert!(text.contains("\"@acme/web\": \"^1.0.0\""), "{text}");
    assert_eq!(
        read(&project.join("config/app.json")),
        "{\n  \"port\": 8080,\n  \"web\": true\n}\n"
    );

    // Again with the same unit: nothing changes, in the declaration or
    // the project.
    let before = tree(&project);
    let output = succeeds(add(&fixture, &project, "@acme/web"));
    assert!(
        stdout(&output)
            .contains("@acme/web is already selected (^1.0.0); assemblr.jsonc is unchanged."),
        "{}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains("Nothing to compose"),
        "{}",
        stdout(&output)
    );
    assert_eq!(tree(&project), before);
}

#[test]
fn add_promotes_a_transitive_unit_and_replaces_a_requirement_only_when_given_one() {
    let fixture = Fixture::new("add-promote");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/web", "^1")]);
    succeeds(fixture.compose(&project));
    let files = tree(&project.join("config"));

    // `@acme/node` is in the composition already, through `@acme/web`:
    // selecting it only changes the declaration, to the locked version.
    let output = succeeds(add(&fixture, &project, "@acme/node"));
    let out = stdout(&output);
    assert!(out.contains("Selected @acme/node (^1.0.0)"), "{out}");
    assert!(out.contains("Nothing to compose"), "{out}");
    assert_eq!(tree(&project.join("config")), files);
    assert!(declaration(&project).contains("\"@acme/node\": \"^1.0.0\""));

    // A unit that is selected keeps its requirement unless one is given.
    let output = succeeds(add(&fixture, &project, "@acme/node"));
    assert!(stdout(&output).contains("already selected (^1.0.0)"));
    let output = succeeds(add(&fixture, &project, "@acme/node@~1.0"));
    assert!(
        stdout(&output).contains(
            "Changed the requirement of @acme/node from ^1.0.0 to ~1.0 in assemblr.jsonc."
        ),
        "{}",
        stdout(&output)
    );
    assert!(declaration(&project).contains("\"@acme/node\": \"~1.0\""));

    // A bare version is recorded with its caret, so the file never holds a
    // requirement without an operator.
    let output = succeeds(add(&fixture, &project, "@acme/node@1.0.0"));
    assert!(
        stdout(&output).contains("from ~1.0 to ^1.0.0"),
        "{}",
        stdout(&output)
    );
    assert!(declaration(&project).contains("\"@acme/node\": \"^1.0.0\""));
}

#[test]
fn a_declared_requirement_without_an_operator_is_refused() {
    let fixture = Fixture::new("bare-requirement");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/web", "1.0.0")]);
    let before = tree(&project);
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(err.contains("has no operator"), "{err}");
    assert!(
        err.contains("\"^1.0.0\"") && err.contains("\"=1.0.0\""),
        "{err}"
    );
    assert_eq!(tree(&project), before);
}

#[test]
fn add_takes_no_option_values() {
    let fixture = Fixture::new("add-options");
    registry(&fixture);
    let project = initialized(&fixture, "shop");
    let before = tree(&project);
    let output = fixture.assemblr(&project, &["add", "@acme/node", "--option", "port=9090"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("unexpected argument '--option'"),
        "{}",
        stderr(&output)
    );
    assert_eq!(tree(&project), before);
}

#[test]
fn a_refused_add_leaves_the_declaration_and_the_project_as_they_were() {
    let fixture = Fixture::new("add-refused");
    registry(&fixture);
    let project = initialized(&fixture, "shop");
    succeeds(add(&fixture, &project, "@acme/node"));
    let before = tree(&project);

    // A unit nobody registered.
    let err = stderr(&fails(add(&fixture, &project, "@acme/missing")));
    assert!(err.contains("@acme/missing"), "{err}");
    assert_eq!(tree(&project), before);

    // A requirement that is not one, and an argument that is not a unit.
    let err = stderr(&fails(add(&fixture, &project, "@acme/web@banana")));
    assert!(
        err.contains("the version requirement is not valid"),
        "{err}"
    );
    assert!(err.contains("@owner/name@^1.2.0"), "{err}");
    // An argument that is not a unit is a usage error.
    let output = add(&fixture, &project, "web");
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("invalid unit \"web\""));
    let output = add(&fixture, &project, "@acme/web@");
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("nothing follows the last `@`"));
    assert_eq!(tree(&project), before);

    // The project's own content where the unit would create a file: a
    // conflict, which `--abort` undoes, the declaration included.
    fs::write(project.join("web.txt"), "mine\n").unwrap();
    let before = tree(&project);
    let output = fails(add(&fixture, &project, "@acme/web"));
    assert!(
        stdout(&output).contains("  conflict web.txt"),
        "{}",
        stdout(&output)
    );
    assert!(read(&project.join("web.txt")).contains("<<<<<<< current"));
    succeeds(fixture.assemblr(&project, &["compose", "--abort"]));
    assert_eq!(tree(&project), before);
    assert!(!declaration(&project).contains("@acme/web"));
}

#[test]
fn remove_retires_the_files_only_that_unit_contributed_and_keeps_what_others_need() {
    let fixture = Fixture::new("remove");
    registry(&fixture);
    let project = initialized(&fixture, "shop");
    succeeds(add(&fixture, &project, "@acme/node"));
    succeeds(add(&fixture, &project, "@acme/web"));

    // `@acme/web` goes: its file and its amendment of the config leave,
    // and what `@acme/node` delivers stays.
    let output = succeeds(remove(&fixture, &project, "@acme/web"));
    let out = stdout(&output);
    assert!(
        out.contains("Removed @acme/web from assemblr.jsonc."),
        "{out}"
    );
    assert!(out.contains("  removed web.txt\n"), "{out}");
    assert!(!project.join("web.txt").exists());
    assert_eq!(read(&project.join("README.md")), "node\n");
    assert_eq!(
        read(&project.join("config/app.json")),
        "{\n  \"port\": 3000\n}\n"
    );
    assert!(!declaration(&project).contains("@acme/web"));
    assert!(declaration(&project).contains("@acme/node"));

    // A unit that is only there because another one requires it is not
    // removable by name, and the message says why.
    let transitive = fixture.project("deps", &[("@acme/web", "^1")]);
    succeeds(fixture.compose(&transitive));
    let err = stderr(&fails(remove(&fixture, &transitive, "@acme/node")));
    assert!(
        err.contains(
            "@acme/node is not an explicit selection of this project; it is in the composition \
             only because a selected unit requires it"
        ),
        "{err}"
    );
    let err = stderr(&fails(remove(&fixture, &transitive, "@acme/nothing")));
    assert!(err.contains("is not an explicit selection"), "{err}");
    assert!(!err.contains("only because"), "{err}");

    // A unit that others still require leaves the declaration and stays in
    // the composition.
    succeeds(add(&fixture, &transitive, "@acme/node"));
    let before = tree(&transitive.join("config"));
    let output = succeeds(remove(&fixture, &transitive, "@acme/node"));
    assert!(stdout(&output).contains("Removed @acme/node from assemblr.jsonc."));
    assert_eq!(tree(&transitive.join("config")), before);
    assert!(!declaration(&transitive).contains("@acme/node"));
}

#[test]
fn remove_never_discards_an_edited_file() {
    let fixture = Fixture::new("remove-edited");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    succeeds(fixture.compose(&project));

    // A file the consumer edited is not deleted because its unit left: the
    // removal is refused and nothing changes, the declaration included.
    fs::write(project.join("README.md"), "my readme\n").unwrap();
    let before = tree(&project);
    let err = stderr(&fails(remove(&fixture, &project, "@acme/node")));
    assert!(err.contains("README.md"), "{err}");
    assert_eq!(tree(&project), before);

    fs::write(project.join("README.md"), "node\n").unwrap();
    let output = succeeds(remove(&fixture, &project, "@acme/node"));
    assert!(
        stdout(&output).contains("Removed @acme/node from assemblr.jsonc."),
        "{}",
        stdout(&output)
    );
    let text = declaration(&project);
    assert!(!text.contains("@acme/node"), "{text}");
    assert!(!project.join("README.md").exists());
    assert!(!project.join("config").exists());
    assert_eq!(
        lockfile(&project)["project"]["managed"],
        serde_json::json!([])
    );
}

#[test]
fn a_sequence_of_adds_equals_one_compose_of_the_final_declaration() {
    let fixture = Fixture::new("converge");
    registry(&fixture);
    let node_then_web = initialized(&fixture, "a");
    succeeds(add(&fixture, &node_then_web, "@acme/node"));
    succeeds(add(&fixture, &node_then_web, "@acme/web"));
    let web_then_node = initialized(&fixture, "b");
    succeeds(add(&fixture, &web_then_node, "@acme/web"));
    succeeds(add(&fixture, &web_then_node, "@acme/node"));
    let at_once = fixture.project("c", &[("@acme/node", "^1.0.0"), ("@acme/web", "^1.0.0")]);
    succeeds(fixture.compose(&at_once));

    let base = |project: &Path| lockfile(project)["project"]["base"].clone();
    assert_eq!(base(&node_then_web), base(&at_once));
    assert_eq!(base(&web_then_node), base(&at_once));
    // The files and the lock are the same; the store also keeps the bases
    // the incremental projects went through, which is its job.
    let files = |project: &Path| -> Vec<(String, Vec<u8>)> {
        tree(project)
            .into_iter()
            .filter(|(name, _)| !name.starts_with(".assemblr") && name != "assemblr.jsonc")
            .collect()
    };
    for project in [&node_then_web, &web_then_node] {
        assert_eq!(files(project), files(&at_once));
        assert_eq!(lockfile(project)["units"], lockfile(&at_once)["units"]);
        assert_eq!(read(&project.join("web.txt")), "web\n");
    }
}

#[test]
fn add_and_remove_refuse_a_read_only_declaration_before_writing_anything() {
    let fixture = Fixture::new("read-only");
    registry(&fixture);
    let project = initialized(&fixture, "shop");
    succeeds(add(&fixture, &project, "@acme/node"));
    let path = project.join("assemblr.jsonc");
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions.clone()).unwrap();
    let before = tree(&project);

    let err = stderr(&fails(add(&fixture, &project, "@acme/web")));
    assert!(err.contains("assemblr.jsonc is read-only"), "{err}");
    let err = stderr(&fails(remove(&fixture, &project, "@acme/node")));
    assert!(err.contains("assemblr.jsonc is read-only"), "{err}");
    assert_eq!(tree(&project), before);
    // A plain composition still works on it.
    succeeds(fixture.compose(&project));

    #[allow(clippy::permissions_set_readonly_false, reason = "a fixture, restored")]
    {
        permissions.set_readonly(false);
        fs::set_permissions(&path, permissions).unwrap();
    }
}

/// A project composed from `@acme/node` 1.0.0 whose README the consumer
/// edited, and version 1.1.0 of the unit published with a changed README
/// and script. Returns the project, declared at `^1.1` already.
fn text_conflict(fixture: &Fixture) -> PathBuf {
    let (node, _) = registry(fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    succeeds(fixture.compose(&project));
    fs::write(project.join("README.md"), "mine\n").unwrap();
    publish_node(
        fixture,
        &node,
        "1.1.0",
        "node v2\n",
        "#!/bin/sh\necho run v2\n",
    );
    declare(&project, &[("@acme/node", "^1.1")]);
    project
}

/// A file of the project's own where the composition creates one, which
/// Assemblr did not write: both, whole, between markers, resolved like any
/// conflict (owner decision on OC-44).
#[test]
fn a_file_assemblr_did_not_write_is_a_conflict_the_consumer_resolves() {
    let fixture = Fixture::new("existing-conflict");
    let (node, _) = registry(&fixture);
    let project = initialized(&fixture, "shop");
    fs::write(project.join("README.md"), "# Shop\n").unwrap();
    fs::create_dir(project.join("config")).unwrap();
    fs::write(project.join("config/app.json"), "{\n  \"port\": 3000\n}\n").unwrap();

    let output = fails(add(&fixture, &project, "@acme/node"));
    let out = stdout(&output);
    assert!(out.contains("  conflict README.md"), "{out}");
    assert!(out.contains("  created bin/run"), "{out}");
    assert!(
        !out.contains("config/app.json"),
        "already composed content is kept: {out}"
    );
    assert!(
        stderr(&output).contains("config/app.json already held the composed content"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        read(&project.join("README.md")),
        "<<<<<<< current (your file)\n# Shop\n\
         ||||||| base (none: no composition wrote this file)\n\
         =======\nnode\n\
         >>>>>>> incoming (the declared composition)\n"
    );
    assert!(!project.join("assemblr-lock.json").exists());

    // The consumer keeps both; the next composition reconciles the file
    // against the unit's content like any other.
    fs::write(project.join("README.md"), "# Shop\nnode\n").unwrap();
    succeeds(fixture.assemblr(&project, &["compose", "--continue"]));
    assert!(project.join("assemblr-lock.json").exists());
    publish_node(
        &fixture,
        &node,
        "1.1.0",
        "node\n",
        "#!/bin/sh\necho run v2\n",
    );
    declare(&project, &[("@acme/node", "^1.1")]);
    let output = succeeds(fixture.compose(&project));
    assert!(
        stdout(&output).contains("  modified bin/run"),
        "{}",
        stdout(&output)
    );
    assert_eq!(read(&project.join("README.md")), "# Shop\nnode\n");
}

#[test]
fn a_file_with_the_composed_content_in_crlf_is_kept() {
    let fixture = Fixture::new("existing-crlf");
    registry(&fixture);
    let project = initialized(&fixture, "shop");
    fs::write(project.join("README.md"), "node\r\n").unwrap();

    let output = succeeds(add(&fixture, &project, "@acme/node"));

    assert!(
        stderr(&output).contains("README.md already held the composed content"),
        "{}",
        stderr(&output)
    );
    assert_eq!(read(&project.join("README.md")), "node\r\n");
    succeeds(fixture.compose(&project));
    assert_eq!(read(&project.join("README.md")), "node\r\n");
}

#[cfg(unix)]
#[test]
fn a_marked_file_keeps_the_project_s_executable_bit() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("existing-mode");
    registry(&fixture);
    let project = initialized(&fixture, "shop");
    fs::create_dir(project.join("bin")).unwrap();
    fs::write(project.join("bin/run"), "#!/bin/sh\necho mine\n").unwrap();
    fs::set_permissions(project.join("bin/run"), fs::Permissions::from_mode(0o644)).unwrap();
    let mode = |project: &Path| {
        fs::metadata(project.join("bin/run"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111
    };

    fails(add(&fixture, &project, "@acme/node"));
    assert_eq!(
        mode(&project),
        0,
        "the unit's bit does not replace the project's"
    );
    fs::write(project.join("bin/run"), "#!/bin/sh\necho both\n").unwrap();
    succeeds(fixture.assemblr(&project, &["compose", "--continue"]));
    succeeds(fixture.compose(&project));
    assert_eq!(mode(&project), 0);
}

#[cfg(unix)]
#[test]
fn a_link_at_or_above_a_created_path_is_refused() {
    let fixture = Fixture::new("existing-link");
    registry(&fixture);
    let outside = fixture.dir.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("README.md"), "kept\n").unwrap();
    let project = initialized(&fixture, "shop");
    std::os::unix::fs::symlink(outside.join("README.md"), project.join("README.md")).unwrap();
    std::os::unix::fs::symlink(&outside, project.join("config")).unwrap();
    let before = tree(&project);

    let err = stderr(&fails(add(&fixture, &project, "@acme/node")));

    assert!(err.contains("cannot mark as a conflict:"), "{err}");
    assert!(err.contains("error:   README.md"), "{err}");
    assert!(err.contains("error:   config"), "{err}");
    assert_eq!(tree(&project), before);
    assert_eq!(read(&outside.join("README.md")), "kept\n");
    assert!(!outside.join("app.json").exists());
}

#[test]
fn a_binary_file_assemblr_did_not_write_is_refused() {
    let fixture = Fixture::new("existing-binary");
    registry(&fixture);
    let project = initialized(&fixture, "shop");
    fs::write(project.join("README.md"), b"\0binary").unwrap();
    let before = tree(&project);

    let err = stderr(&fails(add(&fixture, &project, "@acme/node")));

    assert!(
        err.contains("cannot mark as a conflict:\nerror:   README.md"),
        "{err}"
    );
    assert_eq!(tree(&project), before);
}

#[test]
fn a_text_conflict_is_left_marked_and_continue_finalizes_it() {
    let fixture = Fixture::new("pending-continue");
    let project = text_conflict(&fixture);

    let output = fails(fixture.compose(&project));
    let out = stdout(&output);
    assert!(out.contains("but 1 file has conflicts"), "{out}");
    assert!(out.contains("  conflict README.md"), "{out}");
    assert!(out.contains("  modified bin/run"), "{out}");
    let err = stderr(&output);
    assert!(err.contains("compose --continue"), "{err}");
    assert!(err.contains("compose --abort"), "{err}");
    // The three versions are in the file; what does not conflict arrived;
    // the lock still records the previous composition.
    let readme = read(&project.join("README.md"));
    assert!(readme.contains("<<<<<<< current"), "{readme}");
    assert!(readme.contains("\nmine\n"), "{readme}");
    assert!(readme.contains("||||||| base"), "{readme}");
    assert!(readme.contains("\nnode\n"), "{readme}");
    assert!(readme.contains("\nnode v2\n"), "{readme}");
    assert!(readme.contains(">>>>>>> incoming"), "{readme}");
    assert_eq!(read(&project.join("bin/run")), "#!/bin/sh\necho run v2\n");
    assert_eq!(
        lockfile(&project)["units"]["@acme/node"]["version"],
        "1.0.0"
    );

    // Nothing else runs while the resolution is pending, and nothing
    // changes.
    let waiting = tree(&project);
    for args in [
        &["compose"][..],
        &["add", "@acme/node@^1.1"],
        &["remove", "@acme/node"],
    ] {
        let output = fails(fixture.assemblr(&project, args));
        let err = stderr(&output);
        assert!(
            err.contains("waiting for you to resolve its conflicts in 1 file(s)")
                && err.contains("  README.md"),
            "{err}"
        );
        assert!(err.contains("--continue"), "{err}");
        assert_eq!(tree(&project), waiting, "{args:?}");
    }

    // A marker left in the file blocks the continuation.
    let output = fails(fixture.assemblr(&project, &["compose", "--continue"]));
    let err = stderr(&output);
    assert!(
        err.contains("README.md: it still holds conflict markers"),
        "{err}"
    );
    assert_eq!(tree(&project), waiting);

    // Resolved by hand, it finalizes: the lock moves, and the file is the
    // consumer's resolution.
    fs::write(project.join("README.md"), "mine and node v2\n").unwrap();
    let output = succeeds(fixture.assemblr(&project, &["compose", "--continue"]));
    assert!(
        stdout(&output).contains("Resolved 1 file and finished the composition"),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        lockfile(&project)["units"]["@acme/node"]["version"],
        "1.1.0"
    );
    assert!(!project.join(".assemblr/journal.json").exists());
    assert!(!project.join(".assemblr/previous").exists());
    assert_eq!(read(&project.join("README.md")), "mine and node v2\n");

    // The resolution is the project's own content now: composing again
    // changes nothing, and neither does continuing.
    let after = tree(&project);
    let output = succeeds(fixture.compose(&project));
    assert!(
        stdout(&output).contains("Nothing to compose"),
        "{}",
        stdout(&output)
    );
    assert_eq!(tree(&project), after);
    let err = stderr(&fails(
        fixture.assemblr(&project, &["compose", "--continue"]),
    ));
    assert!(err.contains("no composition is waiting"), "{err}");
}

#[test]
fn abort_restores_the_project_and_the_declaration_byte_for_byte() {
    let fixture = Fixture::new("pending-abort");
    let (node, _) = registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    succeeds(fixture.compose(&project));
    fs::write(project.join("README.md"), "mine\n").unwrap();
    publish_node(
        &fixture,
        &node,
        "1.1.0",
        "node v2\n",
        "#!/bin/sh\necho run v2\n",
    );
    let before = tree(&project);

    // `add` changes the declaration, writes what does not conflict, and
    // leaves the conflict marked.
    let output = fails(fixture.assemblr(&project, &["add", "@acme/node@^1.1"]));
    assert!(
        stdout(&output).contains("Changed the requirement of @acme/node from ^1 to ^1.1"),
        "{}",
        stdout(&output)
    );
    assert!(read(&project.join("assemblr.jsonc")).contains("^1.1"));
    assert!(read(&project.join("README.md")).contains("<<<<<<< current"));

    // The consumer has started resolving; abort gives all of it up.
    fs::write(project.join("README.md"), "half resolved\n").unwrap();
    let output = succeeds(fixture.assemblr(&project, &["compose", "--abort"]));
    assert!(
        stdout(&output).contains("Aborted the composition"),
        "{}",
        stdout(&output)
    );
    // Every file of the project is as it was, the store included: the
    // aborted composition's base and the objects only it named are gone,
    // and every object of the lock's base is still there.
    assert_eq!(tree(&project), before);
    assert!(!project.join(".assemblr/journal.json").exists());
    let err = stderr(&fails(fixture.assemblr(&project, &["compose", "--abort"])));
    assert!(err.contains("no composition is waiting"), "{err}");
}

#[test]
fn value_conflicts_in_json_are_marked_with_the_text_ones() {
    let fixture = Fixture::new("pending-json");
    let (node, _) = registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    succeeds(fixture.compose(&project));
    fs::write(project.join("config/app.json"), "{\n  \"port\": 9000\n}\n").unwrap();
    fs::write(project.join("README.md"), "mine\n").unwrap();
    node.write("src/config/app.json", "{\n  \"port\": 8080\n}\n");
    publish_node(
        &fixture,
        &node,
        "1.1.0",
        "node v2\n",
        "#!/bin/sh\necho run\n",
    );
    declare(&project, &[("@acme/node", "^1.1")]);

    let output = fails(fixture.compose(&project));
    let out = stdout(&output);
    assert!(out.contains("but 2 files have conflicts"), "{out}");
    assert!(out.contains("  conflict README.md"), "{out}");
    assert!(out.contains("  conflict config/app.json"), "{out}");
    let json = read(&project.join("config/app.json"));
    assert!(json.contains("9000") && json.contains("8080"), "{json}");
    assert!(json.contains("<<<<<<< current"), "{json}");

    fs::write(project.join("README.md"), "resolved\n").unwrap();
    fs::write(project.join("config/app.json"), "{\n  \"port\": 9000\n}\n").unwrap();
    succeeds(fixture.assemblr(&project, &["compose", "--continue"]));
    assert_eq!(
        read(&project.join("config/app.json")),
        "{\n  \"port\": 9000\n}\n"
    );
    assert_eq!(
        lockfile(&project)["units"]["@acme/node"]["version"],
        "1.1.0"
    );
}

#[test]
fn continue_and_abort_exclude_each_other() {
    let fixture = Fixture::new("pending-flags");
    let project = fixture.project("shop", &[]);
    let output = fixture.assemblr(&project, &["compose", "--continue", "--abort"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("cannot be used with"),
        "{}",
        stderr(&output)
    );
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

#[test]
fn a_clone_of_a_pending_project_marks_the_marked_file_again() {
    let fixture = Fixture::new("pending-clone");
    let project = text_conflict(&fixture);
    fails(fixture.compose(&project));

    // A clone carries the tracked files, not the git-ignored journal.
    let clone = fixture.dir.join("clone");
    copy_tree(&project, &clone);
    fs::remove_file(clone.join(".assemblr/journal.json")).unwrap();
    fs::remove_dir_all(clone.join(".assemblr/previous")).unwrap();

    // Its lock records the previous composition, and the marked file is
    // the project's content, so the conflict is found again, around the
    // markers that are there.
    let output = fails(fixture.compose(&clone));
    assert!(
        stdout(&output).contains("  conflict README.md"),
        "{}",
        stdout(&output)
    );
    let readme = read(&clone.join("README.md"));
    assert_eq!(readme.matches("<<<<<<< current").count(), 2, "{readme}");

    // Both sets of markers have to go.
    fs::write(clone.join("README.md"), "resolved\n").unwrap();
    succeeds(fixture.assemblr(&clone, &["compose", "--continue"]));
    assert_eq!(lockfile(&clone)["units"]["@acme/node"]["version"], "1.1.0");
}

/// A registered `@acme/guide` 1.0.0 delivering `README.md` and
/// `compose.yaml`.
fn guide(fixture: &Fixture, readme: &str) -> Repo {
    let guide = fixture.repo("guide");
    guide.write("manifest.jsonc", &manifest("@acme/guide", "1.0.0", &[], ""));
    guide.write("src/README.md", readme);
    guide.write("src/compose.yaml", "services:\n  web:\n    image: app\n");
    guide.publish("1.0.0");
    fixture.register(&guide);
    guide
}

/// Publishes `version` of `guide` with `readme` and `compose`, indexes it,
/// and declares it in `project`.
fn republish_guide(
    fixture: &Fixture,
    guide: &Repo,
    project: &Path,
    version: &str,
    readme: &str,
    compose: &str,
) {
    guide.write("manifest.jsonc", &manifest("@acme/guide", version, &[], ""));
    guide.write("src/README.md", readme);
    guide.write("src/compose.yaml", compose);
    guide.publish(version);
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    declare(project, &[("@acme/guide", &format!("^{version}"))]);
}

const NOTES: &str = "# Notes\n\nOne.\n\nTwo.\n\nThree.\n\nFour.\n\nFive.\n";

#[test]
fn a_text_file_both_sides_changed_in_different_places_is_merged_without_a_pending_state() {
    let fixture = Fixture::new("text-merged");
    let guide = guide(&fixture, NOTES);
    let project = fixture.project("shop", &[("@acme/guide", "^1")]);
    succeeds(fixture.compose(&project));
    fs::write(
        project.join("README.md"),
        NOTES.replace("Five.", "Five, mine."),
    )
    .unwrap();
    republish_guide(
        &fixture,
        &guide,
        &project,
        "1.1.0",
        &NOTES.replace("# Notes", "# Team notes"),
        "services:\n  web:\n    image: app\n",
    );

    let output = succeeds(fixture.compose(&project));
    assert!(
        stdout(&output).contains("  merged README.md"),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        read(&project.join("README.md")),
        "# Team notes\n\nOne.\n\nTwo.\n\nThree.\n\nFour.\n\nFive, mine.\n"
    );
    assert_eq!(
        lockfile(&project)["units"]["@acme/guide"]["version"],
        "1.1.0"
    );
    assert!(!project.join(".assemblr/journal.json").exists());
    // The merge is the project's own content now.
    let output = succeeds(fixture.compose(&project));
    assert!(
        stdout(&output).contains("Nothing to compose"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn overlapping_edits_mark_only_the_lines_that_disagree() {
    let fixture = Fixture::new("text-localized");
    let guide = guide(&fixture, NOTES);
    let project = fixture.project("shop", &[("@acme/guide", "^1")]);
    succeed_compose(&fixture, &project);
    fs::write(
        project.join("README.md"),
        NOTES.replace("Three.", "Three, mine."),
    )
    .unwrap();
    republish_guide(
        &fixture,
        &guide,
        &project,
        "1.1.0",
        &NOTES
            .replace("# Notes", "# Team notes")
            .replace("Three.", "Three, theirs."),
        "services:\n  web:\n    image: app\n",
    );
    let before = tree(&project);

    let output = fails(fixture.compose(&project));
    assert!(
        stdout(&output).contains("  conflict README.md"),
        "{}",
        stdout(&output)
    );
    // The title the composition changed is applied; only the line both
    // sides changed is between markers, not the whole file.
    let readme = read(&project.join("README.md"));
    assert_eq!(
        readme,
        "# Team notes\n\nOne.\n\nTwo.\n\n\
         <<<<<<< current (your file)\nThree, mine.\n\
         ||||||| base (the last composition)\nThree.\n\
         =======\nThree, theirs.\n\
         >>>>>>> incoming (the declared composition)\n\
         \nFour.\n\nFive.\n"
    );
    assert_eq!(
        lockfile(&project)["units"]["@acme/guide"]["version"],
        "1.0.0"
    );

    // Marked lines block the continuation, and resolving them finalizes it.
    let err = stderr(&fails(
        fixture.assemblr(&project, &["compose", "--continue"]),
    ));
    assert!(
        err.contains("README.md: it still holds conflict markers"),
        "{err}"
    );
    fs::write(
        project.join("README.md"),
        readme
            .replace("<<<<<<< current (your file)\nThree, mine.\n||||||| base (the last composition)\nThree.\n=======\nThree, theirs.\n>>>>>>> incoming (the declared composition)\n", "Three, ours.\n"),
    )
    .unwrap();
    succeeds(fixture.assemblr(&project, &["compose", "--continue"]));
    assert_eq!(
        lockfile(&project)["units"]["@acme/guide"]["version"],
        "1.1.0"
    );
    assert!(read(&project.join("README.md")).contains("Three, ours.\n\nFour.\n"));

    // Abort gives it all up: the file the consumer edited comes back.
    let fixture = Fixture::new("text-localized-abort");
    let guide = self::guide(&fixture, NOTES);
    let project = fixture.project("shop", &[("@acme/guide", "^1")]);
    succeed_compose(&fixture, &project);
    fs::write(
        project.join("README.md"),
        NOTES.replace("Three.", "Three, mine."),
    )
    .unwrap();
    republish_guide(
        &fixture,
        &guide,
        &project,
        "1.1.0",
        &NOTES.replace("Three.", "Three, theirs."),
        "services:\n  web:\n    image: app\n",
    );
    fails(fixture.compose(&project));
    succeeds(fixture.assemblr(&project, &["compose", "--abort"]));
    assert_eq!(
        read(&project.join("README.md")),
        NOTES.replace("Three.", "Three, mine.")
    );
    let _ = before;
}

#[test]
fn continue_refuses_a_base_section_left_behind_but_not_a_heading_underline() {
    let fixture = Fixture::new("text-leftover");
    let guide = guide(&fixture, NOTES);
    let project = fixture.project("shop", &[("@acme/guide", "^1")]);
    succeed_compose(&fixture, &project);
    fs::write(
        project.join("README.md"),
        NOTES.replace("Three.", "Three, mine."),
    )
    .unwrap();
    republish_guide(
        &fixture,
        &guide,
        &project,
        "1.1.0",
        &NOTES.replace("Three.", "Three, theirs."),
        "services:\n  web:\n    image: app\n",
    );
    fails(fixture.compose(&project));
    let marked = read(&project.join("README.md"));

    // Only the opening and closing lines are removed: the base section and
    // the separator are left, and the continuation is refused.
    let partial: String = marked
        .split_inclusive('\n')
        .filter(|line| !line.starts_with("<<<<<<< ") && !line.starts_with(">>>>>>> "))
        .collect();
    assert!(partial.contains("||||||| base"), "{partial}");
    fs::write(project.join("README.md"), &partial).unwrap();
    let waiting = tree(&project);
    let err = stderr(&fails(
        fixture.assemblr(&project, &["compose", "--continue"]),
    ));
    assert!(
        err.contains("README.md: it still holds conflict markers"),
        "{err}"
    );
    assert!(err.contains("||||||| base"), "{err}");
    assert_eq!(tree(&project), waiting);

    // A resolution whose text underlines a heading with `=` is accepted.
    let resolved = NOTES.replace("Three.", "Three\n=======\n\nOurs.");
    fs::write(project.join("README.md"), &resolved).unwrap();
    succeeds(fixture.assemblr(&project, &["compose", "--continue"]));
    assert_eq!(read(&project.join("README.md")), resolved);
    assert_eq!(
        lockfile(&project)["units"]["@acme/guide"]["version"],
        "1.1.0"
    );
}

#[test]
fn abort_after_update_leaves_a_committed_project_clean_for_git() {
    let fixture = Fixture::new("abort-clean");
    let guide = guide(&fixture, NOTES);
    let project = fixture.project("shop", &[("@acme/guide", "^1")]);
    git(&project, &["init", "-q"]);
    succeed_compose(&fixture, &project);
    fs::write(
        project.join("README.md"),
        NOTES.replace("Three.", "Three, mine."),
    )
    .unwrap();
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-q", "-m", "composed"]);
    // A release that changes the same line and another file.
    guide.write("manifest.jsonc", &manifest("@acme/guide", "1.1.0", &[], ""));
    guide.write("src/README.md", &NOTES.replace("Three.", "Three, theirs."));
    guide.write("src/compose.yaml", "services:\n  web:\n    image: app:2\n");
    guide.publish("1.1.0");
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));

    fails(fixture.assemblr(&project, &["update"]));
    assert!(!git(&project, &["status", "--porcelain"]).is_empty());
    succeeds(fixture.assemblr(&project, &["compose", "--abort"]));
    assert_eq!(
        git(
            &project,
            &["status", "--porcelain", "--untracked-files=all"]
        ),
        ""
    );
    // The project still composes from its base, which was kept whole.
    succeeds(fixture.compose(&project));
}

fn succeed_compose(fixture: &Fixture, project: &Path) {
    succeeds(fixture.compose(project));
}

#[test]
fn insertions_at_one_position_by_both_sides_are_a_conflict_not_an_order() {
    let fixture = Fixture::new("text-same-position");
    let guide = guide(&fixture, NOTES);
    let project = fixture.project("shop", &[("@acme/guide", "^1")]);
    succeed_compose(&fixture, &project);
    fs::write(project.join("README.md"), format!("{NOTES}\nMine.\n")).unwrap();
    republish_guide(
        &fixture,
        &guide,
        &project,
        "1.1.0",
        &format!("{NOTES}\nTheirs.\n"),
        "services:\n  web:\n    image: app\n",
    );
    let output = fails(fixture.compose(&project));
    assert!(
        stdout(&output).contains("  conflict README.md"),
        "{}",
        stdout(&output)
    );
    let readme = read(&project.join("README.md"));
    assert!(
        readme.contains(
            "\n<<<<<<< current (your file)\nMine.\n||||||| base (the last composition)\n=======\nTheirs.\n>>>>>>> incoming (the declared composition)\n"
        ),
        "{readme}"
    );
    assert!(readme.starts_with("# Notes\n"), "{readme}");
}

#[test]
fn crlf_text_is_merged_and_marked_with_its_own_line_endings() {
    let fixture = Fixture::new("text-crlf");
    let guide = guide(&fixture, NOTES);
    let project = fixture.project("shop", &[("@acme/guide", "^1")]);
    succeed_compose(&fixture, &project);
    let crlf = |text: &str| text.replace('\n', "\r\n");
    fs::write(
        project.join("README.md"),
        crlf(&NOTES.replace("Five.", "Five, mine.")),
    )
    .unwrap();
    republish_guide(
        &fixture,
        &guide,
        &project,
        "1.1.0",
        &NOTES.replace("# Notes", "# Team notes"),
        "services:\n  web:\n    image: app\n",
    );
    succeeds(fixture.compose(&project));
    assert_eq!(
        fs::read(project.join("README.md")).unwrap(),
        crlf("# Team notes\n\nOne.\n\nTwo.\n\nThree.\n\nFour.\n\nFive, mine.\n").into_bytes()
    );

    // A conflict is marked with CRLF line endings too.
    republish_guide(
        &fixture,
        &guide,
        &project,
        "1.2.0",
        &NOTES
            .replace("# Notes", "# Team notes")
            .replace("Five.", "Five, theirs."),
        "services:\n  web:\n    image: app\n",
    );
    fails(fixture.compose(&project));
    let readme = fs::read(project.join("README.md")).unwrap();
    let text = String::from_utf8(readme).unwrap();
    assert!(
        text.contains(
            "<<<<<<< current (your file)\r\nFive, mine.\r\n||||||| base (the last composition)\r\nFive.\r\n=======\r\nFive, theirs.\r\n>>>>>>> incoming (the declared composition)\r\n"
        ),
        "{text:?}"
    );
    assert!(!text.replace("\r\n", "").contains('\n'), "{text:?}");
}

#[test]
fn a_structured_file_both_sides_changed_is_still_marked_whole() {
    let fixture = Fixture::new("text-structured");
    let guide = guide(&fixture, NOTES);
    let project = fixture.project("shop", &[("@acme/guide", "^1")]);
    succeed_compose(&fixture, &project);
    fs::write(
        project.join("compose.yaml"),
        "services:\n  web:\n    image: mine\n",
    )
    .unwrap();
    republish_guide(
        &fixture,
        &guide,
        &project,
        "1.1.0",
        NOTES,
        "services:\n  web:\n    image: theirs\n    ports: [\"80\"]\n",
    );
    fails(fixture.compose(&project));
    // A line merge could not tell a key twice from data, so the whole file
    // is marked: the project's, the base's, and the composition's.
    let yaml = read(&project.join("compose.yaml"));
    assert!(
        yaml.starts_with("<<<<<<< current (your file)\nservices:\n"),
        "{yaml}"
    );
    assert!(yaml.contains("image: mine"), "{yaml}");
    assert!(yaml.contains("image: app"), "{yaml}");
    assert!(yaml.contains("image: theirs"), "{yaml}");
}

#[test]
fn a_small_binary_file_both_sides_changed_is_a_conflict_never_a_line_merge() {
    // Valid UTF-8 with NUL bytes behind a text path: both sides changed it
    // in different places, which a line merge would combine silently.
    let fixture = Fixture::new("text-binary");
    let blob = "head\n\0\nmiddle\n\0\nmiddle\n\0\nmiddle\n\0\ntail\n";
    let guide = guide(&fixture, NOTES);
    guide.write("src/data.txt", blob);
    guide.write("manifest.jsonc", &manifest("@acme/guide", "1.0.1", &[], ""));
    guide.publish("1.0.1");
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    let project = fixture.project("shop", &[("@acme/guide", "^1")]);
    succeed_compose(&fixture, &project);
    fs::write(project.join("data.txt"), blob.replace("tail", "mine")).unwrap();
    guide.write("src/data.txt", &blob.replace("head", "theirs"));
    guide.write("manifest.jsonc", &manifest("@acme/guide", "1.1.0", &[], ""));
    guide.publish("1.1.0");
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    declare(&project, &[("@acme/guide", "^1.1")]);
    let before = tree(&project);
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(err.contains("data.txt"), "{err}");
    assert!(err.contains("is not UTF-8 text"), "{err}");
    assert_eq!(tree(&project), before);
}

fn update(fixture: &Fixture, project: &Path, units: &[&str]) -> Output {
    let mut args = vec!["update"];
    args.extend_from_slice(units);
    fixture.assemblr(project, &args)
}

fn locked(project: &Path, unit: &str) -> String {
    lockfile(project)["units"][unit]["version"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn update_moves_the_locked_versions_within_the_declared_requirements_and_never_edits_the_declaration()
 {
    let fixture = Fixture::new("update-all");
    let (node, _) = registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    succeeds(fixture.compose(&project));
    let declaration = read(&project.join("assemblr.jsonc"));
    publish_node(
        &fixture,
        &node,
        "1.1.0",
        "node v2\n",
        "#!/bin/sh\necho run v2\n",
    );
    // 2.0.0 is outside the requirement and stays out.
    node.write("manifest.jsonc", &manifest("@acme/node", "2.0.0", &[], ""));
    node.publish("2.0.0");
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));

    // Plain `compose` keeps the lock.
    let output = succeeds(fixture.compose(&project));
    assert!(
        stdout(&output).contains("Nothing to compose"),
        "{}",
        stdout(&output)
    );
    assert_eq!(locked(&project, "@acme/node"), "1.0.0");

    let output = succeeds(update(&fixture, &project, &[]));
    assert!(
        stdout(&output).contains("Updated: @acme/node 1.0.0 -> 1.1.0."),
        "{}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains("  modified README.md"),
        "{}",
        stdout(&output)
    );
    assert_eq!(locked(&project, "@acme/node"), "1.1.0");
    assert_eq!(read(&project.join("README.md")), "node v2\n");
    assert_eq!(read(&project.join("assemblr.jsonc")), declaration);

    // Nothing more to do: the same command touches nothing.
    let before = tree(&project);
    let output = succeeds(update(&fixture, &project, &[]));
    assert!(
        stdout(&output).contains("Nothing to update: every unit is at the newest version"),
        "{}",
        stdout(&output)
    );
    assert_eq!(tree(&project), before);
}

#[test]
fn update_records_the_new_version_in_the_lock_even_when_no_file_changes() {
    let fixture = Fixture::new("update-lock-only");
    let (node, _) = registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    succeeds(fixture.compose(&project));
    let before_lock = read(&project.join("assemblr-lock.json"));
    // 1.0.1 delivers exactly what 1.0.0 did.
    node.write(
        "manifest.jsonc",
        &read(&node.path.join("manifest.jsonc")).replace("1.0.0", "1.0.1"),
    );
    node.publish("1.0.1");
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
    let files_before = tree(&project)
        .into_iter()
        .filter(|(path, _)| path != "assemblr-lock.json")
        .collect::<Vec<_>>();

    let output = succeed_update(&fixture, &project, &[]);
    assert!(
        stdout(&output).contains("Updated: @acme/node 1.0.0 -> 1.0.1."),
        "{}",
        stdout(&output)
    );
    assert_eq!(locked(&project, "@acme/node"), "1.0.1");
    assert_ne!(read(&project.join("assemblr-lock.json")), before_lock);
    let files_after = tree(&project)
        .into_iter()
        .filter(|(path, _)| path != "assemblr-lock.json")
        .collect::<Vec<_>>();
    assert_eq!(files_after, files_before);
    // The lock also records the provenance of what was composed.
    let lock = lockfile(&project);
    assert_eq!(
        lock["units"]["@acme/node"]["commit"],
        node.git(&["rev-parse", "v1.0.1"])
    );
}

fn succeed_update(fixture: &Fixture, project: &Path, units: &[&str]) -> Output {
    succeeds(update(fixture, project, units))
}

#[test]
fn naming_a_unit_updates_only_it_and_what_it_forces_and_the_report_says_so() {
    let fixture = Fixture::new("update-named");
    let (node, _) = registry(&fixture);
    // `@acme/app` requires node ^1.0 and is published at 1.0.0 and 1.1.0; the
    // second needs node ^1.1.
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "1.0.0", &[("@acme/node", "^1")], ""),
    );
    app.write("src/app.txt", "app 1.0\n");
    app.publish("1.0.0");
    fixture.register(&app);
    let other = fixture.repo("other");
    other.write("manifest.jsonc", &manifest("@acme/other", "1.0.0", &[], ""));
    other.write("src/other.txt", "other 1.0\n");
    other.publish("1.0.0");
    fixture.register(&other);
    let project = fixture.project("shop", &[("@acme/app", "^1"), ("@acme/other", "^1")]);
    succeeds(fixture.compose(&project));

    publish_node(&fixture, &node, "1.1.0", "node v2\n", "#!/bin/sh\n");
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "1.1.0", &[("@acme/node", "^1.1")], ""),
    );
    app.write("src/app.txt", "app 1.1\n");
    app.publish("1.1.0");
    other.write("manifest.jsonc", &manifest("@acme/other", "1.1.0", &[], ""));
    other.write("src/other.txt", "other 1.1\n");
    other.publish("1.1.0");
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));

    // Naming `@acme/app` moves it, and node with it, since app 1.1 needs
    // node ^1.1; `@acme/other` stays.
    let output = succeed_update(&fixture, &project, &["@acme/app"]);
    let out = stdout(&output);
    assert!(out.contains("@acme/app 1.0.0 -> 1.1.0"), "{out}");
    assert!(out.contains("@acme/node 1.0.0 -> 1.1.0"), "{out}");
    assert!(!out.contains("@acme/other"), "{out}");
    let err = stderr(&output);
    assert!(
        err.contains("@acme/node moved although not named: the versions selected for the named unit require it; to keep it where it is, do not update that unit"),
        "{err}"
    );
    assert_eq!(locked(&project, "@acme/other"), "1.0.0");
    assert_eq!(read(&project.join("other.txt")), "other 1.0\n");

    // A transitive unit can be named too: `@acme/node` is in the project
    // only because `@acme/app` requires it, and moving it alone leaves the
    // two selected units where they are.
    node.write("manifest.jsonc", &manifest("@acme/node", "1.2.0", &[], ""));
    publish_node(&fixture, &node, "1.2.0", "node v3\n", "#!/bin/sh\n");
    let output = succeed_update(&fixture, &project, &["@acme/node"]);
    let out = stdout(&output);
    assert!(out.contains("@acme/node 1.1.0 -> 1.2.0"), "{out}");
    assert!(
        !out.contains("@acme/app") && !out.contains("@acme/other"),
        "{out}"
    );
    assert_eq!(locked(&project, "@acme/app"), "1.1.0");
    assert_eq!(locked(&project, "@acme/other"), "1.0.0");
    assert_eq!(read(&project.join("README.md")), "node v3\n");

    // A selected unit named alone leaves the transitive one where it is.
    let output = succeed_update(&fixture, &project, &["@acme/other"]);
    assert!(
        stdout(&output).contains("@acme/other 1.0.0 -> 1.1.0"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn update_refuses_unknown_units_and_projects_never_composed_and_changes_nothing() {
    let fixture = Fixture::new("update-refused");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    let err = stderr(&fails(update(&fixture, &project, &[])));
    assert!(err.contains("never composed"), "{err}");
    assert!(err.contains("run `assemblr compose` first"), "{err}");
    succeeds(fixture.compose(&project));
    let before = tree(&project);
    let err = stderr(&fails(update(&fixture, &project, &["@acme/web"])));
    assert!(
        err.contains("@acme/web is not part of the composition this project holds"),
        "{err}"
    );
    assert!(err.contains("@acme/node"), "{err}");
    let output = update(&fixture, &project, &["@acme/node@^1"]);
    assert_eq!(output.status.code(), Some(2));
    let err = stderr(&output);
    assert!(err.contains("includes a version"), "{err}");
    assert!(
        err.contains("assemblr add @acme/node@<requirement>"),
        "{err}"
    );
    assert_eq!(tree(&project), before);
}

#[test]
fn an_update_reconciles_edits_like_compose_and_a_conflict_waits_for_continue_or_abort() {
    let fixture = Fixture::new("update-conflict");
    let (node, _) = registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    succeeds(fixture.compose(&project));
    fs::write(project.join("README.md"), "mine\n").unwrap();
    publish_node(
        &fixture,
        &node,
        "1.1.0",
        "node v2\n",
        "#!/bin/sh\necho run v2\n",
    );
    let before_abort = tree(&project);

    let output = fails(update(&fixture, &project, &[]));
    assert!(
        stdout(&output).contains("  conflict README.md"),
        "{}",
        stdout(&output)
    );
    assert_eq!(read(&project.join("bin/run")), "#!/bin/sh\necho run v2\n");
    assert_eq!(locked(&project, "@acme/node"), "1.0.0");

    // Nothing else runs while it waits.
    let err = stderr(&fails(update(&fixture, &project, &[])));
    assert!(
        err.contains("--continue") && err.contains("--abort"),
        "{err}"
    );

    // Abort restores the project as it was before the update.
    succeeds(fixture.assemblr(&project, &["compose", "--abort"]));
    let outside_store = |files: Vec<(String, Vec<u8>)>| -> Vec<(String, Vec<u8>)> {
        files
            .into_iter()
            .filter(|(path, _)| {
                !path.starts_with(".assemblr/bases/") && !path.starts_with(".assemblr/objects/")
            })
            .collect()
    };
    assert_eq!(outside_store(tree(&project)), outside_store(before_abort));

    // Again, resolved by hand and continued: the lock moves.
    fails(update(&fixture, &project, &[]));
    fs::write(project.join("README.md"), "mine and node v2\n").unwrap();
    succeeds(fixture.assemblr(&project, &["compose", "--continue"]));
    assert_eq!(locked(&project, "@acme/node"), "1.1.0");
}

#[test]
fn an_update_applies_amendments_across_versions_and_a_collision_refuses_it_whole() {
    let fixture = Fixture::new("update-amendments");
    let (node, _web) = registry(&fixture);
    // `@acme/shell` accepts any node 1.x and amends node's JSON file at 1.0.0.
    let shell = fixture.repo("shell");
    shell.write(
        "manifest.jsonc",
        &manifest(
            "@acme/shell",
            "1.0.0",
            &[("@acme/node", "^1")],
            r#", "amendments": {"config/app.json": {"unit": "@acme/node", "base": "1.0.0"}}"#,
        ),
    );
    shell.write(
        "amendments/config/app.json",
        "{\n  \"port\": 3000,\n  \"shell\": true\n}\n",
    );
    shell.write(
        "assemblr-lock.json",
        &format!(
            "{{\n  \"format\": 1,\n  \"units\": {{\n    \"@acme/node\": {{\n      \"commit\": \"{}\",\n      \"tree\": \"{}\",\n      \"version\": \"1.0.0\"\n    }}\n  }}\n}}\n",
            node.git(&["rev-parse", "v1.0.0"]),
            node.git(&["rev-parse", "v1.0.0^{tree}"])
        ),
    );
    shell.publish("1.0.0");
    fixture.register(&shell);
    let project = fixture.project("shop", &[("@acme/shell", "^1")]);
    succeeds(fixture.compose(&project));

    // 1.1.0 changes other files: the amender is not published again.
    publish_node(
        &fixture,
        &node,
        "1.1.0",
        "node v2\n",
        "#!/bin/sh\necho v2\n",
    );
    succeed_update(&fixture, &project, &[]);
    assert_eq!(locked(&project, "@acme/node"), "1.1.0");
    assert_eq!(locked(&project, "@acme/shell"), "1.0.0");
    assert!(read(&project.join("config/app.json")).contains("\"shell\": true"));

    // 1.2.0 sets the key the amender sets: the update is refused whole.
    node.write(
        "src/config/app.json",
        "{\n  \"port\": 3000,\n  \"shell\": false\n}\n",
    );
    publish_node(
        &fixture,
        &node,
        "1.2.0",
        "node v3\n",
        "#!/bin/sh\necho v3\n",
    );
    let before = tree(&project);
    let err = stderr(&fails(update(&fixture, &project, &[])));
    assert!(
        err.contains("the amendment of @acme/shell was taken from @acme/node 1.0.0"),
        "{err}"
    );
    assert!(err.contains("`assemblr add @acme/node@=1.0.0`"), "{err}");
    // An update of every unit says how to update the others.
    assert!(
        err.contains("`assemblr update` with the units to move updates only those"),
        "{err}"
    );
    assert_eq!(tree(&project), before);
    assert_eq!(locked(&project, "@acme/node"), "1.1.0");
}

#[test]
fn a_journal_from_a_newer_version_is_refused_by_every_command_and_nothing_changes() {
    let fixture = Fixture::new("journal-newer");
    let project = text_conflict(&fixture);
    fails(fixture.compose(&project));
    // A later version recorded something this one does not know about the
    // waiting composition.
    let path = project.join(".assemblr/journal.json");
    let mut journal: Value = serde_json::from_str(&read(&path)).unwrap();
    assert_eq!(journal["format"], 3);
    journal["resolved"] = Value::Array(vec![]);
    fs::write(&path, serde_json::to_string_pretty(&journal).unwrap()).unwrap();
    let waiting = tree(&project);

    for args in [
        &["compose"][..],
        &["compose", "--continue"],
        &["compose", "--abort"],
        &["update"],
        &["add", "@acme/node@^1.1"],
    ] {
        let err = stderr(&fails(fixture.assemblr(&project, args)));
        assert!(
            err.contains("has the member `resolved`, which this version of Assemblr does not know"),
            "{args:?}: {err}"
        );
        assert!(
            err.contains("the journal may come from a newer Assemblr"),
            "{args:?}: {err}"
        );
        assert_eq!(tree(&project), waiting, "{args:?}");
    }

    // A newer format number is refused the same way.
    journal.as_object_mut().unwrap().remove("resolved");
    journal["format"] = Value::from(4);
    fs::write(&path, serde_json::to_string_pretty(&journal).unwrap()).unwrap();
    let waiting = tree(&project);
    let err = stderr(&fails(fixture.assemblr(&project, &["compose", "--abort"])));
    assert!(err.contains("it has format 4"), "{err}");
    assert_eq!(tree(&project), waiting);
}

fn status(fixture: &Fixture, project: &Path, args: &[&str]) -> Output {
    let mut all = vec!["status"];
    all.extend_from_slice(args);
    fixture.assemblr(project, &all)
}

fn status_json(fixture: &Fixture, project: &Path) -> Value {
    serde_json::from_str(&stdout(&succeeds(status(fixture, project, &["--json"])))).unwrap()
}

/// `status` reads a project at every step of its life without changing it:
/// what it would compose, its units and what is published beyond them, the
/// files edited since, and which units deliver each file.
#[test]
fn status_reports_a_project_without_changing_it() {
    let fixture = Fixture::new("status");
    let (node, _) = registry(&fixture);
    let project = fixture.project("shop", &[("@acme/web", "^1")]);

    let before = tree(&project);
    let out = stdout(&succeeds(status(&fixture, &project, &[])));
    assert!(out.contains("The project was never composed."), "{out}");
    assert!(out.contains("  @acme/web (^1): not composed yet"), "{out}");
    assert!(out.contains("Next: run `assemblr compose`"), "{out}");
    let json = status_json(&fixture, &project);
    assert_eq!(json["state"], "uncomposed");
    assert_eq!(json["up_to_date"], Value::Null);
    assert_eq!(json["units"][0]["unit"], "@acme/web");
    assert_eq!(json["units"][0]["version"], Value::Null);
    assert_eq!(tree(&project), before);

    succeeds(fixture.compose(&project));
    let composed = tree(&project);
    let out = stdout(&succeeds(status(&fixture, &project, &["--files"])));
    assert!(
        out.contains("Composed from 2 units; `assemblr compose` has nothing to do."),
        "{out}"
    );
    assert!(
        out.contains("  @acme/node 1.0.0 (required by @acme/web)\n"),
        "{out}"
    );
    assert!(out.contains("  @acme/web 1.0.0 (^1)\n"), "{out}");
    assert!(
        out.contains("  config/app.json: from @acme/node, amended by @acme/web"),
        "{out}"
    );
    assert!(out.contains("  web.txt: from @acme/web"), "{out}");
    assert!(!out.contains("Edited"), "{out}");
    assert!(!out.contains("Next:"), "{out}");
    let json = status_json(&fixture, &project);
    assert_eq!(json["state"], "composed");
    assert_eq!(json["up_to_date"], true);
    assert_eq!(json["units"][0]["required_by"][0], "@acme/web");
    assert_eq!(json["units"][1]["requirement"], "^1");
    let files = json["files"].as_array().unwrap();
    let app = files
        .iter()
        .find(|file| file["path"] == "config/app.json")
        .unwrap();
    assert_eq!(app["unit"], "@acme/node");
    assert_eq!(app["amended_by"][0], "@acme/web");
    assert_eq!(tree(&project), composed);

    // Edits since the composition: CRLF line endings on the composed
    // content are not one.
    fs::write(project.join("README.md"), "mine\n").unwrap();
    fs::remove_file(project.join("web.txt")).unwrap();
    fs::write(
        project.join("config/app.json"),
        read(&project.join("config/app.json")).replace('\n', "\r\n"),
    )
    .unwrap();
    let out = stdout(&succeeds(status(&fixture, &project, &[])));
    assert!(out.contains("Edited since the last composition:"), "{out}");
    assert!(
        out.contains("  README.md: modified, from @acme/node"),
        "{out}"
    );
    assert!(out.contains("  web.txt: deleted, from @acme/web"), "{out}");
    assert!(!out.contains("config/app.json"), "{out}");
    let json = status_json(&fixture, &project);
    assert_eq!(
        json["edited"],
        serde_json::json!([
            {"path": "README.md", "edit": "modified"},
            {"path": "web.txt", "edit": "deleted"},
        ])
    );
    // Edits are not a reason to compose.
    assert_eq!(json["up_to_date"], true);

    // A version the requirements do not allow is told apart from an update.
    publish_node(&fixture, &node, "1.1.0", "node v2\n", "#!/bin/sh\n");
    let out = stdout(&succeeds(status(&fixture, &project, &[])));
    assert!(
        out.contains(
            "  @acme/node 1.0.0 (required by @acme/web): 1.1.0 published, outside what the \
             units that require it allow"
        ),
        "{out}"
    );
    let json = status_json(&fixture, &project);
    assert_eq!(json["units"][0]["update"], Value::Null);
    assert_eq!(json["units"][0]["newest"], "1.1.0");

    // A selection whose own requirement allows the version is held back by
    // the unit that requires it, which `add` cannot change.
    declare(&project, &[("@acme/web", "^1"), ("@acme/node", "^1")]);
    let out = stdout(&succeeds(status(&fixture, &project, &[])));
    assert!(
        out.contains(
            "  @acme/node 1.0.0 (^1; also required by @acme/web): 1.1.0 published, outside \
             what the units that require it allow"
        ),
        "{out}"
    );
    assert!(!out.contains("assemblr add"), "{out}");

    // A declaration edited without changing the composition is not a
    // reason to compose; one that changes it is.
    declare(&project, &[("@acme/web", "^1")]);
    let text = read(&project.join("assemblr.jsonc"));
    fs::write(
        project.join("assemblr.jsonc"),
        format!("// Edited.\n{text}"),
    )
    .unwrap();
    let out = stdout(&succeeds(status(&fixture, &project, &[])));
    assert!(
        out.contains("`assemblr compose` has nothing to do"),
        "{out}"
    );
    declare(&project, &[("@acme/node", "^1")]);
    let out = stdout(&succeeds(status(&fixture, &project, &[])));
    assert!(
        out.contains("assemblr.jsonc changed since, so `assemblr compose` would change"),
        "{out}"
    );
    assert!(
        out.contains("  @acme/node 1.0.0 (^1): update to 1.1.0 available"),
        "{out}"
    );
    assert!(
        out.contains("  @acme/web 1.0.0 (no longer selected)"),
        "{out}"
    );
    assert!(
        out.contains("Next: run `assemblr compose` to apply assemblr.jsonc"),
        "{out}"
    );
    let before = tree(&project);
    assert_eq!(status_json(&fixture, &project)["up_to_date"], false);
    assert_eq!(tree(&project), before);
}

/// A composition waiting for its conflicts is reported from the lock it
/// keeps, with each marked file and whether it still holds markers.
#[test]
fn status_reports_a_pending_composition_and_what_is_left_to_resolve() {
    let fixture = Fixture::new("status-pending");
    let project = text_conflict(&fixture);
    fails(fixture.compose(&project));
    let waiting = tree(&project);

    let out = stdout(&succeeds(status(&fixture, &project, &[])));
    assert!(
        out.contains("A composition is waiting for you to resolve conflicts in 1 file:"),
        "{out}"
    );
    assert!(
        out.contains("  README.md: holds conflict markers, from @acme/node"),
        "{out}"
    );
    // The versions are the ones the waiting composition locks.
    assert!(out.contains("  @acme/node 1.1.0 (^1.1)"), "{out}");
    assert!(out.contains("compose --continue"), "{out}");
    assert!(out.contains("compose --abort"), "{out}");
    let json = status_json(&fixture, &project);
    assert_eq!(json["state"], "pending");
    assert_eq!(
        json["pending"],
        serde_json::json!([{"path": "README.md", "marked": true}])
    );
    assert_eq!(json["edited"], serde_json::json!([]));
    assert_eq!(tree(&project), waiting);

    fs::write(project.join("README.md"), "mine and node v2\n").unwrap();
    // `--continue` refuses a declaration edited since, so status does not
    // send the consumer there.
    let planned = declaration(&project);
    fs::write(project.join("assemblr.jsonc"), format!("{planned}\n")).unwrap();
    let output = succeeds(status(&fixture, &project, &[]));
    assert!(
        stdout(&output).contains("Next: put assemblr.jsonc back as the composition left it"),
        "{}",
        stdout(&output)
    );
    assert!(
        stderr(&output).contains("warning: assemblr.jsonc changed"),
        "{}",
        stderr(&output)
    );
    fs::write(project.join("assemblr.jsonc"), planned).unwrap();
    let out = stdout(&succeeds(status(&fixture, &project, &[])));
    assert!(out.contains("  README.md: resolved"), "{out}");
    assert!(
        out.contains("Next: run `assemblr compose --continue` to finish the composition"),
        "{out}"
    );
    succeeds(fixture.assemblr(&project, &["compose", "--continue"]));
    let out = stdout(&succeeds(status(&fixture, &project, &[])));
    assert!(out.contains("Composed from 1 unit;"), "{out}");
    assert!(out.contains("  README.md: modified"), "{out}");
}

#[test]
fn status_reports_an_interrupted_composition() {
    let fixture = Fixture::new("status-interrupted");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    succeeds(fixture.compose(&project));
    // The journal of a composition interrupted before its lock.
    let base = lockfile(&project)["project"]["base"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::remove_file(project.join("assemblr-lock.json")).unwrap();
    fs::write(
        project.join(".assemblr/journal.json"),
        format!("{{\"format\": 1, \"base\": \"{base}\", \"files\": [], \"directories\": []}}\n"),
    )
    .unwrap();

    let out = stdout(&succeeds(status(&fixture, &project, &[])));
    assert!(out.contains("A composition was interrupted."), "{out}");
    assert!(out.contains("Next: run `assemblr compose`"), "{out}");
    assert_eq!(status_json(&fixture, &project)["state"], "interrupted");
}

#[test]
fn status_refuses_a_directory_that_is_not_a_project() {
    let fixture = Fixture::new("status-refused");
    let (node, _) = registry(&fixture);

    let err = stderr(&fails(status(&fixture, &node.path, &[])));
    assert!(err.contains("assemblr unit check"), "{err}");
    let empty = fixture.dir.join("empty");
    fs::create_dir(&empty).unwrap();
    let err = stderr(&fails(status(&fixture, &empty, &[])));
    assert!(err.contains("no assemblr.jsonc"), "{err}");
    assert!(err.contains("assemblr init"), "{err}");
}

/// What keeps a composition from being planned is reported, and the rest
/// of the status still is.
#[test]
fn status_reports_what_keeps_a_composition_from_being_planned() {
    let fixture = Fixture::new("status-unplanned");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    succeeds(fixture.compose(&project));
    declare(&project, &[("@acme/node", "^1"), ("@acme/db", "^1")]);

    let output = succeeds(status(&fixture, &project, &[]));
    let (out, err) = (stdout(&output), stderr(&output));
    assert!(out.contains("Composed from 1 unit.\n"), "{out}");
    assert!(out.contains("  @acme/db (^1): not composed yet"), "{out}");
    assert!(
        err.contains("warning:") && err.contains("@acme/db"),
        "{err}"
    );
    assert!(err.contains("hint: register the unit"), "{err}");
    let json = status_json(&fixture, &project);
    assert_eq!(json["up_to_date"], Value::Null);
    assert_eq!(json["files"], Value::Null);
    assert_eq!(json["units"][1]["required_by"], Value::Null);
    assert_eq!(json["units"][1]["newest"], Value::Null);
    assert!(json["problems"][0].as_str().unwrap().contains("@acme/db"));

    // Without a declaration to read, no unit is said to be retired.
    fs::write(project.join("assemblr.jsonc"), "{").unwrap();
    let output = succeeds(status(&fixture, &project, &[]));
    let out = stdout(&output);
    assert!(out.contains("  @acme/node 1.0.0\n"), "{out}");
    assert!(!out.contains("no longer selected"), "{out}");
    assert!(stderr(&output).contains("warning:"), "{}", stderr(&output));
}

/// A stored base that does not verify makes `compose` refuse, so status
/// does not say it has nothing to do.
#[test]
fn status_does_not_call_a_project_with_a_missing_base_up_to_date() {
    let fixture = Fixture::new("status-base");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")]);
    succeeds(fixture.compose(&project));
    fs::remove_dir_all(project.join(".assemblr/objects")).unwrap();

    let output = succeeds(status(&fixture, &project, &[]));
    let out = stdout(&output);
    assert!(out.contains("Composed from 1 unit.\n"), "{out}");
    assert!(!out.contains("nothing to do"), "{out}");
    assert!(
        stderr(&output).contains("restore .assemblr/ from version control"),
        "{}",
        stderr(&output)
    );
    assert_eq!(status_json(&fixture, &project)["up_to_date"], Value::Null);
    fails(fixture.compose(&project));
}

#[test]
fn the_agents_md_init_wrote_gives_way_to_a_unit_s_and_an_edited_one_does_not() {
    let fixture = Fixture::new("agents-scaffold");
    let guide = fixture.repo("guide");
    guide.write("manifest.jsonc", &manifest("@acme/guide", "1.0.0", &[], ""));
    guide.write("src/AGENTS.md", "# The stack's guidance\n");
    guide.publish("1.0.0");
    fixture.register(&guide);

    // Untouched since `init`: the unit's file replaces it, with a note.
    let project = fixture.project("untouched", &[("@acme/guide", "^1")]);
    let output = succeeds(fixture.compose(&project));
    assert!(
        stderr(&output).contains(
            "AGENTS.md was still the one `assemblr init` wrote, so the composition's replaced it"
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(read(&project.join("AGENTS.md")), "# The stack's guidance\n");
    // It is a composed file now: removing the unit removes it.
    declare(&project, &[]);
    succeeds(fixture.compose(&project));
    assert!(!project.join("AGENTS.md").exists());

    // With CRLF line endings, as a checkout may convert it, the same.
    let project = fixture.project("crlf", &[("@acme/guide", "^1")]);
    let agents = read(&project.join("AGENTS.md")).replace('\n', "\r\n");
    fs::write(project.join("AGENTS.md"), agents).unwrap();
    succeeds(fixture.compose(&project));
    assert_eq!(read(&project.join("AGENTS.md")), "# The stack's guidance\n");

    // Edited by the consumer: a conflict, as for any file of their own.
    let project = fixture.project("edited", &[("@acme/guide", "^1")]);
    let edited = format!("{}\nOur own rule.\n", read(&project.join("AGENTS.md")));
    fs::write(project.join("AGENTS.md"), &edited).unwrap();
    fails(fixture.compose(&project));
    let agents = read(&project.join("AGENTS.md"));
    assert!(agents.contains("<<<<<<< current"), "{agents}");
    assert!(agents.contains("Our own rule."), "{agents}");
}
