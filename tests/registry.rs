//! `assemblr registry`, run as a real process against unit repositories
//! built in temporary directories.
//!
//! Every test gives the binary its own registry through `ASSEMBLR_DATA_DIR`,
//! so the developer's registry is never read or changed.

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

/// An empty directory for one test.
fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("registry")
        .join(test);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
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

fn manifest(name: &str, version: &str) -> String {
    format!(r#"{{"name": "{name}", "version": "{version}", "description": "A unit."}}"#)
}

/// A unit repository with a working tree.
struct Repo {
    path: PathBuf,
}

impl Repo {
    /// An empty repository at `dir/name`.
    fn new(dir: &Path, name: &str) -> Self {
        git(dir, &["init", "-q", name]);
        Self {
            path: dir.join(name),
        }
    }

    fn git(&self, args: &[&str]) -> String {
        git(&self.path, args)
    }

    fn write(&self, path: &str, contents: &str) {
        let path = self.path.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    /// Commits the manifest of `name` at `version`, with one `src/` file.
    fn commit(&self, name: &str, version: &str) {
        self.write("manifest.jsonc", &manifest(name, version));
        self.write("src/README.md", &format!("{name} {version}\n"));
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", version]);
    }

    /// Commits `name` at `version` and tags it `v<version>`.
    fn publish(&self, name: &str, version: &str) {
        self.commit(name, version);
        self.git(&["tag", &format!("v{version}")]);
    }

    fn rev(&self, name: &str) -> String {
        self.git(&["rev-parse", name])
    }
}

/// Runs `assemblr` with its registry under `data`.
fn assemblr(data: &Path, dir: &Path, args: &[&str]) -> Output {
    command(data, dir, args).output().unwrap()
}

fn command(data: &Path, dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_assemblr"));
    command
        .current_dir(dir)
        .env("ASSEMBLR_DATA_DIR", data)
        .args(args);
    command
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

fn list_json(data: &Path, dir: &Path) -> Value {
    let output = succeeds(assemblr(data, dir, &["registry", "list", "--json"]));
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The indexed versions of the unit at `index` in the listing.
fn versions(listing: &Value, index: usize) -> Vec<String> {
    listing["units"][index]["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|version| version["version"].as_str().unwrap().to_owned())
        .collect()
}

/// The registry's index file.
fn index_file(data: &Path) -> PathBuf {
    data.join("registry").join("index.json")
}

#[test]
fn registration_indexes_valid_versions_and_reports_excluded_tags() {
    let dir = scratch("registration_indexes_valid_versions");
    let data = dir.join("data");
    let repo = Repo::new(&dir, "node");
    repo.publish("@acme/node", "1.0.0");
    repo.commit("@acme/node", "1.2.0");
    repo.git(&["tag", "v1.1.0"]);
    repo.git(&["tag", "-a", "-m", "release", "v1.2.0"]);
    repo.git(&["tag", "release-1"]);
    repo.git(&["tag", "v1.0"]);

    let output = succeeds(assemblr(&data, &dir, &["registry", "add", "node"]));

    assert!(
        stdout(&output).contains("Registered @acme/node"),
        "{}",
        stdout(&output)
    );
    let stderr = stderr(&output);
    let warnings: Vec<&str> = stderr.lines().collect();
    assert_eq!(warnings.len(), 1, "{stderr}");
    assert!(warnings[0].contains("excluded tag v1.1.0"), "{stderr}");
    assert!(
        warnings[0].contains("the manifest declares version 1.2.0"),
        "{stderr}"
    );
    assert!(warnings[0].contains("not indexed"), "{stderr}");
    let stdout = stdout(&output);
    assert!(
        stdout.contains("with published versions 1.0.0, 1.2.0."),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "Excluded 1 invalid version tag(s), which are not indexed: @acme/node v1.1.0."
        ),
        "{stdout}"
    );

    let listing = list_json(&data, &dir);
    let unit = &listing["units"][0];
    assert_eq!(unit["identity"], "@acme/node");
    assert_eq!(
        Path::new(unit["source"].as_str().unwrap()),
        std::path::absolute(&repo.path).unwrap()
    );
    assert_eq!(versions(&listing, 0), ["1.0.0", "1.2.0"]);
    assert_eq!(unit["versions"][0]["commit"], repo.rev("v1.0.0^{commit}"));
    assert_eq!(unit["versions"][0]["tree"], repo.rev("v1.0.0^{tree}"));
    assert_eq!(unit["versions"][1]["commit"], repo.rev("v1.2.0^{commit}"));
}

#[test]
fn a_version_with_invalid_src_content_is_excluded() {
    let dir = scratch("invalid_src");
    let data = dir.join("data");
    let repo = Repo::new(&dir, "node");
    repo.commit("@acme/node", "1.0.0");
    let blob = repo.git(&["hash-object", "-w", "--stdin", "--path", "x"]);
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("120000,{blob},src/link"),
    ]);
    repo.git(&["commit", "-q", "-m", "link"]);
    repo.git(&["tag", "v1.0.0"]);
    repo.git(&["rm", "-q", "--cached", "src/link"]);
    repo.publish("@acme/node", "1.1.0");

    let output = succeeds(assemblr(&data, &dir, &["registry", "add", "node"]));

    let stderr = stderr(&output);
    assert!(stderr.contains("excluded tag v1.0.0"), "{stderr}");
    assert!(stderr.contains("src/link"), "{stderr}");
    assert!(stderr.contains("symbolic links"), "{stderr}");
    assert_eq!(versions(&list_json(&data, &dir), 0), ["1.1.0"]);
}

#[test]
fn an_unpublished_unit_is_registered_from_its_committed_manifest() {
    let dir = scratch("unpublished_unit");
    let data = dir.join("data");
    let repo = Repo::new(&dir, "node");
    repo.commit("@acme/node", "0.1.0");
    // Neither uncommitted nor staged changes are read.
    repo.write("manifest.jsonc", &manifest("@acme/staged", "0.2.0"));
    repo.git(&["add", "manifest.jsonc"]);
    repo.write("manifest.jsonc", &manifest("@acme/working", "0.3.0"));

    let output = succeeds(assemblr(&data, &dir, &["registry", "add", "node"]));

    assert!(
        stdout(&output).contains("with no published versions yet"),
        "{}",
        stdout(&output)
    );
    let listing = list_json(&data, &dir);
    assert_eq!(listing["units"][0]["identity"], "@acme/node");
    assert!(versions(&listing, 0).is_empty());
}

#[test]
fn an_unpublished_unit_without_a_usable_committed_manifest_is_rejected() {
    let dir = scratch("unpublished_without_manifest");
    let data = dir.join("data");
    let empty = Repo::new(&dir, "empty");
    empty.write("manifest.jsonc", &manifest("@acme/empty", "0.1.0"));
    let invalid = Repo::new(&dir, "invalid");
    invalid.write("manifest.jsonc", "{}");
    invalid.git(&["add", "-A"]);
    invalid.git(&["commit", "-q", "-m", "invalid"]);
    invalid.write("manifest.jsonc", &manifest("@acme/invalid", "0.1.0"));

    let output = fails(assemblr(&data, &dir, &["registry", "add", "empty"]));
    assert!(
        stderr(&output).contains("`HEAD` has no commit"),
        "{}",
        stderr(&output)
    );

    let output = fails(assemblr(&data, &dir, &["registry", "add", "invalid"]));
    let stderr = stderr(&output);
    assert!(stderr.contains("committed at `HEAD`"), "{stderr}");
    assert!(
        stderr.contains("registry index was not changed"),
        "{stderr}"
    );
    assert!(!data.exists(), "a rejected registration created state");
}

#[test]
fn adding_a_registered_reference_again_changes_nothing() {
    let dir = scratch("adding_again");
    let data = dir.join("data");
    let repo = Repo::new(&dir, "node");
    repo.publish("@acme/node", "1.0.0");
    succeeds(assemblr(&data, &dir, &["registry", "add", "node"]));
    let before = fs::read(index_file(&data)).unwrap();
    repo.publish("@acme/node", "1.1.0");

    let absolute = repo.path.to_string_lossy().into_owned();
    for spelling in ["node", "./node/", "node/../node", &absolute] {
        let output = succeeds(assemblr(&data, &dir, &["registry", "add", spelling]));
        assert!(
            stdout(&output).contains("already registered as @acme/node"),
            "{spelling}: {}",
            stdout(&output)
        );
    }
    assert_eq!(fs::read(index_file(&data)).unwrap(), before);
}

#[test]
fn another_reference_to_a_registered_identity_is_a_source_conflict() {
    let dir = scratch("source_conflict");
    let data = dir.join("data");
    let repo = Repo::new(&dir, "node");
    repo.publish("@acme/node", "1.0.0");
    git(&dir, &["clone", "-q", "node", "copy"]);
    succeeds(assemblr(&data, &dir, &["registry", "add", "node"]));
    let before = fs::read(index_file(&data)).unwrap();

    let output = fails(assemblr(&data, &dir, &["registry", "add", "copy"]));

    let stderr = stderr(&output);
    assert!(stderr.contains("source conflict"), "{stderr}");
    assert!(stderr.contains("@acme/node"), "{stderr}");
    assert!(
        stderr.contains("assemblr registry remove @acme/node"),
        "{stderr}"
    );
    assert_eq!(fs::read(index_file(&data)).unwrap(), before);
}

#[test]
fn versions_of_equal_precedence_reject_the_source() {
    let dir = scratch("equal_precedence");
    let data = dir.join("data");
    let repo = Repo::new(&dir, "node");
    repo.publish("@acme/node", "1.0.0+a");
    repo.publish("@acme/node", "1.0.0+b");

    let output = fails(assemblr(&data, &dir, &["registry", "add", "node"]));

    let stderr = stderr(&output);
    assert!(stderr.contains("1.0.0+a, 1.0.0+b"), "{stderr}");
    assert!(stderr.contains("build metadata"), "{stderr}");
    assert!(!data.exists(), "a rejected registration created state");
}

#[test]
fn versions_declaring_different_identities_reject_the_source() {
    let dir = scratch("different_identities");
    let data = dir.join("data");
    let repo = Repo::new(&dir, "node");
    repo.publish("@acme/old", "1.0.0");
    repo.publish("@acme/node", "2.0.0");

    let output = fails(assemblr(&data, &dir, &["registry", "add", "node"]));

    let stderr = stderr(&output);
    assert!(stderr.contains("declare different identities"), "{stderr}");
    assert!(stderr.contains("@acme/old (1.0.0)"), "{stderr}");
    assert!(!data.exists());
}

#[test]
fn invalid_paths_and_remote_references_are_rejected_without_creating_state() {
    let dir = scratch("invalid_paths");
    let data = dir.join("data");
    fs::create_dir_all(dir.join("plain")).unwrap();

    for (path, expected) in [
        ("git@github.com:acme/node.git", "only local repositories"),
        (
            "https://github.com/acme/node.git",
            "only local repositories",
        ),
        ("missing", "not a Git repository"),
        ("plain", "not a Git repository"),
    ] {
        let output = fails(assemblr(&data, &dir, &["registry", "add", path]));
        let stderr = stderr(&output);
        assert!(stderr.contains(expected), "{path}: {stderr}");
        assert!(!stderr.contains("registry remove"), "{path}: {stderr}");
    }
    assert!(!data.exists());
}

/// Windows file names cannot contain `:`.
#[cfg(unix)]
#[test]
fn a_local_directory_with_a_colon_is_registered_through_dot_slash() {
    let dir = scratch("colon_path");
    let data = dir.join("data");
    let repo = Repo::new(&dir, "host:repo");
    repo.publish("@acme/node", "1.0.0");

    let output = fails(assemblr(&data, &dir, &["registry", "add", "host:repo"]));
    assert!(
        stderr(&output).contains("starting with `./`"),
        "{}",
        stderr(&output)
    );
    assert!(!data.exists());

    succeeds(assemblr(&data, &dir, &["registry", "add", "./host:repo"]));
    let listing = list_json(&data, &dir);
    assert_eq!(
        Path::new(listing["units"][0]["source"].as_str().unwrap()),
        repo.path
    );
}

#[test]
fn listing_an_empty_registry_creates_no_state() {
    let dir = scratch("empty_listing");
    let data = dir.join("data");

    let output = succeeds(assemblr(&data, &dir, &["registry", "list"]));
    assert!(stdout(&output).contains("No unit repositories are registered"));
    assert_eq!(list_json(&data, &dir), serde_json::json!({ "units": [] }));
    succeeds(assemblr(&data, &dir, &["registry", "sync"]));

    assert!(!data.exists());
}

#[test]
fn listings_do_not_depend_on_registration_or_tag_order() {
    let dir = scratch("listing_order");
    let mut outputs = Vec::new();
    for round in 0..2 {
        let data = dir.join(format!("data{round}"));
        let mut repos = Vec::new();
        for name in ["node", "web"] {
            let repo = Repo::new(&dir, &format!("{name}{round}"));
            repo.commit(&format!("@acme/{name}"), "1.0.0");
            repo.commit(&format!("@acme/{name}"), "1.1.0");
            let mut tags = [("v1.0.0", "HEAD~1"), ("v1.1.0", "HEAD")];
            if round == 1 {
                tags.reverse();
            }
            for (tag, commit) in tags {
                repo.git(&["tag", tag, commit]);
            }
            repos.push(repo);
        }
        if round == 1 {
            repos.reverse();
        }
        for repo in &repos {
            let path = repo.path.to_string_lossy().into_owned();
            succeeds(assemblr(&data, &dir, &["registry", "add", &path]));
        }

        let mut listing = list_json(&data, &dir);
        for unit in listing["units"].as_array_mut().unwrap() {
            unit["source"] = Value::Null;
        }
        let mut text = stdout(&succeeds(assemblr(&data, &dir, &["registry", "list"])));
        for repo in &repos {
            text = text.replace(&*repo.path.to_string_lossy(), "<source>");
        }
        outputs.push((listing, text));
    }

    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0].0["units"][0]["identity"], "@acme/node");
    assert_eq!(versions(&outputs[0].0, 0), ["1.0.0", "1.1.0"]);
}

