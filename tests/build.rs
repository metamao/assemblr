//! `assemblr unit build`, run as a real process against unit repositories
//! and a registry built in temporary directories.
//!
//! Every test gives the binary its own registry through `ASSEMBLR_DATA_DIR`
//! and its own home directory, so the developer's registry and Git
//! configuration are never read or changed.

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

/// A test's directories: unit repositories, the registry, and a home.
struct Fixture {
    dir: PathBuf,
    data: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new(test: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("build")
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

    /// An empty unit repository named `name`.
    fn repo(&self, name: &str) -> Repo {
        git(&self.dir, &["init", "-q", name]);
        Repo {
            path: self.dir.join(name),
        }
    }

    /// Runs `assemblr` in `dir`.
    fn assemblr(&self, dir: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_assemblr"))
            .current_dir(dir)
            .env("ASSEMBLR_DATA_DIR", &self.data)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .args(args)
            .output()
            .unwrap()
    }

    fn register(&self, repo: &Repo) {
        let path = repo.path.to_str().unwrap();
        succeeds(self.assemblr(&self.dir, &["registry", "add", path]));
    }

    fn build(&self, repo: &Repo) -> Output {
        self.assemblr(&repo.path, &["unit", "build"])
    }
}

/// `git` in `dir`, isolated from the developer's configuration, with fixed
/// identities and dates so that object ids are reproducible.
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

/// A unit repository with a working tree.
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

    fn write_bytes(&self, path: &str, contents: &[u8]) {
        let path = self.path.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn read_bytes(&self, path: &str) -> Vec<u8> {
        fs::read(self.path.join(path)).unwrap()
    }

    fn remove(&self, path: &str) {
        fs::remove_file(self.path.join(path)).unwrap();
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

    fn rev(&self, name: &str) -> String {
        self.git(&["rev-parse", name])
    }

    /// The files under `build/`, by path, with their contents.
    fn build_files(&self) -> Vec<(String, String)> {
        files(&self.path.join("build"))
    }

    fn lockfile(&self) -> Value {
        serde_json::from_str(&self.read("assemblr-lock.json")).unwrap()
    }
}

/// The files under `dir`, by `/`-separated path, with their contents.
fn files(dir: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path.strip_prefix(dir).unwrap();
                found.push((
                    relative.to_string_lossy().replace('\\', "/"),
                    fs::read_to_string(&path).unwrap(),
                ));
            }
        }
    }
    found.sort();
    found
}

fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(path, contents)| ((*path).to_owned(), (*contents).to_owned()))
        .collect()
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

/// A registered `@acme/node` with versions 1.0.0 and 1.1.0, and an
/// unpublished `@acme/app` that depends on it with `range`.
fn app_on_node(fixture: &Fixture, range: &str) -> (Repo, Repo) {
    let node = fixture.repo("node");
    node.write("manifest.jsonc", &manifest("@acme/node", "1.0.0", &[], ""));
    node.write("src/node.txt", "node 1.0.0");
    node.publish("1.0.0");
    node.write("manifest.jsonc", &manifest("@acme/node", "1.1.0", &[], ""));
    node.write("src/node.txt", "node 1.1.0");
    node.publish("1.1.0");
    fixture.register(&node);
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.1.0", &[("@acme/node", range)], ""),
    );
    app.write("src/app.txt", "app");
    (node, app)
}

#[test]
fn a_unit_is_built_with_its_published_dependencies() {
    let fixture = Fixture::new("builds");
    let (node, app) = app_on_node(&fixture, "^1");
    let output = succeeds(fixture.build(&app));
    assert_eq!(
        stdout(&output).trim(),
        "Built @acme/app 0.1.0 into build/: 2 file(s), with 1 dependency."
    );
    assert!(
        stderr(&output).contains("note: created assemblr-lock.json: @acme/node 1.1.0 (new)"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        app.build_files(),
        pairs(&[("app.txt", "app"), ("node.txt", "node 1.1.0")])
    );
    let lock = app.lockfile();
    assert_eq!(lock["format"], 1);
    assert_eq!(lock["units"]["@acme/node"]["version"], "1.1.0");
    assert_eq!(lock["units"]["@acme/node"]["commit"], node.rev("v1.1.0"));
    assert_eq!(
        lock["units"]["@acme/node"]["tree"],
        node.rev("v1.1.0^{tree}")
    );
    // The lockfile is the only file a build writes beside `build/`.
    let mut top: Vec<String> = fs::read_dir(&app.path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    top.sort();
    assert_eq!(
        top,
        [
            ".git",
            "assemblr-lock.json",
            "build",
            "manifest.jsonc",
            "src"
        ]
    );
}

#[test]
fn dependencies_come_from_published_versions_never_working_trees() {
    let fixture = Fixture::new("published-only");
    let (node, app) = app_on_node(&fixture, "^1");
    node.write("src/node.txt", "uncommitted");
    node.write("src/extra.txt", "uncommitted");
    succeeds(fixture.build(&app));
    assert_eq!(
        app.build_files(),
        pairs(&[("app.txt", "app"), ("node.txt", "node 1.1.0")])
    );
}

#[test]
fn a_newer_published_version_does_not_change_the_lock() {
    let fixture = Fixture::new("lock-kept");
    let (node, app) = app_on_node(&fixture, "^1");
    succeeds(fixture.build(&app));
    let lockfile = app.read("assemblr-lock.json");
    node.write("manifest.jsonc", &manifest("@acme/node", "1.2.0", &[], ""));
    node.write("src/node.txt", "node 1.2.0");
    node.publish("1.2.0");
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "sync"]));

    let output = succeeds(fixture.build(&app));
    assert!(
        !stderr(&output).contains("assemblr-lock.json"),
        "{}",
        stderr(&output)
    );
    assert_eq!(app.read("assemblr-lock.json"), lockfile);
    assert_eq!(
        app.build_files(),
        pairs(&[("app.txt", "app"), ("node.txt", "node 1.1.0")])
    );
}

