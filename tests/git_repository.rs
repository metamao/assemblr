//! Reading published versions and their `src/` files from real Git
//! repositories, built in temporary directories.
//!
//! Most fixtures are written with Git's plumbing, object by object, so that
//! trees holding symbolic links, submodules, or names a file system rejects
//! can be built on every platform.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a fixture that cannot be built fails the test"
)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use assemblr::adapters::{
    GitErrorKind, GitRepository, InvalidVersion, ReadSrcError, SrcEntryErrorReason, VersionTag,
};
use assemblr::domain::{FileTree, RelativePath, RelativePathErrorReason, UnitFile, Version};

#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// An empty directory for one test.
fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("git")
        .join(test);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// `git` in `dir`, isolated from the developer's configuration, with fixed
/// identities and dates so that object ids are reproducible.
fn git_command(dir: &Path) -> Command {
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
    command
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
        .env("GIT_AUTHOR_NAME", "Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_NAME", "Author")
        .env("GIT_COMMITTER_EMAIL", "author@example.com")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .args(["-c", "core.autocrlf=false", "-c", "advice.nestedTag=false"]);
    command
}

/// Runs `git` in `dir` with `input`, and returns its trimmed output.
fn git_in(dir: &Path, args: &[&str], input: &[u8]) -> String {
    let mut child = git_command(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A node of a tree to write.
enum Node {
    Blob {
        mode: &'static str,
        contents: Vec<u8>,
    },
    Submodule,
    Dir(Vec<(Vec<u8>, Node)>),
}

fn file(contents: impl AsRef<[u8]>) -> Node {
    Node::Blob {
        mode: "100644",
        contents: contents.as_ref().to_vec(),
    }
}

fn executable(contents: impl AsRef<[u8]>) -> Node {
    Node::Blob {
        mode: "100755",
        contents: contents.as_ref().to_vec(),
    }
}

fn link(target: &str) -> Node {
    Node::Blob {
        mode: "120000",
        contents: target.as_bytes().to_vec(),
    }
}

fn dir<N: AsRef<[u8]>>(entries: impl IntoIterator<Item = (N, Node)>) -> Node {
    Node::Dir(
        entries
            .into_iter()
            .map(|(name, node)| (name.as_ref().to_vec(), node))
            .collect(),
    )
}

fn manifest(version: &str) -> String {
    format!(
        r#"{{
  "name": "@metamao/example",
  "version": "{version}",
  "description": "An example unit."
}}"#
    )
}

/// A unit's root tree: its manifest and, optionally, `src/`.
fn unit(version: &str, src: Option<Node>) -> Node {
    let mut entries = vec![(b"manifest.jsonc".to_vec(), file(manifest(version)))];
    if let Some(src) = src {
        entries.push((b"src".to_vec(), src));
    }
    Node::Dir(entries)
}

fn decode_hex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
        .collect()
}

struct Repo {
    path: PathBuf,
}

impl Repo {
    /// A repository with a working tree, at `dir/repo`.
    fn new(dir: &Path) -> Self {
        Self::init(dir, &[])
    }

    fn init(dir: &Path, options: &[&str]) -> Self {
        let args = [&["init", "-q"], options, &["repo"]].concat();
        git_in(dir, &args, b"");
        Self {
            path: dir.join("repo"),
        }
    }

    fn git(&self, args: &[&str]) -> String {
        git_in(&self.path, args, b"")
    }

    fn open(&self) -> GitRepository {
        GitRepository::open(&self.path).unwrap()
    }

    fn versions(&self) -> Vec<VersionTag> {
        self.open().published_versions().unwrap()
    }

