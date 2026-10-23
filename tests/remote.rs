//! Remote unit sources, run as the real binary. The remotes are repositories
//! in the test's directory, reached through `file://` URLs, so the tests are
//! offline and still go through `git fetch` like an `https://` or SSH remote.

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

use common::{Fixture, fails, git, stderr, stdout, succeeds};

impl Fixture {
    fn assemblr(&self, args: &[&str]) -> Output {
        self.command(&self.dir, args).output().unwrap()
    }

    /// A repository of `@acme/node` with `versions` published.
    fn remote(&self, name: &str, versions: &[&str]) -> Remote {
        git(&self.dir, &["init", "-q", name]);
        let remote = Remote {
            path: self.dir.join(name),
        };
        for version in versions {
            remote.publish(version);
        }
        remote
    }

    fn list(&self) -> Value {
        serde_json::from_str(&stdout(&succeeds(
            self.assemblr(&["registry", "list", "--json"]),
        )))
        .unwrap()
    }
}

struct Remote {
    path: PathBuf,
}

impl Remote {
    fn git(&self, args: &[&str]) -> String {
        git(&self.path, args)
    }

    fn publish(&self, version: &str) {
        fs::write(
            self.path.join("manifest.jsonc"),
            format!(
                r#"{{"name": "@acme/node", "version": "{version}", "description": "The Node runtime."}}"#
            ),
        )
        .unwrap();
        fs::create_dir_all(self.path.join("src")).unwrap();
        fs::write(self.path.join("src/node.txt"), format!("node {version}\n")).unwrap();
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", version]);
        self.git(&["tag", &format!("v{version}")]);
    }

    /// The repository's `file://` URL.
    fn url(&self) -> String {
        url(&self.path)
    }
}