#[test]
fn manifest_changes_update_the_lock_only_as_resolution_requires() {
    let fixture = Fixture::new("lock-updated");
    let (_node, app) = app_on_node(&fixture, "~1.0");
    succeeds(fixture.build(&app));
    assert_eq!(app.lockfile()["units"]["@acme/node"]["version"], "1.0.0");

    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.1.0", &[("@acme/node", "^1.1")], ""),
    );
    let output = succeeds(fixture.build(&app));
    assert!(
        stderr(&output).contains("note: updated assemblr-lock.json: @acme/node 1.0.0 -> 1.1.0"),
        "{}",
        stderr(&output)
    );
    assert_eq!(app.lockfile()["units"]["@acme/node"]["version"], "1.1.0");

    app.write("manifest.jsonc", &manifest("@acme/app", "0.1.0", &[], ""));
    let output = succeeds(fixture.build(&app));
    assert!(
        stderr(&output).contains("@acme/node 1.1.0 (no longer required)"),
        "{}",
        stderr(&output)
    );
    assert_eq!(app.lockfile()["units"], serde_json::json!({}));
    assert_eq!(app.build_files(), pairs(&[("app.txt", "app")]));
}

#[test]
fn src_is_what_committing_everything_would_publish_with_working_tree_contents() {
    let fixture = Fixture::new("selection");
    let app = fixture.repo("app");
    app.write("manifest.jsonc", &manifest("@acme/app", "0.1.0", &[], ""));
    app.write(".gitignore", "*.log\nignored/\n");
    app.write("src/modified.txt", "committed");
    app.write("src/deleted.txt", "committed");
    app.write("src/staged.txt", "committed");
    app.write("src/untracked-again.txt", "committed");
    app.write("src/tracked.log", "committed");
    app.git(&["add", "-A"]);
    app.git(&["add", "-f", "src/tracked.log"]);
    app.git(&["commit", "-q", "-m", "first"]);

    app.write("src/modified.txt", "working tree");
    app.remove("src/deleted.txt");
    app.write("src/staged.txt", "staged");
    app.write("src/staged-new.txt", "staged");
    app.git(&["add", "src/staged.txt", "src/staged-new.txt"]);
    app.write("src/staged.txt", "working tree");
    app.write("src/staged-new.txt", "working tree");
    app.git(&["rm", "-q", "--cached", "src/untracked-again.txt"]);
    app.write("src/tracked.log", "working tree");
    app.write("src/untracked.log", "ignored");
    app.write("src/ignored/file.txt", "ignored");
    app.write("src/new.txt", "new");
    // Ignore rules outside the working tree's `.gitignore` files never
    // apply: not the clone's own, nor the repository's or the user's
    // configuration.
    app.write(".git/info/exclude", "src/excluded-locally.txt\n");
    app.write("src/excluded-locally.txt", "not ignored");
    app.write("repo-excludes", "src/excluded-by-repository.txt\n");
    app.git(&["config", "core.excludesFile", "repo-excludes"]);
    app.write("src/excluded-by-repository.txt", "not ignored");
    fs::write(fixture.home.join("excludes"), "src/excluded-by-user.txt\n").unwrap();
    fs::write(
        fixture.home.join(".gitconfig"),
        format!(
            "[core]\n\texcludesFile = {}\n",
            fixture
                .home
                .join("excludes")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .unwrap();
    app.write("src/excluded-by-user.txt", "not ignored");
    // A nested `.gitignore` applies, and is itself a file of `src/`.
    app.write("src/nested/.gitignore", "local.txt\n");
    app.write("src/nested/local.txt", "ignored");

    succeeds(fixture.build(&app));
    assert_eq!(
        app.build_files(),
        pairs(&[
            ("excluded-by-repository.txt", "not ignored"),
            ("excluded-by-user.txt", "not ignored"),
            ("excluded-locally.txt", "not ignored"),
            ("modified.txt", "working tree"),
            ("nested/.gitignore", "local.txt\n"),
            ("new.txt", "new"),
            ("staged-new.txt", "working tree"),
            ("staged.txt", "working tree"),
            ("tracked.log", "working tree"),
            ("untracked-again.txt", "committed"),
        ])
    );
}

#[test]
fn a_rejected_build_reports_every_problem_and_changes_nothing() {
    let fixture = Fixture::new("rejected");
    let (_node, app) = app_on_node(&fixture, "^1");
    succeeds(fixture.build(&app));
    let lockfile = app.read("assemblr-lock.json");
    let build = app.build_files();

    app.write("src/node.txt", "collides with the dependency's file");
    app.write("amendments/node/node.txt", "an amendment");
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[("@acme/node", "^1")],
            r#", "params": {"appName": {"type": "string", "description": "The name."}}"#,
        ),
    );
    let output = fails(fixture.build(&app));
    let stderr = stderr(&output);
    for expected in [
        "`amendments/` has content, and amendments cannot be built yet: amendments/node/node.txt",
        "@acme/app declares required parameters without defaults: `appName`",
        "supplying development values is not supported yet",
        "path collision: node.txt from @acme/app, node.txt from @acme/node",
        "hint: the build was rejected; build/ and assemblr-lock.json were not changed",
    ] {
        assert!(stderr.contains(expected), "{expected:?} in {stderr}");
    }
    // The missing value is reported once, as a development value.
    assert!(!stderr.contains("has no value and no default"), "{stderr}");
    assert_eq!(app.read("assemblr-lock.json"), lockfile);
    assert_eq!(app.build_files(), build);
}