    /// Writes `node`, returning its mode and object id.
    fn write(&self, node: &Node) -> (&'static str, String) {
        match node {
            Node::Blob { mode, contents } => (
                mode,
                git_in(&self.path, &["hash-object", "-w", "--stdin"], contents),
            ),
            Node::Submodule => ("160000", "1".repeat(40)),
            Node::Dir(entries) => {
                // Git orders entries by name, with a directory's name read as
                // if it ended in `/`.
                let mut entries: Vec<_> = entries
                    .iter()
                    .map(|(name, node)| {
                        let (mode, id) = self.write(node);
                        let mut key = name.clone();
                        if mode == "40000" {
                            key.push(b'/');
                        }
                        (key, name, mode, id)
                    })
                    .collect();
                entries.sort();
                let raw: Vec<u8> = entries
                    .into_iter()
                    .flat_map(|(_, name, mode, id)| {
                        [mode.as_bytes(), b" ", name, b"\0", &decode_hex(&id)].concat()
                    })
                    .collect();
                let args = ["hash-object", "-w", "-t", "tree", "--literally", "--stdin"];
                ("40000", git_in(&self.path, &args, &raw))
            }
        }
    }

    /// Commits `root` as a commit's whole tree.
    fn commit(&self, root: &Node) -> String {
        let (_, tree) = self.write(root);
        self.git(&["commit-tree", &tree, "-m", "Publish"])
    }

    fn tag(&self, name: &str, target: &str) {
        self.git(&["tag", name, target]);
    }

    fn annotated_tag(&self, name: &str, target: &str) {
        self.git(&["tag", "-a", "-m", name, name, target]);
    }
}

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn outcomes(tags: &[VersionTag]) -> Vec<(&str, Result<&Version, &InvalidVersion>)> {
    tags.iter()
        .map(|tag| {
            let outcome = tag
                .outcome()
                .map(|published| published.manifest().version());
            (tag.name(), outcome)
        })
        .collect()
}

/// Reads the `src/` files of `commit`, a published version, taking its id
/// from the version as callers do.
fn read_src(repo: &Repo, commit: &str) -> Result<FileTree, ReadSrcError> {
    let tags = repo.versions();
    let published = tags
        .iter()
        .filter_map(|tag| tag.outcome().ok())
        .find(|published| published.provenance().commit().as_str() == commit)
        .expect("the commit is a published version");
    repo.open().read_src(published.provenance().commit())
}

fn path(text: &str) -> RelativePath {
    RelativePath::parse(text).unwrap()
}

#[test]
fn lightweight_annotated_and_nested_tags_publish_versions() {
    let repo = Repo::new(&scratch("tag_kinds"));
    let first = repo.commit(&unit("1.0.0", None));
    let second = repo.commit(&unit("1.1.0", None));
    let third = repo.commit(&unit("1.2.0", None));
    repo.tag("v1.0.0", &first);
    repo.annotated_tag("v1.1.0", &second);
    repo.annotated_tag("annotated", &third);
    repo.annotated_tag("v1.2.0", "annotated");

    let tags = repo.versions();

    assert_eq!(
        outcomes(&tags),
        [
            ("v1.0.0", Ok(&version("1.0.0"))),
            ("v1.1.0", Ok(&version("1.1.0"))),
            ("v1.2.0", Ok(&version("1.2.0"))),
        ]
    );
    for (tag, commit) in tags.iter().zip([first, second, third]) {
        let provenance = tag.outcome().unwrap().provenance();
        assert_eq!(provenance.commit().as_str(), commit);
        assert_eq!(
            provenance.tree().as_str(),
            repo.git(&["rev-parse", &format!("{commit}^{{tree}}")])
        );
    }
}

#[test]
fn tags_outside_the_convention_publish_no_version() {
    let repo = Repo::new(&scratch("other_tags"));
    let commit = repo.commit(&unit("1.0.0", None));
    for name in [
        "release-1",
        "latest",
        "v1.0",
        "1.0.0",
        "V1.0.0",
        "nested/v1.0.0",
    ] {
        repo.tag(name, &commit);
    }

    assert_eq!(repo.versions(), []);
}

#[test]
fn a_repository_without_tags_has_no_versions() {
    let repo = Repo::new(&scratch("no_tags"));

    assert_eq!(repo.versions(), []);
}