fn url(path: &Path) -> String {
    let path = path.to_str().unwrap().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

#[test]
fn a_remote_unit_is_registered_by_url_and_read_offline_afterwards() {
    let fixture = Fixture::new("offline");
    let remote = fixture.remote("node", &["1.0.0", "1.1.0"]);
    let url = remote.url();

    let added = succeeds(fixture.assemblr(&["registry", "add", &url]));
    assert!(
        stdout(&added).contains(&format!("Registered @acme/node from {url}")),
        "{}",
        stdout(&added)
    );
    let listing = fixture.list();
    assert_eq!(listing["units"][0]["source"], url.as_str());
    assert_eq!(listing["units"][0]["versions"][1]["version"], "1.1.0");

    // Everything after registration reads Assemblr's copy: the remote can
    // be gone.
    fs::remove_dir_all(&remote.path).unwrap();
    let info = stdout(&succeeds(fixture.assemblr(&["info", "@acme/node"])));
    assert!(info.contains("@acme/node 1.1.0"), "{info}");
    assert!(info.contains(&format!("source: {url}")), "{info}");
    let found = stdout(&succeeds(fixture.assemblr(&["search", "node"])));
    assert_eq!(found, "@acme/node 1.1.0: The Node runtime.\n");

    let project = fixture.dir.join("app");
    succeeds(
        fixture
            .command(&fixture.dir, &["init", "app"])
            .output()
            .unwrap(),
    );
    succeeds(
        fixture
            .command(&project, &["add", "@acme/node"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        fs::read_to_string(project.join("node.txt")).unwrap(),
        "node 1.1.0\n"
    );

    // Registering the same reference again needs no remote either.
    let again = succeeds(fixture.assemblr(&["registry", "add", &url]));
    assert!(stdout(&again).contains("already registered"));
}

#[test]
fn sync_fetches_new_versions_and_refuses_a_moved_tag() {
    let fixture = Fixture::new("sync");
    let remote = fixture.remote("node", &["1.0.0"]);
    succeeds(fixture.assemblr(&["registry", "add", &remote.url()]));

    remote.publish("1.1.0");
    let synced = succeeds(fixture.assemblr(&["registry", "sync"]));
    assert!(stdout(&synced).contains("1.1.0"), "{}", stdout(&synced));
    assert_eq!(
        fixture.list()["units"][0]["versions"][1]["version"],
        "1.1.0"
    );

    let before = fixture.list();
    fs::write(remote.path.join("src/node.txt"), "moved\n").unwrap();
    remote.git(&["commit", "-q", "-am", "moved"]);
    remote.git(&["tag", "-f", "v1.1.0"]);
    let rejected = fails(fixture.assemblr(&["registry", "sync"]));
    assert!(stderr(&rejected).contains("1.1.0"), "{}", stderr(&rejected));
    assert_eq!(fixture.list(), before);

    // The indexed commit is still readable, though no tag names it now.
    let info = stdout(&succeeds(fixture.assemblr(&["info", "@acme/node@1.1.0"])));
    assert!(info.contains("@acme/node 1.1.0"), "{info}");
}

#[test]
fn a_deleted_tag_of_an_indexed_version_is_an_integrity_conflict() {
    let fixture = Fixture::new("deleted_tag");
    let remote = fixture.remote("node", &["1.0.0", "1.1.0"]);
    succeeds(fixture.assemblr(&["registry", "add", &remote.url()]));
    let before = fixture.list();

    remote.git(&["tag", "-d", "v1.1.0"]);
    let rejected = fails(fixture.assemblr(&["registry", "sync"]));
    assert!(stderr(&rejected).contains("1.1.0"), "{}", stderr(&rejected));
    assert_eq!(fixture.list(), before);
}

#[test]
fn an_unreachable_remote_is_rejected_and_changes_nothing() {
    let fixture = Fixture::new("unreachable");
    let missing = url(&fixture.dir.join("missing"));

    let rejected = fails(fixture.assemblr(&["registry", "add", &missing]));
    let err = stderr(&rejected);
    assert!(err.contains(&format!("could not fetch {missing}")), "{err}");
    assert!(err.contains("note: the registry was not changed"), "{err}");
    assert!(err.contains("git ls-remote"), "{err}");
    assert!(!fixture.data.join("registry").exists());

    // A source that becomes unreachable fails the whole synchronization.
    let remote = fixture.remote("node", &["1.0.0"]);
    succeeds(fixture.assemblr(&["registry", "add", &remote.url()]));
    let before = fixture.list();
    fs::remove_dir_all(&remote.path).unwrap();
    let rejected = fails(fixture.assemblr(&["registry", "sync"]));
    assert!(
        stderr(&rejected).contains("could not fetch"),
        "{}",
        stderr(&rejected)
    );
    assert_eq!(fixture.list(), before);
}

#[test]
fn a_missing_copy_is_reported_and_fetched_again_by_sync() {
    let fixture = Fixture::new("missing_copy");
    let remote = fixture.remote("node", &["1.0.0"]);
    succeeds(fixture.assemblr(&["registry", "add", &remote.url()]));

    fs::remove_dir_all(fixture.data.join("sources")).unwrap();
    let failed = fails(fixture.assemblr(&["info", "@acme/node"]));
    assert!(
        stderr(&failed).contains("`assemblr registry sync` fetches it again"),
        "{}",
        stderr(&failed)
    );

    succeeds(fixture.assemblr(&["registry", "sync"]));
    succeeds(fixture.assemblr(&["info", "@acme/node"]));
}

#[test]
fn unsafe_references_are_refused_before_anything_is_fetched() {
    let fixture = Fixture::new("refused");
    for (reference, expected) in [
        (
            "ext::sh -c touch% pwned",
            "not a repository reference Assemblr can fetch",
        ),
        (
            "git://example.com/node.git",
            "not a repository reference Assemblr can fetch",
        ),
        (
            "https://user:secret@example.com/node.git",
            "contains credentials",
        ),
    ] {
        let rejected = fails(fixture.assemblr(&["registry", "add", reference]));
        assert!(
            stderr(&rejected).contains(expected),
            "{}",
            stderr(&rejected)
        );
    }
    assert!(!fixture.dir.join("pwned").exists());
    assert!(!fixture.data.join("sources").exists());
    assert!(!fixture.data.join("registry").exists());
}