#[test]
fn dependency_parameters_set_through_options_build() {
    let fixture = Fixture::new("options");
    let node = fixture.repo("node");
    node.write(
        "manifest.jsonc",
        &manifest(
            "@acme/node",
            "1.0.0",
            &[],
            r#", "params": {"port": {"type": "integer", "description": "The port."}}"#,
        ),
    );
    node.publish("1.0.0");
    fixture.register(&node);
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[("@acme/node", "^1")],
            r#", "options": {"@acme/node": {"port": 8080}},
                "params": {"title": {"type": "string", "description": "A title.", "default": "x"}}"#,
        ),
    );
    succeeds(fixture.build(&app));

    // Without the option, the dependency's parameter has no value.
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.1.0", &[("@acme/node", "^1")], ""),
    );
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(
        stderr.contains("required parameter `port` of @acme/node 1.0.0 has no value"),
        "{stderr}"
    );
    assert!(!stderr.contains("development values"), "{stderr}");
}

#[test]
fn an_empty_amendments_directory_or_ignored_files_in_it_do_not_block_a_build() {
    let fixture = Fixture::new("empty-amendments");
    let app = fixture.repo("app");
    app.write("manifest.jsonc", &manifest("@acme/app", "0.1.0", &[], ""));
    app.write("src/app.txt", "app");
    fs::create_dir_all(app.path.join("amendments/empty")).unwrap();
    app.write(".gitignore", "*.swp\n");
    app.write("amendments/package.json.swp", "editor state");
    succeeds(fixture.build(&app));

    app.write("amendments/package.json", "{}");
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(stderr.contains("amendments/package.json"), "{stderr}");
    assert!(!stderr.contains(".swp"), "{stderr}");
}

#[test]
fn stale_files_from_an_earlier_build_are_gone() {
    let fixture = Fixture::new("stale");
    let app = fixture.repo("app");
    app.write("manifest.jsonc", &manifest("@acme/app", "0.1.0", &[], ""));
    app.write("src/old.txt", "old");
    succeeds(fixture.build(&app));
    app.remove("src/old.txt");
    app.write("src/new.txt", "new");
    app.write("build/foreign.txt", "written by hand");
    succeeds(fixture.build(&app));
    assert_eq!(app.build_files(), pairs(&[("new.txt", "new")]));
}

#[test]
fn a_build_that_is_a_file_is_left_alone() {
    let fixture = Fixture::new("build-file");
    let app = fixture.repo("app");
    app.write("manifest.jsonc", &manifest("@acme/app", "0.1.0", &[], ""));
    app.write("build", "not a directory");
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(stderr.contains("is a regular file"), "{stderr}");
    assert_eq!(app.read("build"), "not a directory");
    assert!(!app.path.join("assemblr-lock.json").exists());
}

#[cfg(unix)]
#[test]
fn a_build_that_is_a_symbolic_link_is_left_alone() {
    let fixture = Fixture::new("build-symlink");
    let app = fixture.repo("app");
    app.write("manifest.jsonc", &manifest("@acme/app", "0.1.0", &[], ""));
    app.write("src/app.txt", "app");
    let elsewhere = fixture.dir.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("keep.txt"), "keep").unwrap();
    std::os::unix::fs::symlink(&elsewhere, app.path.join("build")).unwrap();
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(stderr.contains("is a symbolic link"), "{stderr}");
    assert!(
        app.path
            .join("build")
            .symlink_metadata()
            .unwrap()
            .is_symlink()
    );
    assert_eq!(files(&elsewhere), pairs(&[("keep.txt", "keep")]));
}