#[test]
fn sync_indexes_new_versions_and_excludes_new_invalid_tags() {
    let dir = scratch("sync_new_versions");
    let data = dir.join("data");
    let repo = Repo::new(&dir, "node");
    repo.commit("@acme/node", "0.1.0");
    succeeds(assemblr(&data, &dir, &["registry", "add", "node"]));
    repo.publish("@acme/node", "1.0.0");
    repo.commit("@acme/node", "1.2.0");
    repo.git(&["tag", "v1.1.0"]);

    let output = succeeds(assemblr(&data, &dir, &["registry", "sync"]));

    assert!(
        stdout(&output).contains("@acme/node: indexed 1.0.0"),
        "{}",
        stdout(&output)
    );
    assert!(stderr(&output).contains("excluded tag v1.1.0"));
    assert!(
        stdout(&output).contains("which are not indexed: @acme/node v1.1.0."),
        "{}",
        stdout(&output)
    );
    assert_eq!(versions(&list_json(&data, &dir), 0), ["1.0.0"]);

    let before = fs::read(index_file(&data)).unwrap();
    let output = succeeds(assemblr(&data, &dir, &["registry", "sync"]));
    assert!(stdout(&output).contains("no new published versions"));
    assert_eq!(fs::read(index_file(&data)).unwrap(), before);
}

