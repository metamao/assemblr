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
        self.assemblr_with_global_config(dir, Path::new(NULL_DEVICE), args)
    }

    /// Runs `assemblr` with `global` as the user's Git configuration.
    fn assemblr_with_global_config(&self, dir: &Path, global: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_assemblr"))
            .current_dir(dir)
            .env("ASSEMBLR_DATA_DIR", &self.data)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", global)
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

/// Publishes `version` of the registered `node` with `readme` and `run` as
/// its plain files, and indexes it.
fn publish_node(fixture: &Fixture, node: &Repo, version: &str, readme: &str, run: &str) {
    node.write(
        "manifest.jsonc",
        &manifest(
            "@acme/node",
            version,
            &[],
            r#", "params": {"port": {"type": "integer", "description": "The port.", "default": 3000}}"#,
        ),
    );
    node.write("src/README.md", readme);
    node.write("src/bin/run", run);
    node.publish(version);
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));
}

#[test]
fn a_changed_declaration_is_reconciled_three_way_and_a_bad_base_is_detected() {
    let fixture = Fixture::new("changed");
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")], "");
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

    // Selecting `web` instead brings its file, its option for `node`, and
    // its amendment of the template: the consumer's member survives the
    // merge, the edit survives untouched, and the deletion stands.
    declare(&project, &[("@acme/web", "^1")], "");
    let output = succeeds(fixture.compose(&project));
    let out = stdout(&output);
    assert!(
        out.contains(
            "Reconciled the project with 2 units (4 file(s)): created 1, modified 0, merged 1, \
             removed 0; the reconciliation base sha256-v1:"
        ),
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

    // Back to `node` alone: the amendment's member and the option go, the
    // consumer's member stays, and the unedited file is removed.
    declare(&project, &[("@acme/node", "^1")], "");
    let output = succeeds(fixture.compose(&project));
    let out = stdout(&output);
    assert!(
        out.contains("created 0, modified 0, merged 1, removed 1;"),
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
    declare(&project, &[], "");
    let before = tree(&project);
    let output = fails(fixture.compose(&project));
    let err = stderr(&output);
    assert!(
        err.contains(
            "the project and the declared composition disagree at 2 path(s), and resolving \
             conflicts in the project is not supported yet:\nerror:   README.md: the composition \
             no longer delivers it, and the project edited it\nerror:   config/app.json: the \
             composition no longer delivers it, and the project edited it"
        ),
        "{err}"
    );
    assert!(
        err.contains("resolving conflicts in place is a later increment"),
        "{err}"
    );
    assert_eq!(tree(&project), before);
    declare(&project, &[("@acme/node", "^1")], "");

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
    assert!(err.contains("restore it from version control"), "{err}");
}

#[test]
fn conflicts_are_refused_before_anything_is_written() {
    let fixture = Fixture::new("conflicts");
    let (node, _) = registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")], "");
    succeeds(fixture.compose(&project));

    // The consumer changes the port the option will change, edits the
    // text file the new version changes, and deletes the script the new
    // version changes.
    fs::write(project.join("config/app.json"), "{\n  \"port\": 9000\n}\n").unwrap();
    fs::write(project.join("README.md"), "mine\n").unwrap();
    fs::remove_file(project.join("bin/run")).unwrap();
    publish_node(
        &fixture,
        &node,
        "1.1.0",
        "node v2\n",
        "#!/bin/sh\necho run v2\n",
    );
    declare(
        &project,
        &[("@acme/node", "^1.1")],
        "\"@acme/node\": {\"port\": 8080}",
    );
    let before = tree(&project);

    let output = fails(fixture.compose(&project));
    let err = stderr(&output);
    assert!(
        err.contains(
            "disagree at 3 path(s), and resolving conflicts in the project is not supported \
             yet:\nerror:   README.md: the project and the composition both changed it, and its \
             format has no merge\nerror:   bin/run: the project deleted it, and the composition \
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

    // With every file as composed, the new version and the option apply.
    fs::remove_dir(project.join("bin/run")).unwrap();
    fs::write(project.join("bin/run"), "#!/bin/sh\necho run\n").unwrap();
    let output = succeeds(fixture.compose(&project));
    let out = stdout(&output);
    assert!(
        out.contains("created 0, modified 3, merged 0, removed 0;"),
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
    registry(&fixture);
    let project = fixture.project("shop", &[("@acme/node", "^1")], "");
    succeeds(fixture.compose(&project));
    let crlf = |name: &str| {
        let path = project.join(name);
        let text = read(&path).replace('\n', "\r\n");
        fs::write(&path, text).unwrap();
    };
    crlf("README.md");
    crlf("config/app.json");

    // The option changes the template's output: the file is replaced, with
    // its line endings; the other file is unchanged and stays as it is.
    declare(
        &project,
        &[("@acme/node", "^1")],
        "\"@acme/node\": {\"port\": 8080}",
    );
    let output = succeeds(fixture.compose(&project));
    let out = stdout(&output);
    assert!(
        out.contains("created 0, modified 1, merged 0, removed 0;"),
        "{out}"
    );
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
    declare(&project, &[("@acme/web", "^1")], "");
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
    let project = fixture.project("shop", &[("@acme/env", "^1")], "");
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
    declare(&project, &[("@acme/env", "^1.1")], "");
    let output = succeeds(fixture.compose(&project));
    let out = stdout(&output);
    assert!(
        out.contains("created 0, modified 1, merged 0, removed 0;"),
        "{out}"
    );
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
    declare(&project, &[("@acme/env", "^1.2")], "");
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
    declare(&project, &[("@acme/env", "^1.3")], "");
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
    let project = fixture.project("shop", &[("@acme/node", "^1")], "");
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
    // then blocks the fresh composition, as any existing content does.
    let output = fails(fixture.compose(&project));
    assert!(
        stderr(&output).contains(
            "note: reverted a composition that was interrupted earlier: removed 1 file(s) it had \
             created, restored 0 it had changed or removed, kept 1 whose content changed since \
             (bin/run)"
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
    let project = fixture.project("shop", &[("@acme/env", "^1")], "");
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
    let other = fixture.project("other", &[("@acme/env", "^1")], "");
    git(&other, &["init", "-q"]);
    fs::write(other.join(".gitignore"), ".assemblr/\n").unwrap();
    let output = succeeds(fixture.compose(&other));
    assert!(
        stderr(&output).contains(".assemblr/ is ignored by the project's version control"),
        "{}",
        stderr(&output)
    );
    let objects = fixture.project("objects", &[("@acme/env", "^1")], "");
    git(&objects, &["init", "-q"]);
    fs::write(objects.join(".gitignore"), ".assemblr/objects/\n").unwrap();
    let output = succeeds(fixture.compose(&objects));
    assert!(
        stderr(&output).contains(".assemblr/objects/ is ignored by the project's version control"),
        "{}",
        stderr(&output)
    );

    // An edited attributes file is restored when the base is written.
    let edited = fixture.project("edited", &[("@acme/env", "^1")], "");
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
    let project = fixture.project("shop", &[("@acme/secret", "^1")], "");
    git(&project, &["init", "-q"]);
    let before = tree(&project);
    let err = stderr(&fails(compose_with(&fixture, &project, &global)));
    assert!(
        err.contains("only your own Git rules ignore, not the project's .gitignore"),
        "{err}"
    );
    assert!(err.contains("a.secret (pattern \"*.secret\" in "), "{err}");
    assert!(
        err.contains("add the file to the project's .gitignore"),
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
    let kept = fixture.project("kept", &[("@acme/secret", "^1")], "");
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
        let project = fixture.project(name, &[("@acme/env", "^1")], "");
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
    let project = fixture.project("shop", &[("@acme/env", "^1")], "");
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
    declare(&project, &[("@acme/env", "^1.1")], "");
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
    declare(&project, &[("@acme/env", "^1.2")], "");
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
    let project = fixture.project("shop", &[("@acme/cfg", "^1")], "");
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
    declare(&project, &[("@acme/cfg", "^1.1")], "");
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
    assert!(out.contains("Composed 1 unit into 3 file(s)"), "{out}");
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
    let project = fixture.project("shop", &[("@acme/web", "^1")], "");
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
    let err = stderr(&fails(add(&fixture, &project, "web")));
    assert!(err.contains("`web` is not a unit identity"), "{err}");
    let err = stderr(&fails(add(&fixture, &project, "@acme/web@")));
    assert!(err.contains("names no version requirement"), "{err}");
    assert_eq!(tree(&project), before);

    // A composition that the project's own content refuses: the file the
    // unit would create is there already, edited.
    fs::write(project.join("web.txt"), "mine\n").unwrap();
    let before = tree(&project);
    let err = stderr(&fails(add(&fixture, &project, "@acme/web")));
    assert!(err.contains("web.txt"), "{err}");
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
    let transitive = fixture.project("deps", &[("@acme/web", "^1")], "");
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
fn remove_takes_the_options_entry_with_it_and_never_discards_an_edited_file() {
    let fixture = Fixture::new("remove-options");
    registry(&fixture);
    let project = fixture.project(
        "shop",
        &[("@acme/node", "^1")],
        r#""@acme/node": {"port": 9000}"#,
    );
    succeeds(fixture.compose(&project));
    assert_eq!(
        read(&project.join("config/app.json")),
        "{\n  \"port\": 9000\n}\n"
    );

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
        stdout(&output).contains("Removed @acme/node and its options from assemblr.jsonc."),
        "{}",
        stdout(&output)
    );
    let text = declaration(&project);
    assert!(
        !text.contains("@acme/node") && !text.contains("9000"),
        "{text}"
    );
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
    let at_once = fixture.project(
        "c",
        &[("@acme/node", "^1.0.0"), ("@acme/web", "^1.0.0")],
        "",
    );
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
