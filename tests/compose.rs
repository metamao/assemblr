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

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

struct Fixture {
    dir: PathBuf,
    data: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new(test: &str) -> Self {
        // Outside the repository: a project's ignore rules come from the
        // repository that encloses it, and this crate's own `target/` is
        // ignored, which would make every composed file look ignored.
        let dir = std::env::temp_dir()
            .join("assemblr-compose-tests")
            .join(test);
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        let fixture = Self {
            data: dir.join("data"),
            home: dir.join("home"),
            dir,
        };
        fs::create_dir_all(&fixture.data).unwrap();
        fs::create_dir_all(&fixture.home).unwrap();
        fixture
    }

    fn repo(&self, name: &str) -> Repo {
        git(&self.dir, &["init", "-q", name]);
        Repo {
            path: self.dir.join(name),
        }
    }

    fn assemblr(&self, dir: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_assemblr"))
            .current_dir(dir)
            .env("ASSEMBLR_DATA_DIR", &self.data)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
            .args(args)
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

    /// A project directory with `assemblr.jsonc` declaring `dependencies`
    /// and `options`, created by `assemblr init`.
    fn project(&self, name: &str, dependencies: &[(&str, &str)], options: &str) -> PathBuf {
        succeeds(self.assemblr(&self.dir, &["init", name]));
        let project = self.dir.join(name);
        declare(&project, dependencies, options);
        project
    }
}

fn declare(project: &Path, dependencies: &[(&str, &str)], options: &str) {
    let dependencies: Vec<String> = dependencies
        .iter()
        .map(|(identity, range)| format!("    \"{identity}\": \"{range}\""))
        .collect();
    fs::write(
        project.join("assemblr.jsonc"),
        format!(
            "{{\n  \"dependencies\": {{\n{}\n  }},\n  \"options\": {{{options}}}\n}}\n",
            dependencies.join(",\n")
        ),
    )
    .unwrap();
}

fn git(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    let output = command
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
        .env("GIT_AUTHOR_NAME", "Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_NAME", "Author")
        .env("GIT_COMMITTER_EMAIL", "author@example.com")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .args(["-c", "core.autocrlf=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
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

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn succeeds(output: Output) -> Output {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        stdout(&output),
        stderr(&output)
    );
    output
}

fn fails(output: Output) -> Output {
    assert_eq!(
        output.status.code(),
        Some(1),
        "stdout: {}\nstderr: {}",
        stdout(&output),
        stderr(&output)
    );
    output
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

/// A registered `@acme/node` 1.0.0 with a template and a plain file and an
/// optional parameter, and `@acme/web` 1.0.0 depending on it, amending the
/// template.
fn registry(fixture: &Fixture) -> (Repo, Repo) {
    let node = fixture.repo("node");
    node.write(
        "manifest.jsonc",
        &manifest(
            "@acme/node",
            "1.0.0",
            &[],
            r#", "params": {"port": {"type": "integer", "description": "The port.", "default": 3000}}"#,
        ),
    );
    node.write(
        "src/config/app.json.assemblr.tmpl",
        "{\n  \"port\": {{ port }}\n}\n",
    );
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
            r#", "options": {"@acme/node": {"port": 8080}}, "amendments": {"config/app.json.assemblr.tmpl": {"unit": "@acme/node", "base": "1.0.0"}}"#,
        ),
    );
    web.write(
        "amendments/config/app.json.assemblr.tmpl",
        "{\n  \"port\": {{ port }},\n  \"web\": true\n}\n",
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

#[test]
fn init_creates_the_declaration_and_nothing_else() {
    let fixture = Fixture::new("init");
    let output = succeeds(fixture.assemblr(&fixture.dir, &["init", "apps/shop"]));
    let project = fixture.dir.join("apps").join("shop");
    assert!(
        stdout(&output).contains("created the directory and assemblr.jsonc"),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        read(&project.join("assemblr.jsonc")),
        "{\n  \"dependencies\": {},\n  \"options\": {}\n}\n"
    );
    assert_eq!(tree(&project).len(), 1);
    assert!(!project.join(".git").exists());

    // Again: nothing changes, and an edited declaration is never overwritten.
    fs::write(
        project.join("assemblr.jsonc"),
        "{ \"dependencies\": { \"@acme/web\": \"^1\" } }",
    )
    .unwrap();
    let output = succeeds(fixture.assemblr(&project, &["init"]));
    assert!(
        stdout(&output).contains("already a consumer project"),
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
    let project = fixture.project("shop", &[("@acme/web", "^1")], "");

    let output = succeeds(fixture.compose(&project));
    assert!(
        stdout(&output)
            .contains("Composed 2 units into 4 file(s); the reconciliation base sha256-v1:"),
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
    assert!(listing.contains("100755 "), "{listing}");
    let blobs: Vec<_> = tree(&state.join("objects"));
    assert_eq!(blobs.len(), 4);
    assert!(blobs.iter().any(|(_, bytes)| bytes == b"node\n"));
    assert!(!state.join("journal.json").exists());

    // A second compose changes nothing.
    let before = tree(&project);
    let output = succeeds(fixture.compose(&project));
    assert!(
        stdout(&output).contains("Nothing to compose: the project already holds"),
        "{}",
        stdout(&output)
    );
    assert_eq!(tree(&project), before);
}

#[test]
fn the_consumer_s_options_render_its_selections_and_the_result_is_order_independent() {
    let fixture = Fixture::new("options");
    registry(&fixture);
    // A direct selection of node with the consumer's own value.
    let project = fixture.project(
        "one",
        &[("@acme/node", "^1")],
        r#""@acme/node": {"port": 9000}"#,
    );
    succeeds(fixture.compose(&project));
    assert_eq!(
        read(&project.join("config/app.json")),
        "{\n  \"port\": 9000\n}\n"
    );
    let first = tree(&project);

    // The same declaration in another order composes the same bytes, lock
    // and base included.
    let other = fixture.dir.join("two");
    succeeds(fixture.assemblr(&fixture.dir, &["init", "two"]));
    fs::write(
        other.join("assemblr.jsonc"),
        "{\n  \"options\": {\"@acme/node\": {\"port\": 9000}},\n  \"dependencies\": {\n    \"@acme/node\": \"^1\"\n  }\n}\n",
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

    // A value for a unit the project does not select is refused, with the
    // build's wording, and nothing is written.
    let bad = fixture.project(
        "bad",
        &[("@acme/web", "^1")],
        r#""@acme/node": {"port": 1}"#,
    );
    let err = stderr(&fails(fixture.compose(&bad)));
    assert!(err.contains("@acme/node"), "{err}");
    assert_eq!(tree(&bad).len(), 1);
}

#[test]
fn a_rejected_composition_leaves_the_project_unmodified() {
    let fixture = Fixture::new("rejected");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/missing", "^1")], "");
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("no registered source provides @acme/missing"),
        "{err}"
    );
    assert!(err.contains("register the unit's repository"), "{err}");
    assert_eq!(tree(&project).len(), 1);
    assert!(!project.join(".assemblr").exists());

    // Existing content at a delivered path, or on the way to one.
    declare(&project, &[("@acme/web", "^1")], "");
    fs::write(project.join("README.md"), "mine").unwrap();
    fs::write(project.join("config"), "a file where a directory goes").unwrap();
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("already hold content Assemblr did not materialize:\nerror:   README.md\nerror:   config"),
        "{err}"
    );
    assert!(
        err.contains("composing over existing content is not supported yet"),
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
    let project = fixture.project("shop", &[("@acme/bad", "^1")], "");
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("path(s) Assemblr reserves for its own files:\nerror:   .assemblr/x\nerror:   assemblr.jsonc"),
        "{err}"
    );
    assert_eq!(tree(&project).len(), 1);
}

#[test]
fn a_changed_declaration_is_refused_until_reconciliation_exists_and_a_bad_base_is_detected() {
    let fixture = Fixture::new("changed");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")], "");
    succeeds(fixture.compose(&project));
    let before = tree(&project);

    declare(&project, &[("@acme/web", "^1")], "");
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("reconciling a project that was composed before is not supported yet"),
        "{err}"
    );
    assert!(err.contains("(1 path(s) added or removed)"), "{err}");
    let mut after = tree(&project);
    after.retain(|(path, _)| path != "assemblr.jsonc");
    let mut expected = before.clone();
    expected.retain(|(path, _)| path != "assemblr.jsonc");
    assert_eq!(after, expected);

    // A tampered blob, a tampered listing, and a missing store each fail
    // verification by name, before anything else.
    declare(&project, &[("@acme/node", "^1")], "");
    let objects = project.join(".assemblr").join("objects");
    let (blob, original) = tree(&objects).into_iter().next().unwrap();
    fs::write(objects.join(&blob), b"tampered").unwrap();
    let err = stderr(&fails(fixture.compose(&project)));
    assert!(
        err.contains("does not verify: its bytes are not the content of"),
        "{err}"
    );
    assert!(
        err.contains("restore .assemblr/ from version control"),
        "{err}"
    );
    fs::write(objects.join(&blob), original).unwrap();
    succeeds(fixture.compose(&project));
    let bases = project.join(".assemblr").join("bases");
    let (listing, text) = tree(&bases).into_iter().next().unwrap();
    fs::write(bases.join(&listing), [text.clone(), b"x".to_vec()].concat()).unwrap();
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
    assert!(err.contains("restore it from version control"), "{err}");
}

#[test]
fn an_interrupted_composition_is_reverted_by_the_next_one() {
    let fixture = Fixture::new("interrupted");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")], "");
    succeeds(fixture.compose(&project));
    let complete = tree(&project);

    // The state an interruption after some files and before the lock
    // leaves: the journal, the store, and part of the files.
    fs::remove_file(project.join("assemblr-lock.json")).unwrap();
    fs::remove_file(project.join("README.md")).unwrap();
    fs::write(project.join("bin/run"), "edited meanwhile").unwrap();
    let blob = |bytes: &[u8]| assemblr::domain::BlobId::of(bytes).to_string();
    let journal = format!(
        "{{\n  \"format\": 1,\n  \"files\": [\n    {{\"path\": \"README.md\", \"blob\": \"{}\"}},\n    \
         {{\"path\": \"bin/run\", \"blob\": \"{}\"}},\n    {{\"path\": \"config/app.json\", \"blob\": \"{}\"}}\n  ],\n  \
         \"directories\": [\"bin\", \"config\"]\n}}\n",
        blob(b"node\n"),
        blob(b"#!/bin/sh\necho run\n"),
        blob(b"{\n  \"port\": 3000\n}\n")
    );
    fs::write(project.join(".assemblr").join("journal.json"), journal).unwrap();

    // The next compose reverts first: the files still holding what was
    // written go, the edited one stays, the journal goes. The edited file
    // then blocks the fresh composition, as any existing content does.
    let output = fails(fixture.compose(&project));
    assert!(
        stderr(&output).contains(
            "note: reverted a composition that was interrupted earlier: removed 1 file(s) it had \
             written, kept 1 whose content changed since (bin/run)"
        ),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output)
            .contains("already hold content Assemblr did not materialize:\nerror:   bin/run"),
        "{}",
        stderr(&output)
    );
    assert!(!project.join("config/app.json").exists());
    assert!(!project.join(".assemblr").join("journal.json").exists());
    assert_eq!(read(&project.join("bin/run")), "edited meanwhile");

    // With the edit out of the way, the project composes again to the
    // same state as before the interruption.
    fs::remove_file(project.join("bin/run")).unwrap();
    fs::remove_dir(project.join("bin")).unwrap();
    succeeds(fixture.compose(&project));
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
    let project = fixture.project("shop", &[("@acme/env", "^1")], "");
    git(&project, &["init", "-q"]);
    fs::write(project.join(".gitignore"), ".env\n").unwrap();

    let output = succeeds(fixture.compose(&project));
    assert!(
        stderr(&output).contains(
            "warning: .env is ignored by the project's version control, so its reconciliation base \
             is not stored"
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

    // A project that ignores `.assemblr/` is warned that the base will not
    // travel; a project outside any repository ignores nothing.
    let other = fixture.project("other", &[("@acme/env", "^1")], "");
    git(&other, &["init", "-q"]);
    fs::write(other.join(".gitignore"), ".assemblr/\n").unwrap();
    let output = succeeds(fixture.compose(&other));
    assert!(
        stderr(&output).contains(".assemblr/ is ignored by the project's version control"),
        "{}",
        stderr(&output)
    );
    let plain = fixture.project("plain", &[("@acme/env", "^1")], "");
    let output = succeeds(fixture.compose(&plain));
    assert!(!stderr(&output).contains("warning"), "{}", stderr(&output));
    assert_eq!(tree(&plain.join(".assemblr").join("objects")).len(), 2);
}

#[test]
fn a_checkout_with_line_ending_conversion_leaves_the_stored_base_intact() {
    let fixture = Fixture::new("checkout");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/web", "^1")], "");
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