#[test]
fn the_manifest_must_declare_the_tag_version_exactly() {
    let repo = Repo::new(&scratch("version_mismatch"));
    let commit = repo.commit(&unit("1.0.0", None));
    repo.tag("v1.0.1", &commit);
    repo.tag("v1.0.0+build", &commit);
    repo.tag("v1.0.0", &commit);

    let tags = repo.versions();

    assert_eq!(
        outcomes(&tags),
        [
            ("v1.0.0", Ok(&version("1.0.0"))),
            (
                "v1.0.0+build",
                Err(&InvalidVersion::VersionMismatch {
                    manifest: version("1.0.0")
                })
            ),
            (
                "v1.0.1",
                Err(&InvalidVersion::VersionMismatch {
                    manifest: version("1.0.0")
                })
            ),
        ]
    );
    let error = tags[2].outcome().unwrap_err().to_string();
    assert!(error.contains("declares version 1.0.0"), "{error}");
}

#[test]
fn each_invalid_version_is_reported_and_the_others_are_still_read() {
    let repo = Repo::new(&scratch("invalid_versions"));
    let oversized = format!(
        r#"{{"name": "@metamao/example", "version": "1.0.4", "description": "{}"}}"#,
        "x".repeat(1024 * 1024)
    );
    let cases: [(&str, Node); 6] = [
        ("1.0.0", dir([("README.md", file("No manifest."))])),
        ("1.0.1", dir([("manifest.jsonc", file("{ not json"))])),
        ("1.0.2", dir([("manifest.jsonc", dir([("x", file("x"))]))])),
        ("1.0.3", dir([("manifest.jsonc", link("other.jsonc"))])),
        ("1.0.4", dir([("manifest.jsonc", file(oversized))])),
        ("1.0.5", dir([("manifest.jsonc", file(b"\xff\xfe"))])),
    ];
    for (version, root) in &cases {
        let commit = repo.commit(root);
        repo.tag(&format!("v{version}"), &commit);
    }
    let (_, tree) = repo.write(&unit("1.0.6", None));
    repo.tag("v1.0.6", &tree);
    repo.tag("v2.0.0", &repo.commit(&unit("2.0.0", None)));

    let tags = repo.versions();
    let reasons: Vec<_> = tags
        .iter()
        .map(|tag| (tag.name(), tag.outcome().err()))
        .collect();

    assert_eq!(reasons.len(), 8);
    assert_eq!(
        reasons[0],
        ("v1.0.0", Some(&InvalidVersion::MissingManifest))
    );
    assert!(matches!(
        reasons[1],
        ("v1.0.1", Some(InvalidVersion::InvalidManifest(_)))
    ));
    assert_eq!(
        reasons[2],
        ("v1.0.2", Some(&InvalidVersion::ManifestNotAFile))
    );
    assert_eq!(
        reasons[3],
        ("v1.0.3", Some(&InvalidVersion::ManifestNotAFile))
    );
    assert!(matches!(
        reasons[4],
        ("v1.0.4", Some(InvalidVersion::ManifestTooLarge { size, limit: 1_048_576 }))
            if *size > 1_048_576
    ));
    assert_eq!(
        reasons[5],
        ("v1.0.5", Some(&InvalidVersion::ManifestNotUtf8))
    );
    assert_eq!(
        reasons[6],
        (
            "v1.0.6",
            Some(&InvalidVersion::NotACommit {
                object_type: "tree".to_owned()
            })
        )
    );
    assert_eq!(reasons[7], ("v2.0.0", None));
    let error = tags[1].outcome().unwrap_err().to_string();
    assert!(error.contains("not valid JSONC"), "{error}");
}

#[test]
fn the_order_tags_were_created_in_does_not_matter() {
    let names = ["v2.0.0", "v1.0.0", "v1.10.0", "v1.2.0", "v1.2.0-beta.1"];
    let read = |test: &str, order: &[usize]| {
        let repo = Repo::new(&scratch(test));
        for &index in order {
            let name = names[index];
            let commit = repo.commit(&unit(&name[1..], None));
            repo.annotated_tag(name, &commit);
        }
        repo.versions()
    };

    let forward = read("tag_order_forward", &[0, 1, 2, 3, 4]);
    let shuffled = read("tag_order_shuffled", &[3, 0, 4, 2, 1]);

    assert_eq!(forward, shuffled);
    let order: Vec<_> = forward.iter().map(VersionTag::name).collect();
    assert_eq!(
        order,
        ["v1.0.0", "v1.2.0-beta.1", "v1.2.0", "v1.10.0", "v2.0.0"]
    );
}