/// A manifest of `@acme/app` at `version` depending on `@acme/node` and
/// amending `paths` of it at base 1.0.0.
fn amender_manifest(version: &str, paths: &[&str]) -> String {
    let amendments: Vec<String> = paths
        .iter()
        .map(|path| format!(r#""{path}": {{"unit": "@acme/node", "base": "1.0.0"}}"#))
        .collect();
    format!(
        r#"{{"name": "@acme/app", "version": "{version}", "description": "An app.",
            "dependencies": {{"@acme/node": "^1"}}, "amendments": {{{}}}}}"#,
        amendments.join(", ")
    )
}

/// A lockfile locking `@acme/node` at `version`, with the commit and tree
/// of `node`'s tag `v<version>`.
fn lockfile(node: &Repo, version: &str) -> String {
    format!(
        r#"{{"format": 1, "units": {{"@acme/node": {{"commit": "{}", "tree": "{}", "version": "{version}"}}}}}}"#,
        node.rev(&format!("v{version}^{{commit}}")),
        node.rev(&format!("v{version}^{{tree}}"))
    )
}

/// A registered `@acme/node` with 1.0.0 and 1.1.0, and an `@acme/app`
/// whose 0.1.0 amends `README.md` of `@acme/node` 1.0.0 consistently.
fn node_and_amender(dir: &Path, data: &Path) -> (Repo, Repo) {
    let node = Repo::new(dir, "node");
    node.publish("@acme/node", "1.0.0");
    node.publish("@acme/node", "1.1.0");
    succeeds(assemblr(data, dir, &["registry", "add", "node"]));
    let app = Repo::new(dir, "app");
    app.write("manifest.jsonc", &amender_manifest("0.1.0", &["README.md"]));
    app.write("src/app.txt", "app\n");
    app.write("amendments/README.md", "amended\n");
    app.write("assemblr-lock.json", &lockfile(&node, "1.0.0"));
    app.git(&["add", "-A"]);
    app.git(&["commit", "-q", "-m", "0.1.0"]);
    app.git(&["tag", "v0.1.0"]);
    (node, app)
}

