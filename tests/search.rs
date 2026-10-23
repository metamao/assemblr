//! `assemblr search`, run as the real binary against registries built in
//! temporary directories. Every test gives the binary its own registry and
//! home.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a fixture that cannot be built fails the test"
)]

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Output;

use serde_json::{Value, json};

use common::{Fixture, fails, git, stderr, stdout, succeeds};

impl Fixture {
    fn assemblr(&self, args: &[&str]) -> Output {
        self.command(&self.dir, args).output().unwrap()
    }

    /// A registered repository of `identity`, with `versions` published, each
    /// described by `description` and its version.
    fn unit(&self, identity: &str, description: &str, versions: &[&str]) -> PathBuf {
        let name = identity.rsplit('/').next().unwrap();
        git(&self.dir, &["init", "-q", name]);
        let path = self.dir.join(name);
        let write = |version: &str| {
            fs::write(
                path.join("manifest.jsonc"),
                format!(
                    r#"{{"name": "{identity}", "version": "{version}", "description": "{description}"}}"#
                ),
            )
            .unwrap();
            git(&path, &["add", "-A"]);
            git(&path, &["commit", "-q", "-m", version]);
        };
        if versions.is_empty() {
            write("0.1.0");
        }
        for version in versions {
            write(version);
            git(&path, &["tag", &format!("v{version}")]);
        }
        succeeds(self.assemblr(&["registry", "add", path.to_str().unwrap()]));
        path
    }
}

fn lines(output: &Output) -> Vec<String> {
    stdout(output).lines().map(str::to_owned).collect()
}

#[test]
fn search_matches_identities_and_descriptions_in_identity_order() {
    let fixture = Fixture::new("order");
    // Registered out of order, so the listing's order cannot be the
    // registration's.
    fixture.unit("@acme/worker", "Deploys to Cloudflare Workers.", &["1.0.0"]);
    fixture.unit(
        "@acme/cloudflare",
        "Cloudflare account settings.",
        &["1.0.0", "1.1.0"],
    );
    fixture.unit("@acme/neon", "A Neon Postgres database.", &["2.0.0"]);

    let found = succeeds(fixture.assemblr(&["search", "CLOUDFLARE"]));
    assert_eq!(
        lines(&found),
        [
            "@acme/cloudflare 1.1.0: Cloudflare account settings.",
            "@acme/worker 1.0.0: Deploys to Cloudflare Workers.",
        ]
    );
    let found = succeeds(fixture.assemblr(&["search", "acme/neon"]));
    assert_eq!(
        lines(&found),
        ["@acme/neon 2.0.0: A Neon Postgres database."]
    );
}

#[test]
fn every_term_must_match_the_identity_or_the_description() {
    let fixture = Fixture::new("terms");
    fixture.unit("@acme/worker", "Deploys to Cloudflare Workers.", &["1.0.0"]);
    fixture.unit(
        "@acme/cloudflare",
        "Cloudflare account settings.",
        &["1.0.0"],
    );

    let found = succeeds(fixture.assemblr(&["search", "cloudflare", "worker"]));
    assert_eq!(
        lines(&found),
        ["@acme/worker 1.0.0: Deploys to Cloudflare Workers."]
    );
    let quoted = succeeds(fixture.assemblr(&["search", "cloudflare worker"]));
    assert_eq!(stdout(&quoted), stdout(&found));
}

#[test]
fn search_describes_the_newest_stable_version() {
    let fixture = Fixture::new("stable");
    fixture.unit(
        "@acme/node",
        "The Node runtime.",
        &["1.0.0", "2.0.0-beta.1"],
    );

    let found = succeeds(fixture.assemblr(&["search", "node"]));
    assert_eq!(lines(&found), ["@acme/node 1.0.0: The Node runtime."]);
}

#[test]
fn search_json_lists_the_same_units() {
    let fixture = Fixture::new("json");
    let worker = fixture.unit("@acme/worker", "Deploys to Cloudflare Workers.", &["1.0.0"]);
    let draft = fixture.unit("@acme/draft-worker", "Unpublished.", &[]);

    let found = succeeds(fixture.assemblr(&["search", "worker", "--json"]));
    let json: Value = serde_json::from_str(&stdout(&found)).unwrap();
    assert_eq!(
        json,
        json!({
            "units": [
                {
                    "identity": "@acme/draft-worker",
                    "source": draft.to_str().unwrap(),
                    "version": null,
                    "description": null,
                },
                {
                    "identity": "@acme/worker",
                    "source": worker.to_str().unwrap(),
                    "version": "1.0.0",
                    "description": "Deploys to Cloudflare Workers.",
                },
            ]
        })
    );
}

#[test]
fn a_unit_without_a_published_version_is_matched_on_its_identity() {
    let fixture = Fixture::new("unpublished");
    fixture.unit("@acme/draft", "Matches nothing by description.", &[]);

    let found = succeeds(fixture.assemblr(&["search", "draft"]));
    assert_eq!(lines(&found), ["@acme/draft (no published version)"]);
    assert_eq!(stderr(&found), "");
    let missed = succeeds(fixture.assemblr(&["search", "description"]));
    assert_eq!(stdout(&missed), "");
}

#[test]
fn an_unreadable_repository_is_reported_and_does_not_hide_other_units() {
    let fixture = Fixture::new("unreadable");
    let gone = fixture.unit("@acme/gone", "Moved away.", &["1.0.0"]);
    fixture.unit("@acme/kept", "Still here, and moved nowhere.", &["1.0.0"]);
    fs::remove_dir_all(&gone).unwrap();

    let found = succeeds(fixture.assemblr(&["search", "moved"]));
    assert_eq!(
        lines(&found),
        ["@acme/kept 1.0.0: Still here, and moved nowhere."]
    );
    let err = stderr(&found);
    assert!(
        err.contains("warning: the repository of @acme/gone"),
        "{err}"
    );
    assert!(err.contains("matched on its identity only"), "{err}");

    let by_identity = succeeds(fixture.assemblr(&["search", "gone"]));
    assert_eq!(
        lines(&by_identity),
        ["@acme/gone (not described: see the warning above)"]
    );
}

#[test]
fn no_match_succeeds_with_a_note() {
    let fixture = Fixture::new("none");
    fixture.unit("@acme/node", "The Node runtime.", &["1.0.0"]);

    let found = succeeds(fixture.assemblr(&["search", "docker"]));
    assert_eq!(stdout(&found), "");
    assert!(
        stderr(&found).contains("no registered unit matches \"docker\""),
        "{}",
        stderr(&found)
    );
    let empty = succeeds(fixture.assemblr(&["search", "docker", "--json"]));
    let json: Value = serde_json::from_str(&stdout(&empty)).unwrap();
    assert_eq!(json, json!({ "units": [] }));
}

#[test]
fn a_search_without_terms_is_rejected() {
    let fixture = Fixture::new("empty");
    let blank = fails(fixture.assemblr(&["search", " "]));
    assert!(
        stderr(&blank).contains("error: the search has no term"),
        "{}",
        stderr(&blank)
    );
    // A usage error, reported by the argument parser.
    assert!(!fixture.assemblr(&["search"]).status.success());
}