#[cfg(unix)]
#[test]
fn symbolic_links_in_src_are_rejected() {
    let fixture = Fixture::new("src-symlink");
    let app = fixture.repo("app");
    app.write("manifest.jsonc", &manifest("@acme/app", "0.1.0", &[], ""));
    app.write("outside/secret.txt", "secret");
    fs::create_dir_all(app.path.join("src")).unwrap();
    std::os::unix::fs::symlink("../outside/secret.txt", app.path.join("src/link.txt")).unwrap();
    std::os::unix::fs::symlink("../outside", app.path.join("src/dir")).unwrap();
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(
        stderr.contains("src/link.txt: symbolic links are not supported"),
        "{stderr}"
    );
    assert!(
        stderr.contains("src/dir: symbolic links are not supported"),
        "{stderr}"
    );
    assert!(!app.path.join("build").exists());
}

#[cfg(unix)]
#[test]
fn executable_bits_come_from_the_working_tree() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("executable");
    let app = fixture.repo("app");
    app.write("manifest.jsonc", &manifest("@acme/app", "0.1.0", &[], ""));
    app.write("src/run.sh", "#!/bin/sh\n");
    app.write("src/README", "text");
    let executable = |path: &Path| {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    };
    executable(&app.path.join("src/run.sh"));
    succeeds(fixture.build(&app));
    let mode = |name: &str| {
        fs::metadata(app.path.join("build").join(name))
            .unwrap()
            .permissions()
            .mode()
    };
    assert_ne!(mode("run.sh") & 0o100, 0);
    assert_eq!(mode("README") & 0o111, 0);
}

#[test]
fn a_tracked_symbolic_link_is_rejected_even_when_checked_out_as_a_file() {
    // On Windows without symbolic link support, Git checks a link out as a
    // plain file. The index still records it as a link.
    let fixture = Fixture::new("index-symlink");
    let app = fixture.repo("app");
    app.write("manifest.jsonc", &manifest("@acme/app", "0.1.0", &[], ""));
    app.write("src/link", "target.txt");
    let blob = app.git(&["hash-object", "-w", "src/link"]);
    app.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("120000,{blob},src/link"),
    ]);
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(
        stderr.contains("src/link: symbolic links are not supported"),
        "{stderr}"
    );
}

#[test]
fn paths_that_differ_only_in_unicode_normalization_collide() {
    let fixture = Fixture::new("normalization");
    let node = fixture.repo("node");
    node.write("manifest.jsonc", &manifest("@acme/node", "1.0.0", &[], ""));
    node.write("src/caf\u{e9}.md", "precomposed");
    node.publish("1.0.0");
    fixture.register(&node);
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.1.0", &[("@acme/node", "^1")], ""),
    );
    app.write("src/cafe\u{301}.md", "decomposed");
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(stderr.contains("path collision"), "{stderr}");
    assert!(stderr.contains("Unicode normalization"), "{stderr}");
}

#[test]
fn a_locked_version_that_is_not_available_asks_for_synchronization() {
    let fixture = Fixture::new("unavailable");
    let (node, app) = app_on_node(&fixture, "^1");
    let commit = node.rev("v1.1.0");
    let tree = node.rev("v1.1.0^{tree}");
    app.write(
        "assemblr-lock.json",
        &format!(
            r#"{{"format": 1, "units": {{"@acme/node": {{"version": "1.5.0", "commit": "{commit}", "tree": "{tree}"}}}}}}"#
        ),
    );
    let lockfile = app.read("assemblr-lock.json");
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(
        stderr.contains("@acme/node is locked to 1.5.0, which is not available locally"),
        "{stderr}"
    );
    assert!(stderr.contains("assemblr registry sync"), "{stderr}");
    assert_eq!(app.read("assemblr-lock.json"), lockfile);
    assert!(!app.path.join("build").exists());
}

#[test]
fn a_locked_version_whose_content_changed_is_rejected() {
    let fixture = Fixture::new("content-changed");
    let (_node, app) = app_on_node(&fixture, "^1");
    let other = "0".repeat(40);
    app.write(
        "assemblr-lock.json",
        &format!(
            r#"{{"format": 1, "units": {{"@acme/node": {{"version": "1.0.0", "commit": "{other}", "tree": "{other}"}}}}}}"#
        ),
    );
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(
        stderr.contains(&format!("@acme/node 1.0.0 is locked to commit {other}")),
        "{stderr}"
    );
    assert!(
        stderr.contains("restore the tag in the unit's repository; deleting assemblr-lock.json is not a repair: it discards every locked version"),
        "{stderr}"
    );
}