#[test]
fn a_version_with_consistent_amendments_is_indexed_and_inconsistent_ones_are_excluded() {
    let dir = scratch("published_amendments");
    let data = dir.join("data");
    let (node, app) = node_and_amender(&dir, &data);
    // 0.2.0: a declaration without a copy, a copy without a declaration,
    // and a lockfile that locks the provider elsewhere.
    app.write(
        "manifest.jsonc",
        &amender_manifest("0.2.0", &["README.md", "missing.txt"]),
    );
    app.write("amendments/stray.txt", "stray\n");
    app.write("assemblr-lock.json", &lockfile(&node, "1.1.0"));
    app.git(&["add", "-A"]);
    app.git(&["commit", "-q", "-m", "0.2.0"]);
    app.git(&["tag", "v0.2.0"]);
    // 0.3.0: no lockfile.
    app.write("manifest.jsonc", &amender_manifest("0.3.0", &["README.md"]));
    app.git(&["rm", "-q", "amendments/stray.txt", "assemblr-lock.json"]);
    app.git(&["add", "-A"]);
    app.git(&["commit", "-q", "-m", "0.3.0"]);
    app.git(&["tag", "v0.3.0"]);
    // 0.4.0: a lockfile that is not one.
    app.write("manifest.jsonc", &amender_manifest("0.4.0", &["README.md"]));
    app.write("assemblr-lock.json", "not json");
    app.git(&["add", "-A"]);
    app.git(&["commit", "-q", "-m", "0.4.0"]);
    app.git(&["tag", "v0.4.0"]);
    // 0.5.0: a symbolic link under `amendments/`.
    app.write("manifest.jsonc", &amender_manifest("0.5.0", &["README.md"]));
    app.write("assemblr-lock.json", &lockfile(&node, "1.0.0"));
    app.git(&["add", "-A"]);
    let blob = app.git(&["hash-object", "-w", "--stdin", "--path", "x"]);
    app.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("120000,{blob},amendments/link"),
    ]);
    app.git(&["commit", "-q", "-m", "0.5.0"]);
    app.git(&["tag", "v0.5.0"]);

    let output = succeeds(assemblr(&data, &dir, &["registry", "add", "app"]));

    let stderr = stderr(&output);
    for expected in [
        "excluded tag v0.2.0, which is not indexed as a published version: its amendments are \
         invalid: amendments/README.md is based on @acme/node 1.0.0, but its assemblr-lock.json \
         locks @acme/node 1.1.0; its manifest declares an amendment of missing.txt from \
         @acme/node, but it has no amendments/missing.txt; amendments/missing.txt is based on \
         @acme/node 1.0.0, but its assemblr-lock.json locks @acme/node 1.1.0; \
         amendments/stray.txt is not declared in its manifest",
        "excluded tag v0.3.0, which is not indexed as a published version: its amendments are \
         invalid: it declares amendments but has no assemblr-lock.json",
        "excluded tag v0.4.0, which is not indexed as a published version: it declares \
         amendments, but `assemblr-lock.json` is not a valid lockfile: ",
        "excluded tag v0.5.0, which is not indexed as a published version: invalid \
         `amendments/`: `amendments/` has 1 unsupported entry:",
        "amendments/link: symbolic links",
    ] {
        assert!(stderr.contains(expected), "{expected:?} in {stderr}");
    }
    assert!(!stderr.contains("v0.1.0"), "{stderr}");
    let listing = list_json(&data, &dir);
    assert_eq!(versions(&listing, 0), ["0.1.0"]);
    assert_eq!(
        listing["units"][0]["versions"][0]["commit"],
        app.rev("v0.1.0^{commit}")
    );
}