#[test]
fn src_files_are_read_exactly_as_committed() {
    let repo = Repo::new(&scratch("src_files"));
    let binary = [0, 159, 146, 150, 255, b'\n', 0];
    let commit = repo.commit(&unit(
        "1.0.0",
        Some(dir([
            ("README.md", file("Line one\r\nLine two\n")),
            (
                "bin",
                dir([("run.sh", executable("#!/bin/sh\necho run\n"))]),
            ),
            (
                "assets",
                dir([("logo.bin", file(binary)), ("empty", file(""))]),
            ),
        ])),
    ));
    repo.tag("v1.0.0", &commit);

    let files = read_src(&repo, &commit).unwrap();

    assert_eq!(
        files,
        FileTree::from([
            (
                path("README.md"),
                UnitFile::new(b"Line one\r\nLine two\n".to_vec(), false)
            ),
            (path("assets/empty"), UnitFile::new(Vec::new(), false)),
            (
                path("assets/logo.bin"),
                UnitFile::new(binary.to_vec(), false)
            ),
            (
                path("bin/run.sh"),
                UnitFile::new(b"#!/bin/sh\necho run\n".to_vec(), true)
            ),
        ])
    );
}

#[test]
fn a_unit_without_src_has_no_files() {
    let repo = Repo::new(&scratch("no_src"));
    let commit = repo.commit(&unit("1.0.0", None));
    repo.tag("v1.0.0", &commit);

    assert_eq!(read_src(&repo, &commit).unwrap(), FileTree::new());
}

#[test]
fn src_must_be_a_directory() {
    let repo = Repo::new(&scratch("src_not_a_directory"));
    let commit = repo.commit(&unit("1.0.0", Some(file("not a directory"))));
    repo.tag("v1.0.0", &commit);

    let error = read_src(&repo, &commit).unwrap_err();

    assert!(matches!(error, ReadSrcError::SrcNotADirectory), "{error}");
}

#[test]
fn every_unsupported_src_entry_is_reported_by_path() {
    let repo = Repo::new(&scratch("src_entries"));
    let src = dir::<&[u8]>([
        (b"fine.txt".as_slice(), file("fine")),
        (b"link".as_slice(), link("fine.txt")),
        (b"vendor".as_slice(), Node::Submodule),
        (b"con.txt".as_slice(), file("reserved on Windows")),
        (b"a:b".as_slice(), file("a stream on Windows")),
        (b"back\\slash".as_slice(), file("a separator on Windows")),
        (b"caf\xe9".as_slice(), file("not UTF-8")),
        (b"..".as_slice(), dir([("escape", file("up"))])),
        (b"nested".as_slice(), dir([("deep", link("../fine.txt"))])),
    ]);
    let commit = repo.commit(&unit("1.0.0", Some(src)));
    repo.tag("v1.0.0", &commit);

    let Err(ReadSrcError::Entries(errors)) = read_src(&repo, &commit) else {
        panic!("the entries are rejected");
    };
    let reasons: Vec<_> = errors
        .iter()
        .map(|error| (error.path(), error.reason()))
        .collect();

    assert_eq!(reasons.len(), 8, "{reasons:?}");
    let reason = |path: &str| {
        reasons
            .iter()
            .find(|(found, _)| *found == path)
            .unwrap_or_else(|| panic!("{path} is reported: {reasons:?}"))
            .1
    };
    assert_eq!(reason("link"), &SrcEntryErrorReason::SymbolicLink);
    assert_eq!(reason("nested/deep"), &SrcEntryErrorReason::SymbolicLink);
    assert_eq!(reason("vendor"), &SrcEntryErrorReason::Submodule);
    assert_eq!(reason("caf\u{fffd}"), &SrcEntryErrorReason::NotUtf8);
    let invalid = |path: &str| match reason(path) {
        SrcEntryErrorReason::InvalidPath(error) => error.reason().clone(),
        other => panic!("{path}: {other:?}"),
    };
    assert!(matches!(
        invalid("con.txt"),
        RelativePathErrorReason::ReservedName(_)
    ));
    assert!(matches!(invalid("a:b"), RelativePathErrorReason::Colon(_)));
    assert_eq!(invalid("back\\slash"), RelativePathErrorReason::Backslash);
    assert_eq!(
        invalid("../escape"),
        RelativePathErrorReason::ParentDirectory
    );
    let mut paths: Vec<_> = reasons.iter().map(|(path, _)| *path).collect();
    paths.sort_unstable();
    assert_eq!(
        reasons.iter().map(|(path, _)| *path).collect::<Vec<_>>(),
        paths
    );

    let message = ReadSrcError::Entries(errors.clone()).to_string();
    assert!(message.contains("src/link: symbolic links"), "{message}");
    assert!(message.contains("src/vendor: submodules"), "{message}");
}