#[test]
fn an_invalid_lockfile_is_reported_and_kept() {
    let fixture = Fixture::new("invalid-lockfile");
    let (_node, app) = app_on_node(&fixture, "^1");
    app.write("assemblr-lock.json", "<<<<<<< HEAD\n");
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(stderr.contains("is not a valid lockfile"), "{stderr}");
    assert!(
        stderr.contains("fix assemblr-lock.json; deleting assemblr-lock.json is not a repair"),
        "{stderr}"
    );
    assert_eq!(app.read("assemblr-lock.json"), "<<<<<<< HEAD\n");
}

#[test]
fn what_an_interrupted_build_left_is_cleaned_up() {
    let fixture = Fixture::new("interrupted");
    let (_node, app) = app_on_node(&fixture, "^1");
    // A build interrupted after replacing the lockfile leaves no `build/`,
    // and the record that it started writing.
    app.write(".assemblr-build.new/node.txt", "half written");
    app.write(".assemblr-build.old/app.txt", "previous");
    app.write(".assemblr-lock.json.new", "{");
    app.write(".git/assemblr-build.pending", "");
    succeeds(fixture.build(&app));
    assert_eq!(
        app.build_files(),
        pairs(&[("app.txt", "app"), ("node.txt", "node 1.1.0")])
    );
    for leftover in [
        ".assemblr-build.new",
        ".assemblr-build.old",
        ".assemblr-lock.json.new",
    ] {
        assert!(!app.path.join(leftover).exists(), "{leftover}");
    }
    assert!(!app.path.join(".git/assemblr-build.pending").exists());
}

#[test]
fn content_at_reserved_paths_that_no_build_left_is_kept() {
    let fixture = Fixture::new("unrecorded");
    let (_node, app) = app_on_node(&fixture, "^1");
    app.write(".assemblr-build.old/notes.txt", "the author's");
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(
        stderr.contains("exists, but no interrupted build was recorded"),
        "{stderr}"
    );
    assert_eq!(app.read(".assemblr-build.old/notes.txt"), "the author's");
    assert!(!app.path.join("build").exists());
}

/// Builds the app concurrently, half the builds against a registry whose
/// `@acme/node` has 1.0.0 only, and half against one that also has 1.1.0
/// from a clone with the same history. Which build runs first decides the
/// lock: 1.0.0, which every build can then keep, or 1.1.0, which builds
/// against the first registry cannot find. Whatever the order, `build/`
/// must match the lockfile, and nothing may be left beside it.
#[test]
fn concurrent_builds_of_one_unit_run_one_at_a_time() {
    let fixture = Fixture::new("concurrent");
    let older = fixture.repo("node-older");
    older.write("manifest.jsonc", &manifest("@acme/node", "1.0.0", &[], ""));
    older.write("src/node.txt", "node 1.0.0");
    older.publish("1.0.0");
    git(&fixture.dir, &["clone", "-q", "node-older", "node-newer"]);
    let newer = Repo {
        path: fixture.dir.join("node-newer"),
    };
    newer.write("manifest.jsonc", &manifest("@acme/node", "1.1.0", &[], ""));
    newer.write("src/node.txt", "node 1.1.0");
    newer.publish("1.1.0");
    let registries = [
        fixture.dir.join("data-older"),
        fixture.dir.join("data-newer"),
    ];
    for (registry, repo) in registries.iter().zip(["node-older", "node-newer"]) {
        let output = Command::new(env!("CARGO_BIN_EXE_assemblr"))
            .current_dir(&fixture.dir)
            .env("ASSEMBLR_DATA_DIR", registry)
            .args(["registry", "add", repo])
            .output()
            .unwrap();
        succeeds(output);
    }
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.1.0", &[("@acme/node", "^1")], ""),
    );
    app.write("src/app.txt", "app");

    let mut locked = Vec::new();
    for _ in 0..6 {
        let _ = fs::remove_dir_all(app.path.join("build"));
        let _ = fs::remove_file(app.path.join("assemblr-lock.json"));
        let builds: Vec<_> = (0..8)
            .map(|index| {
                Command::new(env!("CARGO_BIN_EXE_assemblr"))
                    .current_dir(&app.path)
                    .env("ASSEMBLR_DATA_DIR", &registries[index % 2])
                    .env("HOME", &fixture.home)
                    .args(["unit", "build"])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap()
            })
            .collect();
        let outputs: Vec<Output> = builds
            .into_iter()
            .map(|build| build.wait_with_output().unwrap())
            .collect();
        assert!(outputs.iter().any(|output| output.status.success()));
        for output in outputs.iter().filter(|output| !output.status.success()) {
            // The only failure: the lock another build wrote names a version
            // this build's registry does not have.
            let stderr = stderr(output);
            assert!(
                stderr.contains("@acme/node is locked to 1.1.0, which is not available locally")
                    && stderr
                        .lines()
                        .filter(|line| line.starts_with("error:"))
                        .count()
                        == 1,
                "{stderr}"
            );
        }
        let version = app.lockfile()["units"]["@acme/node"]["version"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            app.build_files(),
            pairs(&[("app.txt", "app"), ("node.txt", &format!("node {version}"))])
        );
        for leftover in [
            ".assemblr-build.new",
            ".assemblr-build.old",
            ".assemblr-lock.json.new",
            ".git/assemblr-build.pending",
        ] {
            assert!(!app.path.join(leftover).exists(), "{leftover}");
        }
        locked.push(version);
    }
    eprintln!("locked versions by round: {locked:?}");
}