#[test]
fn an_indexed_version_with_amendments_keeps_its_integrity() {
    let dir = scratch("published_amendments_integrity");
    let data = dir.join("data");
    let (_node, app) = node_and_amender(&dir, &data);
    succeeds(assemblr(&data, &dir, &["registry", "add", "app"]));
    let recorded = app.rev("v0.1.0");
    let before = fs::read(index_file(&data)).unwrap();

    // The tag moves to a commit whose copy differs.
    app.write("amendments/README.md", "changed\n");
    app.git(&["add", "-A"]);
    app.git(&["commit", "-q", "-m", "change the copy"]);
    app.git(&["tag", "-f", "v0.1.0"]);

    let output = fails(assemblr(&data, &dir, &["registry", "sync"]));
    let stderr = stderr(&output);
    for expected in ["integrity conflict", "tag v0.1.0 moved", &recorded] {
        assert!(stderr.contains(expected), "{expected}: {stderr}");
    }
    assert_eq!(fs::read(index_file(&data)).unwrap(), before);
}

/// Two registered units, `@acme/node` with 1.0.0 indexed and `@acme/web`,
/// which has published 1.0.0 since.
fn registered_pair(dir: &Path, data: &Path) -> (Repo, Repo) {
    let node = Repo::new(dir, "node");
    node.publish("@acme/node", "1.0.0");
    let web = Repo::new(dir, "web");
    web.commit("@acme/web", "0.1.0");
    succeeds(assemblr(data, dir, &["registry", "add", "node"]));
    succeeds(assemblr(data, dir, &["registry", "add", "web"]));
    web.publish("@acme/web", "1.0.0");
    (node, web)
}