#[test]
fn sha256_repositories_are_read() {
    let repo = Repo::init(&scratch("sha256"), &["--object-format=sha256"]);
    let commit = repo.commit(&unit(
        "1.0.0",
        Some(dir([("app.json", file("{}")), ("run", executable("run"))])),
    ));
    repo.tag("v1.0.0", &commit);

    let tags = repo.versions();
    let provenance = tags[0].outcome().unwrap().provenance();
    assert_eq!(provenance.commit().as_str().len(), 64);
    assert_eq!(
        read_src(&repo, &commit).unwrap(),
        FileTree::from([
            (path("app.json"), UnitFile::new(b"{}".to_vec(), false)),
            (path("run"), UnitFile::new(b"run".to_vec(), true)),
        ])
    );
}

/// Everything about a working tree that reading must leave alone.
fn checkout_state(repo: &Repo) -> (String, String, Vec<u8>, Vec<u8>, Vec<u8>) {
    (
        repo.git(&["rev-parse", "HEAD"]),
        repo.git(&["status", "--porcelain=v1", "--untracked-files=all"]),
        fs::read(repo.path.join(".git/index")).unwrap(),
        fs::read(repo.path.join("manifest.jsonc")).unwrap(),
        fs::read(repo.path.join("src/app.txt")).unwrap(),
    )
}

#[test]
fn reading_leaves_the_working_tree_index_and_head_unchanged() {
    let repo = Repo::new(&scratch("working_tree"));
    fs::create_dir(repo.path.join("src")).unwrap();
    fs::write(repo.path.join("manifest.jsonc"), manifest("1.0.0")).unwrap();
    fs::write(repo.path.join("src/app.txt"), "published").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-q", "-m", "Publish"]);
    repo.git(&["tag", "v1.0.0"]);
    // Uncommitted work of every kind: staged, unstaged, and untracked.
    fs::write(repo.path.join("manifest.jsonc"), manifest("1.1.0")).unwrap();
    repo.git(&["add", "manifest.jsonc"]);
    fs::write(repo.path.join("src/app.txt"), "editing").unwrap();
    fs::write(repo.path.join("src/new.txt"), "untracked").unwrap();
    let before = checkout_state(&repo);

    let tags = repo.versions();
    let commit = tags[0].outcome().unwrap().provenance().commit();
    let files = repo.open().read_src(commit).unwrap();

    assert_eq!(checkout_state(&repo), before);
    assert_eq!(
        files,
        FileTree::from([(path("app.txt"), UnitFile::new(b"published".to_vec(), false))])
    );
}

#[test]
fn bare_repositories_and_linked_worktrees_are_read() {
    let base = scratch("repository_layouts");
    let repo = Repo::init(&base, &["--bare"]);
    let commit = repo.commit(&unit("1.0.0", None));
    repo.tag("v1.0.0", &commit);
    let worktree = base.join("worktree");
    git_in(
        &repo.path,
        &[
            "worktree",
            "add",
            "-q",
            &worktree.to_string_lossy(),
            "v1.0.0",
        ],
        b"",
    );

    for path in [&repo.path, &worktree] {
        let tags = GitRepository::open(path)
            .unwrap()
            .published_versions()
            .unwrap();
        assert_eq!(outcomes(&tags), [("v1.0.0", Ok(&version("1.0.0")))]);
    }
}