#[test]
fn a_directory_without_a_manifest_is_not_a_unit() {
    let fixture = Fixture::new("not-a-unit");
    let dir = fixture.dir.join("empty");
    fs::create_dir_all(&dir).unwrap();
    let stderr = stderr(&fails(fixture.assemblr(&dir, &["unit", "build"])));
    assert!(stderr.contains("manifest.jsonc does not exist"), "{stderr}");
    assert!(
        stderr.contains("run `assemblr unit build` in the directory that holds `manifest.jsonc`"),
        "{stderr}"
    );
    assert!(!dir.join("build").exists());
}

#[test]
fn a_unit_outside_its_own_git_repository_is_rejected() {
    let fixture = Fixture::new("no-repository");
    let dir = fixture.dir.join("plain");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("manifest.jsonc"),
        manifest("@acme/app", "0.1.0", &[], ""),
    )
    .unwrap();
    fs::write(dir.join("src/app.txt"), "app").unwrap();
    let stderr = stderr(&fails(fixture.assemblr(&dir, &["unit", "build"])));
    assert!(stderr.contains("is not a Git repository"), "{stderr}");
    assert!(
        stderr.contains("build/ and assemblr-lock.json were not changed"),
        "{stderr}"
    );
    assert!(!dir.join("build").exists());
}

#[test]
fn a_missing_dependency_suggests_registering_it() {
    let fixture = Fixture::new("missing");
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.1.0", &[("@acme/absent", "^1")], ""),
    );
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(
        stderr.contains("no registered source provides @acme/absent"),
        "{stderr}"
    );
    assert!(stderr.contains("assemblr registry add <path>"), "{stderr}");
    assert!(!app.path.join("assemblr-lock.json").exists());
}

#[test]
fn a_removed_dependency_repository_fails_explicitly() {
    let fixture = Fixture::new("source-gone");
    let (node, app) = app_on_node(&fixture, "^1");
    fs::remove_dir_all(&node.path).unwrap();
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(
        stderr.contains("could not read the registered source of @acme/node"),
        "{stderr}"
    );
    assert!(
        stderr.contains("`assemblr registry remove <identity>`"),
        "{stderr}"
    );
}

/// A source that only a version outside every requirement depends on still
/// decides the order in which resolution chooses versions, so a build that
/// cannot read it fails rather than risk choosing differently.
#[test]
fn an_unreadable_source_reachable_only_through_an_unselectable_version_fails() {
    let fixture = Fixture::new("unselectable");
    let extra = fixture.repo("extra");
    extra.write("manifest.jsonc", &manifest("@acme/extra", "1.0.0", &[], ""));
    extra.publish("1.0.0");
    fixture.register(&extra);
    let node = fixture.repo("node");
    node.write("manifest.jsonc", &manifest("@acme/node", "1.0.0", &[], ""));
    node.publish("1.0.0");
    node.write(
        "manifest.jsonc",
        &manifest("@acme/node", "2.0.0", &[("@acme/extra", "^1")], ""),
    );
    node.publish("2.0.0");
    fixture.register(&node);
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.1.0", &[("@acme/node", "^1")], ""),
    );
    succeeds(fixture.build(&app));

    fs::remove_dir_all(&extra.path).unwrap();
    let lockfile = app.read("assemblr-lock.json");
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(
        stderr.contains("could not read the registered source of @acme/extra"),
        "{stderr}"
    );
    assert_eq!(app.read("assemblr-lock.json"), lockfile);
}

#[test]
fn the_build_and_lockfile_do_not_depend_on_registration_or_declaration_order() {
    let fixture = Fixture::new("order");
    let mut units = Vec::new();
    for name in ["alpha", "beta"] {
        let repo = fixture.repo(name);
        repo.write(
            "manifest.jsonc",
            &manifest(&format!("@acme/{name}"), "1.0.0", &[], ""),
        );
        repo.write(&format!("src/{name}.txt"), name);
        repo.publish("1.0.0");
        units.push(repo);
    }
    let app = fixture.repo("app");
    app.write("src/app.txt", "app");
    let mut outcomes = Vec::new();
    for (order, declared) in [
        ([0, 1], [("@acme/alpha", "^1"), ("@acme/beta", "^1")]),
        ([1, 0], [("@acme/beta", "^1"), ("@acme/alpha", "^1")]),
    ] {
        fs::remove_dir_all(&fixture.data).unwrap();
        for index in order {
            fixture.register(&units[index]);
        }
        app.write(
            "manifest.jsonc",
            &manifest("@acme/app", "0.1.0", &declared, ""),
        );
        let _ = fs::remove_file(app.path.join("assemblr-lock.json"));
        succeeds(fixture.build(&app));
        outcomes.push((app.read("assemblr-lock.json"), app.build_files()));
    }
    assert_eq!(outcomes[0], outcomes[1]);
}