/// Runs `sync`, which must fail with `expected` and leave the index as it
/// was, the new version of `@acme/web` included.
fn sync_is_rejected(dir: &Path, data: &Path, expected: &[&str]) {
    let before = fs::read(index_file(data)).unwrap();

    let output = fails(assemblr(data, dir, &["registry", "sync"]));

    let stderr = stderr(&output);
    for expected in expected {
        assert!(stderr.contains(expected), "{expected}: {stderr}");
    }
    assert!(
        stderr.contains("registry index was not changed"),
        "{stderr}"
    );
    assert_eq!(fs::read(index_file(data)).unwrap(), before);
    assert!(versions(&list_json(data, dir), 1).is_empty());
}

#[test]
fn a_removed_indexed_tag_rejects_the_whole_sync() {
    let dir = scratch("sync_removed_tag");
    let data = dir.join("data");
    let (node, _web) = registered_pair(&dir, &data);
    node.git(&["tag", "-d", "v1.0.0"]);

    sync_is_rejected(
        &dir,
        &data,
        &["integrity conflict", "@acme/node 1.0.0", "no longer exists"],
    );
}

#[test]
fn a_moved_indexed_tag_rejects_the_whole_sync() {
    let dir = scratch("sync_moved_tag");
    let data = dir.join("data");
    let (node, _web) = registered_pair(&dir, &data);
    let recorded = node.rev("v1.0.0");
    node.write("README.md", "changed\n");
    node.git(&["add", "-A"]);
    node.git(&["commit", "-q", "-m", "readme"]);
    node.git(&["tag", "-f", "v1.0.0"]);

    sync_is_rejected(
        &dir,
        &data,
        &["integrity conflict", "tag v1.0.0 moved", &recorded],
    );
}

#[test]
fn an_indexed_tag_that_becomes_invalid_rejects_the_whole_sync() {
    let dir = scratch("sync_invalid_tag");
    let data = dir.join("data");
    let (node, _web) = registered_pair(&dir, &data);
    node.git(&["rm", "-q", "manifest.jsonc"]);
    node.git(&["commit", "-q", "-m", "remove manifest"]);
    node.git(&["tag", "-f", "v1.0.0"]);

    sync_is_rejected(
        &dir,
        &data,
        &[
            "integrity conflict",
            "no longer valid",
            "no `manifest.jsonc`",
        ],
    );
}

#[test]
fn an_unreadable_source_rejects_the_whole_sync_and_can_be_removed() {
    let dir = scratch("sync_unreadable");
    let data = dir.join("data");
    let (node, _web) = registered_pair(&dir, &data);
    fs::remove_dir_all(&node.path).unwrap();

    sync_is_rejected(&dir, &data, &["not a Git repository", "registry remove"]);

    let output = succeeds(assemblr(&data, &dir, &["registry", "remove", "@acme/node"]));
    assert!(stdout(&output).contains("Removed @acme/node"));
    let output = succeeds(assemblr(&data, &dir, &["registry", "sync"]));
    assert!(stdout(&output).contains("@acme/web: indexed 1.0.0"));
}