#[test]
fn only_a_repository_itself_can_be_opened() {
    let base = scratch("not_a_repository");
    let repo = Repo::new(&base);
    fs::create_dir(repo.path.join("src")).unwrap();
    fs::create_dir(base.join("plain")).unwrap();

    for path in [
        repo.path.join("src"),
        base.join("plain"),
        base.join("absent"),
    ] {
        let error = GitRepository::open(&path).unwrap_err();
        assert!(
            matches!(error.kind(), GitErrorKind::NotARepository(_)),
            "{}: {error}",
            path.display()
        );
        assert_eq!(error.repository(), path);
        assert!(
            error.to_string().contains("is not a Git repository"),
            "{error}"
        );
    }
}

#[test]
fn replace_refs_do_not_change_what_is_read() {
    let repo = Repo::new(&scratch("replace_refs"));
    let commit = repo.commit(&unit("1.0.0", Some(dir([("app.txt", file("published"))]))));
    repo.tag("v1.0.0", &commit);
    let (_, replacement_manifest) = repo.write(&file(manifest("9.9.9")));
    let (_, replacement_file) = repo.write(&file("replaced"));
    let manifest_blob = repo.git(&["rev-parse", &format!("{commit}:manifest.jsonc")]);
    let file_blob = repo.git(&["rev-parse", &format!("{commit}:src/app.txt")]);
    repo.git(&["replace", &manifest_blob, &replacement_manifest]);
    repo.git(&["replace", &file_blob, &replacement_file]);

    assert_eq!(
        outcomes(&repo.versions()),
        [("v1.0.0", Ok(&version("1.0.0")))]
    );
    assert_eq!(
        read_src(&repo, &commit).unwrap(),
        FileTree::from([(path("app.txt"), UnitFile::new(b"published".to_vec(), false))])
    );
}

#[test]
fn programs_the_repository_configures_are_not_run() {
    let repo = Repo::new(&scratch("configured_programs"));
    let commit = repo.commit(&unit(
        "1.0.0",
        Some(dir([
            (".gitattributes", file("* filter=rewrite\n")),
            ("app.txt", file("published")),
        ])),
    ));
    repo.tag("v1.0.0", &commit);
    // Each would leave a marker in the directory Git runs in.
    repo.git(&["config", "filter.rewrite.smudge", "echo ran > filter-ran"]);
    repo.git(&["config", "filter.rewrite.required", "true"]);
    repo.git(&["config", "core.fsmonitor", "echo ran > fsmonitor-ran"]);
    repo.git(&["config", "core.pager", "echo ran > pager-ran"]);

    let files = read_src(&repo, &commit).unwrap();

    assert_eq!(files[&path("app.txt")].contents(), b"published");
    for marker in ["filter-ran", "fsmonitor-ran", "pager-ran"] {
        assert!(!repo.path.join(marker).exists(), "{marker}");
    }
}

#[test]
fn missing_objects_are_never_fetched() {
    let base = scratch("partial_clone");
    let source = Repo::new(&base);
    let commit = source.commit(&unit("1.0.0", Some(dir([("app.txt", file("published"))]))));
    source.tag("v1.0.0", &commit);
    source.git(&["config", "uploadpack.allowFilter", "true"]);
    let url = format!(
        "file://{}{}",
        if cfg!(windows) { "/" } else { "" },
        source.path.to_string_lossy().replace('\\', "/")
    );
    // A partial clone without blobs, whose Git would fetch them on demand.
    git_in(
        &base,
        &["clone", "-q", "--bare", "--filter=blob:none", &url, "clone"],
        b"",
    );
    let clone = Repo {
        path: base.join("clone"),
    };

    let error = clone.open().published_versions().unwrap_err();
    assert!(
        matches!(
            error.kind(),
            GitErrorKind::MissingObject(_) | GitErrorKind::Failed { .. }
        ),
        "{error}"
    );
    let tags = source.versions();
    let commit_id = tags[0].outcome().unwrap().provenance().commit();
    assert!(clone.open().read_src(commit_id).is_err());
    let manifest_blob = clone.git(&["rev-parse", &format!("{commit}:manifest.jsonc")]);
    let present = git_command(&clone.path)
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "none")
        .args(["cat-file", "-e", &manifest_blob])
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!present.success(), "the blob was fetched");
}