// Templates (I7).

/// A registered `@acme/node` 1.0.0 with a required `port`, a defaulted
/// `name`, a template, and a plain file that looks like one.
fn node_with_template(fixture: &Fixture) -> Repo {
    let node = fixture.repo("node");
    node.write(
        "manifest.jsonc",
        &manifest(
            "@acme/node",
            "1.0.0",
            &[],
            r#", "params": {
                "port": {"type": "integer", "description": "The port."},
                "name": {"type": "string", "description": "The name.", "default": "node"}
            }"#,
        ),
    );
    node.write(
        "src/config.json.assemblr.tmpl",
        "{\"port\": {{ port }}, \"name\": \"{{ name }}\", \"tmpl\": \"{{ \"{{\" }} x }}\"}\n",
    );
    node.write("src/plain.txt", "{{ port }} stays as it is\n");
    node.publish("1.0.0");
    fixture.register(&node);
    node
}

#[test]
fn templates_render_with_their_unit_s_values_and_other_files_are_delivered_as_they_are() {
    let fixture = Fixture::new("templates");
    node_with_template(&fixture);
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[("@acme/node", "^1")],
            r#", "options": {"@acme/node": {"port": 8080}},
                "params": {
                    "title": {"type": "string", "description": "A title.", "default": "Demo \"app\""},
                    "motto": {"type": "string", "description": "Lines.", "default": "one\ntwo"},
                    "ssr": {"type": "boolean", "description": "Server-side.", "default": true},
                    "workers": {"type": "integer", "description": "Workers.", "default": -2}
                }"#,
        ),
    );
    app.write(
        "src/index.html.assemblr.tmpl",
        "<h1>{{ title }}</h1>\n<p>{{motto}}</p>\nssr={{ ssr }} workers={{\n  workers }}\r\n",
    );
    // Angular templates and workflows are delivered intact without the suffix.
    app.write("src/app.component.html", "<span>{{ value }}</span>\n");
    app.write(
        "src/.github/workflows/ci.yml",
        "runs-on: ${{ matrix.os }}\n{% raw %}\n",
    );
    app.write("src/notes.tmpl", "{{ title }} is not rendered here\n");
    let output = succeeds(fixture.build(&app));
    assert_eq!(
        stdout(&output).trim(),
        "Built @acme/app 0.1.0 into build/: 6 file(s), with 1 dependency."
    );
    assert_eq!(
        app.build_files(),
        pairs(&[
            (
                ".github/workflows/ci.yml",
                "runs-on: ${{ matrix.os }}\n{% raw %}\n"
            ),
            ("app.component.html", "<span>{{ value }}</span>\n"),
            (
                "config.json",
                "{\"port\": 8080, \"name\": \"node\", \"tmpl\": \"{{ x }}\"}\n"
            ),
            (
                "index.html",
                "<h1>Demo \"app\"</h1>\n<p>one\ntwo</p>\nssr=true workers=-2\r\n"
            ),
            ("notes.tmpl", "{{ title }} is not rendered here\n"),
            ("plain.txt", "{{ port }} stays as it is\n"),
        ])
    );
}

#[test]
fn rendering_problems_reject_the_build_and_change_nothing() {
    let fixture = Fixture::new("template-errors");
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[],
            r#", "params": {"title": {"type": "string", "description": "A title.", "default": "Demo"}}"#,
        ),
    );
    app.write("src/index.html.assemblr.tmpl", "<h1>{{ title }}</h1>\n");
    succeeds(fixture.build(&app));
    let before = (app.build_files(), app.read("assemblr-lock.json"));

    app.write(
        "src/index.html.assemblr.tmpl",
        "<h1>{{ title }}</h1>\n  {{ titel }}\n{% if title %}x{% endif %}\n{{ own.title }}\n{{ title",
    );
    app.write("src/b.txt.assemblr.tmpl", "{{ title | upper }}\n");
    let stderr = stderr(&fails(fixture.build(&app)));
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("error: template "))
        .collect();
    assert_eq!(
        lines,
        [
            "error: template src/b.txt.assemblr.tmpl of @acme/app, line 1, column 1: unsupported \
             template construct \"{{ title | upper }}\"; only `{{ name }}` references and the \
             escape `{{ \"{{\" }}` are supported",
            "error: template src/index.html.assemblr.tmpl of @acme/app, line 2, column 3: \
             undefined reference `titel`; the unit's parameters are `title`",
            "error: template src/index.html.assemblr.tmpl of @acme/app, line 3, column 1: \
             unsupported template construct \"{% if title %}\"; only `{{ name }}` references and \
             the escape `{{ \"{{\" }}` are supported",
            "error: template src/index.html.assemblr.tmpl of @acme/app, line 3, column 16: \
             unsupported template construct \"{% endif %}\"; only `{{ name }}` references and \
             the escape `{{ \"{{\" }}` are supported",
            "error: template src/index.html.assemblr.tmpl of @acme/app, line 4, column 1: \
             undefined reference `own.title`; `own` is defined only when an amendment copy is \
             rendered, and a unit's own files refer to its parameters by name",
            "error: template src/index.html.assemblr.tmpl of @acme/app, line 5, column 1: `{{` \
             is never closed",
        ],
        "{stderr}"
    );
    assert!(
        stderr.contains(
            "hint: the build was rejected; build/ and assemblr-lock.json were not changed"
        ),
        "{stderr}"
    );
    assert_eq!((app.build_files(), app.read("assemblr-lock.json")), before);
}