#[test]
fn removal_drops_only_the_registration() {
    let dir = scratch("removal");
    let data = dir.join("data");
    let repo = Repo::new(&dir, "node");
    repo.publish("@acme/node", "1.0.0");
    repo.write("src/README.md", "uncommitted\n");
    succeeds(assemblr(&data, &dir, &["registry", "add", "node"]));
    let status = repo.git(&["status", "--porcelain"]);
    let head = repo.rev("HEAD");

    let output = succeeds(assemblr(&data, &dir, &["registry", "remove", "@acme/node"]));

    assert!(stdout(&output).contains("The repository was not modified"));
    assert_eq!(list_json(&data, &dir), serde_json::json!({ "units": [] }));
    assert_eq!(repo.git(&["status", "--porcelain"]), status);
    assert_eq!(repo.rev("HEAD"), head);
    assert_eq!(repo.rev("v1.0.0"), head);

    let output = fails(assemblr(&data, &dir, &["registry", "remove", "@acme/node"]));
    assert!(stderr(&output).contains("@acme/node is not registered"));
}

#[test]
fn removing_from_an_empty_registry_creates_no_state() {
    let dir = scratch("removal_empty");
    let data = dir.join("data");

    let output = fails(assemblr(&data, &dir, &["registry", "remove", "@acme/node"]));
    assert!(stderr(&output).contains("not registered"));

    let output = assemblr(&data, &dir, &["registry", "remove", "acme"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!data.exists());
}

#[test]
fn a_failed_index_write_leaves_the_index_unchanged() {
    let dir = scratch("failed_write");
    let data = dir.join("data");
    let node = Repo::new(&dir, "node");
    node.publish("@acme/node", "1.0.0");
    let web = Repo::new(&dir, "web");
    web.publish("@acme/web", "1.0.0");
    succeeds(assemblr(&data, &dir, &["registry", "add", "node"]));
    let before = fs::read(index_file(&data)).unwrap();
    // The new index cannot be written where a directory is in the way.
    fs::create_dir(data.join("registry").join("index.json.tmp")).unwrap();

    let output = fails(assemblr(&data, &dir, &["registry", "add", "web"]));

    assert!(
        stderr(&output).contains("could not write"),
        "{}",
        stderr(&output)
    );
    assert_eq!(fs::read(index_file(&data)).unwrap(), before);
}

#[test]
fn a_corrupt_index_is_reported_and_left_as_it_is() {
    let dir = scratch("corrupt_index");
    let data = dir.join("data");
    fs::create_dir_all(data.join("registry")).unwrap();
    fs::write(index_file(&data), "{\"format\": 99, \"units\": {}}").unwrap();
    let repo = Repo::new(&dir, "node");
    repo.publish("@acme/node", "1.0.0");

    for args in [
        &["registry", "list"][..],
        &["registry", "add", "node"],
        &["registry", "sync"],
    ] {
        let output = fails(assemblr(&data, &dir, args));
        assert!(stderr(&output).contains("format 99"), "{}", stderr(&output));
    }
    assert_eq!(
        fs::read_to_string(index_file(&data)).unwrap(),
        "{\"format\": 99, \"units\": {}}"
    );
}

#[test]
fn concurrent_registrations_lose_no_update() {
    let dir = scratch("concurrent");
    let data = dir.join("data");
    let names: Vec<String> = (0..8).map(|n| format!("unit{n}")).collect();
    for name in &names {
        Repo::new(&dir, name).publish(&format!("@acme/{name}"), "1.0.0");
    }

    let children: Vec<_> = names
        .iter()
        .map(|name| {
            command(&data, &dir, &["registry", "add", name])
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        succeeds(child.wait_with_output().unwrap());
    }

    let listing = list_json(&data, &dir);
    let identities: Vec<&str> = listing["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|unit| unit["identity"].as_str().unwrap())
        .collect();
    let expected: Vec<String> = names.iter().map(|name| format!("@acme/{name}")).collect();
    assert_eq!(identities, expected);
}

#[test]
fn only_the_implemented_registry_commands_are_offered() {
    let dir = scratch("help");
    let data = dir.join("data");

    let output = succeeds(assemblr(&data, &dir, &["registry", "--help"]));

    let help = stdout(&output);
    for command in ["add", "list", "remove", "sync"] {
        assert!(help.contains(&format!("  {command} ")), "{command}: {help}");
    }
    for missing in ["create", "info", "doctor"] {
        assert!(!help.contains(missing), "{missing}: {help}");
    }
}