/// A directory whose `width` entries all share one subtree, `depth` levels
/// down to `width` files. A few objects describe `width^depth` files.
fn shared_subtrees(repo: &Repo, width: usize, depth: usize) -> String {
    let (_, blob) = repo.write(&file("shared"));
    let mut level = ("100644", blob);
    for _ in 0..depth {
        let raw: Vec<u8> = (0..width)
            .flat_map(|index| {
                let name = format!("e{index:03}");
                [
                    level.0.as_bytes(),
                    b" ",
                    name.as_bytes(),
                    b"\0",
                    &decode_hex(&level.1),
                ]
                .concat()
            })
            .collect();
        let args = ["hash-object", "-w", "-t", "tree", "--literally", "--stdin"];
        level = ("40000", git_in(&repo.path, &args, &raw));
    }
    level.1
}

/// Commits a unit whose `src/` is the tree `src`.
fn commit_with_src_tree(repo: &Repo, version: &str, src: &str) -> String {
    let (_, manifest_blob) = repo.write(&file(manifest(version)));
    let raw = [
        b"100644 manifest.jsonc\0".as_slice(),
        &decode_hex(&manifest_blob),
        b"40000 src\0",
        &decode_hex(src),
    ]
    .concat();
    let args = ["hash-object", "-w", "-t", "tree", "--literally", "--stdin"];
    let root = git_in(&repo.path, &args, &raw);
    repo.git(&["commit-tree", &root, "-m", "Publish"])
}

#[test]
fn shared_subtrees_cannot_expand_src_without_bound() {
    let repo = Repo::new(&scratch("shared_subtrees"));
    // 25 + 625 + 15,625 entries from four objects.
    let src = shared_subtrees(&repo, 25, 3);
    let commit = commit_with_src_tree(&repo, "1.0.0", &src);
    repo.tag("v1.0.0", &commit);

    let error = read_src(&repo, &commit).unwrap_err();

    assert!(
        matches!(error, ReadSrcError::TooManyEntries { limit: 10_000 }),
        "{error}"
    );
    assert!(
        error.to_string().contains("more than 10000 files"),
        "{error}"
    );
}

#[test]
fn src_files_are_bounded_one_by_one_and_in_total() {
    const MIB: usize = 1024 * 1024;
    let repo = Repo::new(&scratch("src_sizes"));
    // Git compresses these to a few kilobytes, but they expand when read.
    let at_limit = vec![0; 16 * MIB];
    let nine_at_limit = dir((0..9).map(|index| (format!("f{index}"), file(&at_limit))));
    let total = repo.commit(&unit("1.0.0", Some(nine_at_limit)));
    repo.tag("v1.0.0", &total);
    let oversized = repo.commit(&unit(
        "1.1.0",
        Some(dir([
            ("big.bin", file(vec![0; 16 * MIB + 1])),
            ("ok.txt", file("ok")),
        ])),
    ));
    repo.tag("v1.1.0", &oversized);

    let error = read_src(&repo, &total).unwrap_err();
    assert!(
        matches!(error, ReadSrcError::TooLarge { size, limit } if size == 9 * 16 * MIB as u64 && limit == 128 * MIB as u64),
        "{error}"
    );

    let Err(ReadSrcError::Entries(errors)) = read_src(&repo, &oversized) else {
        panic!("the oversized file is rejected");
    };
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].path(), "big.bin");
    assert_eq!(
        errors[0].reason(),
        &SrcEntryErrorReason::TooLarge {
            size: 16 * MIB as u64 + 1,
            limit: 16 * MIB as u64
        }
    );
    assert!(
        errors[0].to_string().starts_with("src/big.bin: "),
        "{}",
        errors[0]
    );
}