#[test]
fn a_dependency_s_template_sees_only_the_dependency_s_values() {
    let fixture = Fixture::new("template-context");
    let node = fixture.repo("node");
    node.write(
        "manifest.jsonc",
        &manifest(
            "@acme/node",
            "1.0.0",
            &[],
            r#", "params": {"port": {"type": "integer", "description": "The port.", "default": 80}}"#,
        ),
    );
    node.write(
        "src/server.txt.assemblr.tmpl",
        "{{ port }} for {{ title }}\n",
    );
    node.publish("1.0.0");
    fixture.register(&node);
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[("@acme/node", "^1")],
            r#", "params": {"title": {"type": "string", "description": "A title.", "default": "Demo"}}"#,
        ),
    );
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(
        stderr.contains(
            "template src/server.txt.assemblr.tmpl of @acme/node, line 1, column 16: undefined \
             reference `title`; the unit's parameters are `port`"
        ),
        "{stderr}"
    );
}

#[test]
fn a_template_that_is_not_utf_8_is_rejected_while_other_files_are_opaque_bytes() {
    let fixture = Fixture::new("template-bytes");
    let app = fixture.repo("app");
    app.write("manifest.jsonc", &manifest("@acme/app", "0.1.0", &[], ""));
    let binary = [
        0xff, 0xfe, b'{', b'{', b' ', b'x', b' ', b'}', b'}', 0x00, b'\r', b'\n',
    ];
    app.write_bytes("src/logo.bin", &binary);
    succeeds(fixture.build(&app));
    assert_eq!(app.read_bytes("build/logo.bin"), binary);

    app.write_bytes("src/bad.txt.assemblr.tmpl", &binary);
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(
        stderr.contains(
            "template src/bad.txt.assemblr.tmpl of @acme/app is not UTF-8 text; a template is \
             rendered as text, and a file that is not text must not end in `.assemblr.tmpl`"
        ),
        "{stderr}"
    );
}

#[test]
fn a_template_and_an_ordinary_file_delivered_at_the_same_path_collide() {
    let fixture = Fixture::new("template-collision");
    let node = fixture.repo("node");
    node.write("manifest.jsonc", &manifest("@acme/node", "1.0.0", &[], ""));
    node.write("src/package.json", "{}\n");
    node.publish("1.0.0");
    fixture.register(&node);
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest("@acme/app", "0.1.0", &[("@acme/node", "^1")], ""),
    );
    app.write("src/package.json.assemblr.tmpl", "{}\n");
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(
        stderr.contains(
            "path collision: package.json from @acme/node, package.json.assemblr.tmpl from \
             @acme/app (delivered as package.json); no unit takes precedence"
        ),
        "{stderr}"
    );
    assert!(!app.path.join("build").exists());
}

#[test]
fn own_is_not_a_parameter_name() {
    let fixture = Fixture::new("own-parameter");
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[],
            r#", "params": {"own": {"type": "string", "description": "Reserved.", "default": "x"}}"#,
        ),
    );
    let stderr = stderr(&fails(fixture.build(&app)));
    assert!(
        stderr.contains("invalid parameter name \"own\": `own` is reserved"),
        "{stderr}"
    );
}

#[cfg(unix)]
#[test]
fn a_rendered_template_keeps_its_executable_bit() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("template-executable");
    let app = fixture.repo("app");
    app.write(
        "manifest.jsonc",
        &manifest(
            "@acme/app",
            "0.1.0",
            &[],
            r#", "params": {"shell": {"type": "string", "description": "The shell.", "default": "/bin/sh"}}"#,
        ),
    );
    app.write("src/run.sh.assemblr.tmpl", "#!{{ shell }}\n");
    let source = app.path.join("src/run.sh.assemblr.tmpl");
    let mut permissions = fs::metadata(&source).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&source, permissions).unwrap();
    succeeds(fixture.build(&app));
    let built = app.path.join("build/run.sh");
    assert_eq!(fs::read_to_string(&built).unwrap(), "#!/bin/sh\n");
    assert_ne!(
        fs::metadata(&built).unwrap().permissions().mode() & 0o100,
        0
    );
}