/// A tree whose single entry has a name long enough to make the tree
/// larger than any directory listing Assemblr reads.
fn oversized_tree(repo: &Repo) -> String {
    let (_, blob) = repo.write(&file("x"));
    let name = "n".repeat(5 * 1024 * 1024);
    let raw = [
        b"100644 ".as_slice(),
        name.as_bytes(),
        b"\0",
        &decode_hex(&blob),
    ]
    .concat();
    let args = ["hash-object", "-w", "-t", "tree", "--literally", "--stdin"];
    git_in(&repo.path, &args, &raw)
}

#[test]
fn oversized_src_directories_are_reported_by_path() {
    let repo = Repo::new(&scratch("oversized_directory"));
    let big = oversized_tree(&repo);
    let (_, fine) = repo.write(&file("fine"));
    let raw = [
        b"40000 big\0".as_slice(),
        &decode_hex(&big),
        b"100644 fine.txt\0",
        &decode_hex(&fine),
    ]
    .concat();
    let args = ["hash-object", "-w", "-t", "tree", "--literally", "--stdin"];
    let src = git_in(&repo.path, &args, &raw);
    let commit = commit_with_src_tree(&repo, "1.0.0", &src);
    repo.tag("v1.0.0", &commit);

    let Err(ReadSrcError::Entries(errors)) = read_src(&repo, &commit) else {
        panic!("the oversized directory is rejected");
    };
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].path(), "big");
    assert!(matches!(
        errors[0].reason(),
        SrcEntryErrorReason::TooLarge {
            limit: 4_194_304,
            ..
        }
    ));
}

#[test]
fn oversized_commits_and_root_trees_are_invalid_versions() {
    let repo = Repo::new(&scratch("oversized_objects"));
    let (_, tree) = repo.write(&unit("1.0.0", None));
    let message = "m".repeat(1024 * 1024 + 1);
    let long_commit = git_in(
        &repo.path,
        &["commit-tree", &tree, "-F", "-"],
        message.as_bytes(),
    );
    repo.tag("v1.0.0", &long_commit);
    let big_root = oversized_tree(&repo);
    repo.tag(
        "v1.1.0",
        &repo.git(&["commit-tree", &big_root, "-m", "Publish"]),
    );
    repo.tag("v2.0.0", &repo.commit(&unit("2.0.0", None)));

    let tags = repo.versions();
    let too_large: Vec<_> = tags
        .iter()
        .map(|tag| match tag.outcome() {
            Err(InvalidVersion::ObjectTooLarge(too_large)) => {
                Some((too_large.object_type(), too_large.limit()))
            }
            _ => None,
        })
        .collect();

    assert_eq!(
        too_large,
        [Some(("commit", 1_048_576)), Some(("tree", 4_194_304)), None]
    );
    assert_eq!(outcomes(&tags)[2], ("v2.0.0", Ok(&version("2.0.0"))));
    let message = tags[0].outcome().unwrap_err().to_string();
    assert!(
        message.starts_with(&format!("commit {long_commit} has ")),
        "{message}"
    );
}

#[test]
fn repositories_open_by_relative_path() {
    let repo = Repo::new(&scratch("relative_path"));
    let commit = repo.commit(&unit("1.0.0", None));
    repo.tag("v1.0.0", &commit);
    let relative = relative_to_current_dir(&repo.path);
    assert!(relative.is_relative());

    let opened = GitRepository::open(&relative).unwrap();

    assert_eq!(opened.path(), relative);
    let tags = opened.published_versions().unwrap();
    assert_eq!(outcomes(&tags), [("v1.0.0", Ok(&version("1.0.0")))]);
    let commit = tags[0].outcome().unwrap().provenance().commit();
    assert_eq!(opened.read_src(commit).unwrap(), FileTree::new());
}

/// `path` relative to the current directory, which Cargo sets to the
/// package root for tests.
fn relative_to_current_dir(path: &Path) -> PathBuf {
    let current = std::env::current_dir().unwrap();
    let mut base = current.as_path();
    let mut up = PathBuf::new();
    loop {
        if let Ok(rest) = path.strip_prefix(base) {
            return up.join(rest);
        }
        base = base.parent().expect("the paths share a root");
        up.push("..");
    }
}
