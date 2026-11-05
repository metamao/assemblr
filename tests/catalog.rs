//! Catalogs, run as a real process: a repository with `catalog.jsonc` and
//! its units in `units/`, built, checked, and published together, then
//! registered and composed like any other units.
//!
//! Every test gives the binary its own registry through `ASSEMBLR_DATA_DIR`
//! and its own home directory (see `tests/common`).

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
    fn run(&self, dir: &Path, args: &[&str]) -> Output {
        self.command(dir, args).output().unwrap()
    }

    /// A unit repository outside any catalog, `@acme/base` 1.0.0 with one
    /// file, tagged and registered.
    fn base(&self) -> PathBuf {
        let path = self.dir.join("base");
        git(&self.dir, &["init", "-q", "base"]);
        write(
            &path,
            "manifest.jsonc",
            r#"{"name": "@acme/base", "version": "1.0.0", "description": "A base."}"#,
        );
        write(&path, "src/base.txt", "base\n");
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-q", "-m", "base"]);
        git(&path, &["tag", "v1.0.0"]);
        succeeds(self.run(&self.dir, &["registry", "add", path.to_str().unwrap()]));
        path
    }

    /// A catalog repository, `@acme/web` at `version`, with no units yet.
    fn catalog(&self, version: &str) -> PathBuf {
        let path = self.dir.join("web");
        git(&self.dir, &["init", "-q", "web"]);
        write(
            &path,
            "catalog.jsonc",
            &format!(r#"{{"name": "@acme/web", "version": "{version}"}}"#),
        );
        path
    }

    /// The catalog of the acceptance scenario: `a` depends on `@acme/base`
    /// in another repository, `b` on its sibling `a`, and `c` on nothing.
    fn three_units(&self) -> PathBuf {
        self.base();
        let catalog = self.catalog("1.0.0");
        for unit in ["a", "b", "c"] {
            succeeds(self.run(&catalog, &["unit", "init", &format!("@acme/{unit}")]));
            write(
                &catalog,
                &format!("units/{unit}/src/{unit}.txt"),
                &format!("{unit}\n"),
            );
        }
        write(
            &catalog,
            "units/a/manifest.jsonc",
            &unit("a", r#""@acme/base": "^1.0.0""#),
        );
        write(
            &catalog,
            "units/b/manifest.jsonc",
            &unit("b", r#""@acme/a": "catalog""#),
        );
        catalog
    }
}

/// The manifest of the unit `name` in the catalog, with these dependencies.
fn unit(name: &str, dependencies: &str) -> String {
    format!(
        r#"{{"name": "@acme/{name}", "description": "The {name} unit.",
            "dependencies": {{{dependencies}}}}}"#
    )
}

fn write(root: &Path, path: &str, contents: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

/// The files of a directory, sorted, as paths relative to it.
fn files(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path.strip_prefix(root).unwrap();
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn a_catalog_is_built_published_with_one_tag_registered_and_composed() {
    let fixture = Fixture::new("acceptance");
    let catalog = fixture.three_units();

    let output = succeeds(fixture.run(&catalog, &["unit", "build"]));
    let out = stdout(&output);
    for unit in ["a", "b", "c"] {
        assert!(
            out.contains(&format!(
                "Built @acme/{unit} 1.0.0 into units/{unit}/build/"
            )),
            "{out}"
        );
    }
    assert_eq!(
        files(&catalog.join("units/b/build")),
        ["a.txt", "b.txt", "base.txt"]
    );
    // One lockfile, at the top level, with what the units depend on in
    // other repositories only.
    let lock = read(&catalog.join("assemblr-lock.json"));
    assert!(lock.contains("@acme/base"), "{lock}");
    assert!(!lock.contains("@acme/a"), "{lock}");
    for unit in ["a", "b", "c"] {
        assert!(
            !catalog
                .join(format!("units/{unit}/assemblr-lock.json"))
                .exists()
        );
    }

    let output = succeeds(fixture.run(&catalog, &["unit", "check"]));
    assert!(
        stdout(&output).contains("Checked @acme/b 1.0.0: no problems"),
        "{}",
        stdout(&output)
    );
    // In a unit's directory, the command builds that unit.
    let output = succeeds(fixture.run(&catalog.join("units/b"), &["unit", "build"]));
    assert!(
        stdout(&output).contains("Built @acme/b 1.0.0 into build/"),
        "{}",
        stdout(&output)
    );

    git(&catalog, &["add", "-A"]);
    git(&catalog, &["commit", "-q", "-m", "catalog"]);
    let output = succeeds(fixture.run(&catalog, &["unit", "publish"]));
    assert!(
        stdout(&output).contains("Published the catalog @acme/web 1.0.0 as tag v1.0.0 at commit"),
        "{}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains("with 3 units: @acme/a, @acme/b, @acme/c."),
        "{}",
        stdout(&output)
    );
    // The catalog's name is not a unit the registry knows: its units are.
    assert!(
        stdout(&output).contains(
            "The local registry does not register the units of the catalog @acme/web, so"
        ),
        "{}",
        stdout(&output)
    );
    assert_eq!(git(&catalog, &["tag", "--list"]), "v1.0.0");
    assert_eq!(
        git(
            &catalog,
            &["tag", "--list", "--format=%(contents:subject)", "v1.0.0"]
        ),
        "@acme/web 1.0.0"
    );

    let reference = format!("file://{}", catalog.to_str().unwrap().replace('\\', "/"));
    let output = succeeds(fixture.run(&fixture.dir, &["registry", "add", &reference]));
    let out = stdout(&output);
    assert!(out.contains("with 3 units"), "{out}");

    succeeds(fixture.run(&fixture.dir, &["init", "app"]));
    let project = fixture.dir.join("app");
    write(
        &project,
        "assemblr.jsonc",
        r#"{"dependencies": {"@acme/b": "^1.0.0"}}"#,
    );
    succeeds(fixture.run(&project, &["compose"]));
    for file in ["a.txt", "b.txt", "base.txt"] {
        assert!(project.join(file).is_file(), "{file}");
    }
    assert!(!project.join("c.txt").exists());
}

#[test]
fn a_registered_catalog_indexes_each_new_version_and_its_new_units() {
    let fixture = Fixture::new("registered");
    let catalog = fixture.three_units();
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    git(&catalog, &["add", "-A"]);
    git(&catalog, &["commit", "-q", "-m", "1.0.0"]);
    succeeds(fixture.run(
        &fixture.dir,
        &["registry", "add", catalog.to_str().unwrap()],
    ));
    let output = succeeds(fixture.run(&catalog, &["unit", "publish"]));
    assert!(
        stdout(&output).contains("Indexed it in the local registry."),
        "{}",
        stdout(&output)
    );

    // The next version adds a unit, which the registry registers from the
    // catalog with the others.
    write(
        &catalog,
        "catalog.jsonc",
        r#"{"name": "@acme/web", "version": "1.1.0"}"#,
    );
    succeeds(fixture.run(&catalog, &["unit", "init", "@acme/d"]));
    write(&catalog, "units/d/src/d.txt", "d\n");
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    git(&catalog, &["add", "-A"]);
    git(&catalog, &["commit", "-q", "-m", "1.1.0"]);
    succeeds(fixture.run(&catalog, &["unit", "publish"]));
    let list = stdout(&succeeds(fixture.run(&fixture.dir, &["registry", "list"])));
    assert!(list.contains("@acme/d"), "{list}");
    assert!(list.contains("1.1.0"), "{list}");
}

#[test]
fn building_the_catalog_twice_changes_nothing() {
    let fixture = Fixture::new("deterministic");
    let catalog = fixture.three_units();
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    let lock = read(&catalog.join("assemblr-lock.json"));
    let output = succeeds(fixture.run(&catalog, &["unit", "build"]));
    assert_eq!(read(&catalog.join("assemblr-lock.json")), lock);
    assert!(
        !stderr(&output).contains("assemblr-lock.json"),
        "{}",
        stderr(&output)
    );
    // The units are built in the order of their directories.
    let out = stdout(&output);
    let positions: Vec<usize> = ["@acme/a", "@acme/b", "@acme/c"]
        .iter()
        .map(|unit| out.find(&format!("Built {unit} ")).unwrap())
        .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{out}");
}

/// Builds the catalog, which must fail, and returns its stderr.
fn build_error(fixture: &Fixture, catalog: &Path) -> String {
    stderr(&fails(fixture.run(catalog, &["unit", "build"])))
}

#[test]
fn a_unit_in_a_catalog_declares_no_version() {
    let fixture = Fixture::new("version");
    let catalog = fixture.three_units();
    write(
        &catalog,
        "units/c/manifest.jsonc",
        r#"{"name": "@acme/c", "version": "1.0.0", "description": "C."}"#,
    );
    let err = build_error(&fixture, &catalog);
    // The manifest is named by its file-system path, with the platform's
    // separators.
    let manifest = Path::new("units").join("c").join("manifest.jsonc");
    assert!(err.contains(&manifest.display().to_string()), "{err}");
    assert!(err.contains("has no version of its own"), "{err}");
}

#[test]
fn a_sibling_is_required_as_catalog() {
    let fixture = Fixture::new("range");
    let catalog = fixture.three_units();
    write(
        &catalog,
        "units/b/manifest.jsonc",
        &unit("b", r#""@acme/a": "^1.0.0""#),
    );
    let err = build_error(&fixture, &catalog);
    assert!(err.contains("require it as \"catalog\""), "{err}");
}

#[test]
fn catalog_requires_a_unit_of_the_same_catalog() {
    let fixture = Fixture::new("not-a-sibling");
    let catalog = fixture.three_units();
    write(
        &catalog,
        "units/b/manifest.jsonc",
        &unit("b", r#""@acme/base": "catalog""#),
    );
    let err = build_error(&fixture, &catalog);
    assert!(err.contains("the catalog has no such unit"), "{err}");
}

#[test]
fn catalog_outside_a_catalog_is_rejected() {
    let fixture = Fixture::new("outside");
    fixture.base();
    git(&fixture.dir, &["init", "-q", "alone"]);
    let alone = fixture.dir.join("alone");
    write(
        &alone,
        "manifest.jsonc",
        r#"{"name": "@acme/alone", "version": "1.0.0", "description": "Alone.",
            "dependencies": {"@acme/base": "catalog"}}"#,
    );
    let err = stderr(&fails(fixture.run(&alone, &["unit", "build"])));
    assert!(
        err.contains("only a unit in a catalog's `units/` has one"),
        "{err}"
    );
}

#[test]
fn a_unit_is_named_after_its_directory() {
    let fixture = Fixture::new("name");
    let catalog = fixture.three_units();
    write(
        &catalog,
        "units/c/manifest.jsonc",
        r#"{"name": "@acme/other", "description": "C."}"#,
    );
    let err = build_error(&fixture, &catalog);
    assert!(err.contains("this one is @acme/c"), "{err}");
    // `unit init` gives the same name or refuses.
    let err = stderr(&fails(
        fixture.run(&catalog, &["unit", "init", "@other/e", "units/e"]),
    ));
    assert!(err.contains("this directory's unit is @acme/e"), "{err}");
}

#[test]
fn every_entry_of_units_is_a_directory() {
    let fixture = Fixture::new("entry");
    let catalog = fixture.three_units();
    write(&catalog, "units/notes.txt", "not a unit\n");
    let err = build_error(&fixture, &catalog);
    assert!(err.contains("notes.txt is not a directory"), "{err}");
}

#[test]
fn a_catalog_has_units() {
    let fixture = Fixture::new("empty");
    fixture.base();
    let catalog = fixture.catalog("1.0.0");
    let err = build_error(&fixture, &catalog);
    assert!(err.contains("the catalog has no units"), "{err}");
}

#[test]
fn a_repository_is_a_catalog_or_one_unit() {
    let fixture = Fixture::new("both");
    let catalog = fixture.three_units();
    write(
        &catalog,
        "manifest.jsonc",
        r#"{"name": "@acme/web", "version": "1.0.0", "description": "Both."}"#,
    );
    let err = build_error(&fixture, &catalog);
    assert!(err.contains("is beside catalog.jsonc"), "{err}");
    // `unit init` never puts a manifest beside catalog.jsonc.
    let err = stderr(&fails(
        fixture.run(&catalog, &["unit", "init", "@acme/web", "."]),
    ));
    assert!(err.contains("is a catalog's top level"), "{err}");
}

#[test]
fn an_invalid_catalog_file_is_reported() {
    let fixture = Fixture::new("catalog-file");
    let catalog = fixture.three_units();
    write(&catalog, "catalog.jsonc", r#"{"name": "@acme/web"}"#);
    let err = build_error(&fixture, &catalog);
    assert!(err.contains("catalog.jsonc"), "{err}");
    assert!(err.contains("version"), "{err}");
}

#[test]
fn a_sibling_amended_at_a_version_is_refused() {
    let fixture = Fixture::new("sibling-version");
    let catalog = fixture.three_units();
    write(
        &catalog,
        "units/b/manifest.jsonc",
        &unit(
            "b",
            r#""@acme/a": "catalog"}, "amendments": {"a.txt": {"unit": "@acme/a", "base": "1.0.0"}"#,
        ),
    );
    let err = build_error(&fixture, &catalog);
    assert!(
        err.contains("the base of an amendment of its file is"),
        "{err}"
    );
    assert!(err.contains("\"catalog\""), "{err}");
    // And "catalog" is the base of a sibling's file only.
    write(
        &catalog,
        "units/b/manifest.jsonc",
        &unit(
            "b",
            r#""@acme/a": "catalog"}, "amendments": {"base.txt": {"unit": "@acme/base", "base": "catalog"}"#,
        ),
    );
    let err = build_error(&fixture, &catalog);
    assert!(err.contains("the catalog has no such unit"), "{err}");
}

impl Fixture {
    /// The acceptance catalog, built, with `a.txt` of `@acme/a` three lines
    /// long, and `b` amending it: its copy changes the last line.
    fn amending(&self) -> PathBuf {
        let catalog = self.three_units();
        write(&catalog, "units/a/src/a.txt", "one\ntwo\nthree\n");
        succeeds(self.run(&catalog, &["unit", "build"]));
        let output = succeeds(self.run(&catalog.join("units/b"), &["unit", "amend", "a.txt"]));
        assert!(
            stdout(&output).contains("Its base is stored in the catalog's .assemblr/"),
            "{}",
            stdout(&output)
        );
        write(
            &catalog,
            "units/b/amendments/a.txt",
            "one\ntwo\nthree, by b\n",
        );
        succeeds(self.run(&catalog, &["unit", "build"]));
        catalog
    }
}

/// Every file under `.assemblr/` and `amendments/` of the catalog's units,
/// with its content.
fn state(catalog: &Path) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    for root in [catalog.join(".assemblr"), catalog.join("units")] {
        for path in files(&root) {
            if root.ends_with("units") && !path.contains("/amendments/") {
                continue;
            }
            found.push((path.clone(), fs::read(root.join(&path)).unwrap()));
        }
    }
    found
}

#[test]
fn a_sibling_file_is_amended_with_a_stored_base() {
    let fixture = Fixture::new("sibling-amend");
    let catalog = fixture.amending();
    let manifest = read(&catalog.join("units/b/manifest.jsonc"));
    assert!(
        manifest.contains(r#""a.txt": { "unit": "@acme/a", "base": "catalog" }"#),
        "{manifest}"
    );
    assert!(catalog.join(".assemblr/amendment-bases.json").is_file());
    assert_eq!(
        read(&catalog.join("units/b/build/a.txt")),
        "one\ntwo\nthree, by b\n"
    );
    succeeds(fixture.run(&catalog, &["unit", "check"]));
    // At the top level, the command names no unit to amend for.
    let err = stderr(&fails(fixture.run(&catalog, &["unit", "amend", "a.txt"])));
    assert!(err.contains("catalog's top level"), "{err}");
}

#[test]
fn a_change_below_a_copy_flows_into_it() {
    let fixture = Fixture::new("sibling-rebase");
    let catalog = fixture.amending();
    write(&catalog, "units/a/src/a.txt", "one, by a\ntwo\nthree\n");
    let output = succeeds(fixture.run(&catalog, &["unit", "build"]));
    assert!(
        stderr(&output).contains("note: rebased units/b/amendments/a.txt onto the change"),
        "{}",
        stderr(&output)
    );
    let rebased = "one, by a\ntwo\nthree, by b\n";
    assert_eq!(read(&catalog.join("units/b/amendments/a.txt")), rebased);
    assert_eq!(read(&catalog.join("units/b/build/a.txt")), rebased);
    // The stored base follows: the next build has nothing to rebase.
    let output = succeeds(fixture.run(&catalog, &["unit", "build"]));
    assert!(!stderr(&output).contains("rebased"), "{}", stderr(&output));
    succeeds(fixture.run(&catalog, &["unit", "check"]));
}

#[test]
fn a_change_to_what_a_copy_changed_stops_the_build_with_markers() {
    let fixture = Fixture::new("sibling-conflict");
    let catalog = fixture.amending();
    write(&catalog, "units/a/src/a.txt", "one\ntwo\nthree, by a\n");
    let err = build_error(&fixture, &catalog);
    assert!(
        err.contains(
            "units/b/amendments/a.txt: src/a.txt of @acme/a, as the units below compose it, \
             changed"
        ),
        "{err}"
    );
    assert!(err.contains("between markers"), "{err}");
    let copy = read(&catalog.join("units/b/amendments/a.txt"));
    assert!(copy.contains("<<<<<<< current (your copy)"), "{copy}");
    assert!(copy.contains("||||||| base (stored)"), "{copy}");
    assert!(
        copy.contains(">>>>>>> incoming (the file below the copy now)"),
        "{copy}"
    );
    // Nothing is built until the markers are gone.
    let err = build_error(&fixture, &catalog);
    assert!(err.contains("conflict markers"), "{err}");
    write(
        &catalog,
        "units/b/amendments/a.txt",
        "one\ntwo\nthree, by a and b\n",
    );
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    assert_eq!(
        read(&catalog.join("units/b/build/a.txt")),
        "one\ntwo\nthree, by a and b\n"
    );
    succeeds(fixture.run(&catalog, &["unit", "check"]));
}

#[test]
fn check_and_publish_refuse_a_stale_base() {
    let fixture = Fixture::new("sibling-stale");
    let catalog = fixture.amending();
    write(&catalog, "units/a/src/a.txt", "one, by a\ntwo\nthree\n");
    let err = stderr(&fails(fixture.run(&catalog, &["unit", "check"])));
    assert!(err.contains("is not rebased onto the change yet"), "{err}");
    assert!(err.contains("which rebases the copy"), "{err}");
    let err = stderr(&fails(
        fixture.run(&catalog.join("units/b"), &["unit", "check"]),
    ));
    assert!(err.contains("is not rebased onto the change yet"), "{err}");
    git(&catalog, &["add", "-A"]);
    git(&catalog, &["commit", "-q", "-m", "stale"]);
    let err = stderr(&fails(fixture.run(&catalog, &["unit", "publish"])));
    assert!(err.contains("is not rebased onto the change yet"), "{err}");
    assert_eq!(git(&catalog, &["tag", "--list"]), "");
    // Built and committed, the catalog publishes, and its dependents
    // compose the copy as it was published.
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    git(&catalog, &["add", "-A"]);
    git(&catalog, &["commit", "-q", "-m", "rebased"]);
    succeeds(fixture.run(&catalog, &["unit", "publish"]));
    succeeds(fixture.run(
        &fixture.dir,
        &["registry", "add", catalog.to_str().unwrap()],
    ));
    let project = fixture.dir.join("project");
    fs::create_dir_all(&project).unwrap();
    succeeds(fixture.run(&project, &["init"]));
    succeeds(fixture.run(&project, &["add", "@acme/b"]));
    assert_eq!(
        read(&project.join("a.txt")),
        "one, by a\ntwo\nthree, by b\n"
    );
}

#[test]
fn a_corrupt_or_missing_store_fails_explicitly_and_changes_nothing() {
    let fixture = Fixture::new("sibling-corrupt");
    let catalog = fixture.amending();
    write(&catalog, "units/a/src/a.txt", "one, by a\ntwo\nthree\n");
    let objects = catalog.join(".assemblr/objects");
    let blob = files(&objects)
        .into_iter()
        .map(|path| objects.join(path))
        .next()
        .unwrap();
    fs::write(&blob, "tampered\n").unwrap();
    let before = state(&catalog);
    let err = build_error(&fixture, &catalog);
    assert!(err.contains("does not verify"), "{err}");
    assert!(
        err.contains("restore .assemblr/ from version control"),
        "{err}"
    );
    assert_eq!(state(&catalog), before);
    let err = stderr(&fails(fixture.run(&catalog, &["unit", "check"])));
    assert!(err.contains("does not verify"), "{err}");
    // Without the store, the copy's base is unknown.
    fs::remove_dir_all(catalog.join(".assemblr")).unwrap();
    let err = build_error(&fixture, &catalog);
    assert!(err.contains("has no stored base"), "{err}");
}

#[test]
fn a_failed_build_leaves_the_copies_and_the_store_as_they_were() {
    let fixture = Fixture::new("sibling-failed");
    let catalog = fixture.amending();
    write(&catalog, "units/a/src/a.txt", "one, by a\ntwo\nthree\n");
    // Another unit fails validation once the copy is rebased in memory.
    write(
        &catalog,
        "units/c/manifest.jsonc",
        r#"{"name": "@acme/c", "description": "The c unit.",
            "placeholders": {"replace": {"__UNUSED__": "Nowhere."}}}"#,
    );
    let before = state(&catalog);
    let err = build_error(&fixture, &catalog);
    assert!(err.contains("__UNUSED__"), "{err}");
    assert_eq!(state(&catalog), before);
}

#[test]
fn layered_copies_between_siblings_are_rebased_in_order() {
    let fixture = Fixture::new("sibling-layers");
    let catalog = fixture.amending();
    // `c` is above `b`, and amends the file `b` amends.
    write(
        &catalog,
        "units/c/manifest.jsonc",
        &unit("c", r#""@acme/b": "catalog""#),
    );
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    succeeds(fixture.run(&catalog.join("units/c"), &["unit", "amend", "a.txt"]));
    write(
        &catalog,
        "units/c/amendments/a.txt",
        "one\ntwo\nthree, by b\nfour, by c\n",
    );
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    write(&catalog, "units/a/src/a.txt", "one, by a\ntwo\nthree\n");
    let output = succeeds(fixture.run(&catalog.join("units/c"), &["unit", "build"]));
    let err = stderr(&output);
    assert!(err.contains("rebased units/b/amendments/a.txt"), "{err}");
    assert!(err.contains("rebased units/c/amendments/a.txt"), "{err}");
    assert_eq!(
        read(&catalog.join("units/c/build/a.txt")),
        "one, by a\ntwo\nthree, by b\nfour, by c\n"
    );
    succeeds(fixture.run(&catalog, &["unit", "check"]));
}

#[test]
fn a_conflict_between_siblings_creates_the_pending_state_in_a_catalog() {
    let fixture = Fixture::new("sibling-pending");
    let catalog = fixture.three_units();
    write(&catalog, "units/a/src/a.txt", "one\ntwo\nthree\n");
    write(
        &catalog,
        "units/c/manifest.jsonc",
        &unit("c", r#""@acme/a": "catalog""#),
    );
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    for (amender, copy) in [
        ("b", "one, by b\ntwo\nthree\n"),
        ("c", "one, by c\ntwo\nthree\n"),
    ] {
        succeeds(fixture.run(
            &catalog.join("units").join(amender),
            &["unit", "amend", "a.txt"],
        ));
        write(&catalog, &format!("units/{amender}/amendments/a.txt"), copy);
    }
    succeeds(fixture.run(&catalog, &["unit", "init", "@acme/d"]));
    write(
        &catalog,
        "units/d/manifest.jsonc",
        &unit("d", r#""@acme/b": "catalog", "@acme/c": "catalog""#),
    );
    write(&catalog, "units/d/src/d.txt", "d\n");
    let err = build_error(&fixture, &catalog);
    assert!(
        err.contains("run `assemblr unit build` in units/d"),
        "{err}"
    );
    let err = stderr(&fails(
        fixture.run(&catalog.join("units/d"), &["unit", "build"]),
    ));
    assert!(
        err.contains("the copy holds each conflict between markers"),
        "{err}"
    );
    let manifest = read(&catalog.join("units/d/manifest.jsonc"));
    assert!(
        manifest.contains(r#""a.txt": { "unit": "@acme/a", "base": "catalog" }"#),
        "{manifest}"
    );
    write(
        &catalog,
        "units/d/amendments/a.txt",
        "one, by b and c\ntwo\nthree\n",
    );
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    assert_eq!(
        read(&catalog.join("units/d/build/a.txt")),
        "one, by b and c\ntwo\nthree\n"
    );
    succeeds(fixture.run(&catalog, &["unit", "check"]));
}

#[test]
fn a_problem_in_one_unit_names_it() {
    let fixture = Fixture::new("in-unit");
    let catalog = fixture.three_units();
    // Nothing of `c` is composed: the publication gate reports it, by
    // unit.
    fs::remove_file(catalog.join("units/c/src/c.txt")).unwrap();
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    let err = stderr(&fails(fixture.run(&catalog, &["unit", "check"])));
    assert!(err.contains("units/c: "), "{err}");
}

#[test]
fn a_unit_of_a_catalog_is_published_with_the_catalog() {
    let fixture = Fixture::new("publish-unit");
    let catalog = fixture.three_units();
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    git(&catalog, &["add", "-A"]);
    git(&catalog, &["commit", "-q", "-m", "1.0.0"]);
    let err = stderr(&fails(
        fixture.run(&catalog.join("units/a"), &["unit", "publish"]),
    ));
    assert!(err.contains("whose units are published together"), "{err}");
    assert!(
        err.contains("hint: run `assemblr unit publish` in "),
        "{err}"
    );
    assert_eq!(git(&catalog, &["tag", "--list"]), "");
}

#[test]
fn publishing_needs_the_current_lockfile() {
    let fixture = Fixture::new("stale");
    let catalog = fixture.three_units();
    git(&catalog, &["add", "-A"]);
    git(&catalog, &["commit", "-q", "-m", "no lockfile"]);
    let err = stderr(&fails(fixture.run(&catalog, &["unit", "publish"])));
    assert!(err.contains("there is no assemblr-lock.json"), "{err}");
    // Reported once, for the whole catalog.
    assert_eq!(
        err.matches("there is no assemblr-lock.json").count(),
        1,
        "{err}"
    );
    assert_eq!(git(&catalog, &["tag", "--list"]), "");
}

#[test]
fn editing_commands_refuse_a_catalog() {
    let fixture = Fixture::new("refused");
    let catalog = fixture.three_units();
    for args in [&["unit", "add", "@acme/base"][..], &["unit", "update"]] {
        let err = stderr(&fails(fixture.run(&catalog.join("units/a"), args)));
        assert!(
            err.contains("does not work in a catalog yet"),
            "{args:?}: {err}"
        );
    }
}

#[test]
fn unit_init_in_a_catalog_writes_no_version_and_no_repository() {
    let fixture = Fixture::new("init");
    let catalog = fixture.catalog("1.0.0");
    let output = succeeds(fixture.run(&catalog, &["unit", "init", "@acme/a"]));
    assert!(
        stdout(&output).contains("kept the catalog's Git repository"),
        "{}",
        stdout(&output)
    );
    let manifest = read(&catalog.join("units/a/manifest.jsonc"));
    assert!(!manifest.contains("\"version\""), "{manifest}");
    assert!(!catalog.join("units/a/.git").exists());
    assert!(read(&catalog.join("units/a/.gitignore")).contains("/build/"));
}

#[test]
fn removing_a_unit_of_a_catalog_removes_every_unit_of_it() {
    let fixture = Fixture::new("remove");
    let catalog = fixture.three_units();
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    git(&catalog, &["add", "-A"]);
    git(&catalog, &["commit", "-q", "-m", "1.0.0"]);
    succeeds(fixture.run(&catalog, &["unit", "publish"]));
    succeeds(fixture.run(
        &fixture.dir,
        &["registry", "add", catalog.to_str().unwrap()],
    ));
    let output = succeeds(fixture.run(&fixture.dir, &["registry", "remove", "@acme/b"]));
    let out = stdout(&output);
    assert!(out.contains("@acme/a"), "{out}");
    assert!(out.contains("@acme/c"), "{out}");
    let list = stdout(&succeeds(fixture.run(&fixture.dir, &["registry", "list"])));
    assert!(!list.contains("@acme/a"), "{list}");
    assert!(list.contains("@acme/base"), "{list}");
}

#[cfg(unix)]
#[test]
fn units_behind_a_symbolic_link_are_refused() {
    let fixture = Fixture::new("units-link");
    let catalog = fixture.three_units();
    let outside = fixture.dir.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::rename(catalog.join("units"), outside.join("units")).unwrap();
    std::os::unix::fs::symlink(outside.join("units"), catalog.join("units")).unwrap();
    let err = build_error(&fixture, &catalog);
    assert!(err.contains("never behind a symbolic link"), "{err}");
    assert!(!outside.join("units/a/build").exists());
}

#[test]
fn a_directory_of_units_without_a_manifest_is_explained() {
    let fixture = Fixture::new("no-manifest");
    let catalog = fixture.three_units();
    fs::remove_file(catalog.join("units/c/manifest.jsonc")).unwrap();
    let err = build_error(&fixture, &catalog);
    assert!(err.contains("every directory of units/ is a unit"), "{err}");
}

#[test]
fn unit_init_at_a_catalog_says_where_the_unit_is() {
    let fixture = Fixture::new("init-next");
    let catalog = fixture.catalog("1.0.0");
    let out = stdout(&succeeds(
        fixture.run(&catalog, &["unit", "init", "@acme/a"]),
    ));
    assert!(out.contains("`cd units/a`"), "{out}");
    assert!(!out.contains("unit add"), "{out}");
}

#[test]
fn a_json_change_below_a_copy_is_merged_into_it() {
    let fixture = Fixture::new("sibling-json");
    let catalog = fixture.three_units();
    write(
        &catalog,
        "units/a/src/package.json",
        "{\n  \"name\": \"app\",\n  \"scripts\": {\n    \"build\": \"tsc\"\n  }\n}\n",
    );
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    succeeds(fixture.run(&catalog.join("units/b"), &["unit", "amend", "package.json"]));
    write(
        &catalog,
        "units/b/amendments/package.json",
        "{\n  \"name\": \"app\",\n  \"scripts\": {\n    \"build\": \"tsc\",\n    \"test\": \"vitest\"\n  }\n}\n",
    );
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    // `a` adds a script: the copy keeps it, and its own.
    write(
        &catalog,
        "units/a/src/package.json",
        "{\n  \"name\": \"app\",\n  \"scripts\": {\n    \"build\": \"tsc\",\n    \"lint\": \"eslint\"\n  }\n}\n",
    );
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    let built = read(&catalog.join("units/b/build/package.json"));
    for script in [
        "\"build\": \"tsc\"",
        "\"lint\": \"eslint\"",
        "\"test\": \"vitest\"",
    ] {
        assert!(built.contains(script), "{built}");
    }
    succeeds(fixture.run(&catalog, &["unit", "check"]));
}

#[cfg(unix)]
#[test]
fn a_rebased_copy_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("sibling-mode");
    let catalog = fixture.amending();
    let copy = catalog.join("units/b/amendments/a.txt");
    fs::set_permissions(&copy, fs::Permissions::from_mode(0o755)).unwrap();
    write(&catalog, "units/a/src/a.txt", "one, by a\ntwo\nthree\n");
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    assert_eq!(read(&copy), "one, by a\ntwo\nthree, by b\n");
    assert_eq!(
        fs::metadata(&copy).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn a_failed_store_write_puts_the_rebased_copies_back() {
    let fixture = Fixture::new("sibling-restore");
    let catalog = fixture.amending();
    write(&catalog, "units/a/src/a.txt", "one, by a\ntwo\nthree\n");
    // The record cannot be staged: a directory holds its staging name.
    fs::create_dir_all(catalog.join(".assemblr/amendment-bases.json.new/x")).unwrap();
    let record = read(&catalog.join(".assemblr/amendment-bases.json"));
    let err = build_error(&fixture, &catalog);
    assert!(
        err.contains("the copies and their stored bases are as they were"),
        "{err}"
    );
    assert_eq!(
        read(&catalog.join("units/b/amendments/a.txt")),
        "one\ntwo\nthree, by b\n"
    );
    assert_eq!(
        read(&catalog.join(".assemblr/amendment-bases.json")),
        record
    );
    assert!(
        !catalog
            .join("units/b/amendments/a.txt.assemblr-rebase-new")
            .exists()
    );
}

#[test]
fn a_catalog_is_not_a_consumer_project() {
    let fixture = Fixture::new("not-a-project");
    let catalog = fixture.catalog("1.0.0");
    succeeds(fixture.run(&catalog, &["unit", "init", "@acme/a"]));
    for args in [&["init"][..], &["compose"], &["status"]] {
        let err = stderr(&fails(fixture.run(&catalog, args)));
        assert!(
            err.contains(
                "holds `catalog.jsonc`, so it is a catalog of units, not a consumer project"
            ),
            "{args:?}: {err}"
        );
    }
    assert!(!catalog.join("assemblr.jsonc").exists());
    assert!(!catalog.join("AGENTS.md").exists());
}

#[test]
fn a_unit_is_not_created_inside_a_catalog_outside_its_units() {
    let fixture = Fixture::new("nested");
    let catalog = fixture.catalog("1.0.0");
    succeeds(fixture.run(&catalog, &["unit", "init", "@acme/a"]));
    let unit = catalog.join("units/a");
    let err = stderr(&fails(fixture.run(&unit, &["unit", "init", "@acme/x"])));
    assert!(
        err.contains("is inside the catalog at") && err.contains("outside its units/"),
        "{err}"
    );
    assert!(err.contains("run `assemblr unit init @acme/x` in"), "{err}");
    assert!(!unit.join("x").exists());
    // From the catalog's top level, the unit goes in `units/` as usual.
    succeeds(fixture.run(&catalog, &["unit", "init", "@acme/x"]));
    assert!(catalog.join("units/x/manifest.jsonc").is_file());
}

#[test]
fn a_provider_of_another_repository_moves_by_its_declared_base() {
    let fixture = Fixture::new("external-base");
    let base = fixture.base();
    let catalog = fixture.catalog("1.0.0");
    succeeds(fixture.run(&catalog, &["unit", "init", "@acme/a"]));
    write(
        &catalog,
        "units/a/manifest.jsonc",
        &unit("a", r#""@acme/base": "^1.0.0""#),
    );
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    succeeds(fixture.run(&catalog.join("units/a"), &["unit", "amend", "base.txt"]));
    write(&catalog, "units/a/amendments/base.txt", "base, by a\n");
    succeeds(fixture.run(&catalog, &["unit", "build"]));

    // The provider publishes 2.0.0, which adds a file.
    write(
        &base,
        "manifest.jsonc",
        r#"{"name": "@acme/base", "version": "2.0.0", "description": "A base."}"#,
    );
    write(&base, "src/more.txt", "more\n");
    git(&base, &["add", "-A"]);
    git(&base, &["commit", "-q", "-m", "2.0.0"]);
    git(&base, &["tag", "v2.0.0"]);
    succeeds(fixture.run(&fixture.dir, &["registry", "sync"]));

    // A requirement only 2.0.0 satisfies: `unit update` does not run in a
    // catalog, so the message says to move the declared base.
    let manifest = read(&catalog.join("units/a/manifest.jsonc"));
    write(
        &catalog,
        "units/a/manifest.jsonc",
        &manifest.replace("^1.0.0", "^2.0.0"),
    );
    let err = stderr(&fails(fixture.run(&catalog, &["unit", "build"])));
    assert!(
        err.contains(
            "declares @acme/base 1.0.0 as its base, but the catalog's units now resolve \
             @acme/base to 2.0.0; declare 2.0.0 as the base in manifest.jsonc"
        ),
        "{err}"
    );
    assert!(!err.contains("unit update"), "{err}");

    // Moving the base by hand: the build locks 2.0.0 on the copy.
    let manifest = read(&catalog.join("units/a/manifest.jsonc"));
    write(
        &catalog,
        "units/a/manifest.jsonc",
        &manifest.replace(r#""base": "1.0.0""#, r#""base": "2.0.0""#),
    );
    let output = succeeds(fixture.run(&catalog, &["unit", "build"]));
    assert!(
        stderr(&output).contains("@acme/base 1.0.0 -> 2.0.0"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        read(&catalog.join("units/a/build/base.txt")),
        "base, by a\n"
    );
    assert_eq!(read(&catalog.join("units/a/build/more.txt")), "more\n");
    succeeds(fixture.run(&catalog, &["unit", "check"]));
}

#[test]
fn a_project_may_hold_its_own_catalog_jsonc() {
    let fixture = Fixture::new("project-catalog-file");
    succeeds(fixture.run(&fixture.dir, &["init", "app"]));
    let project = fixture.dir.join("app");
    write(&project, "catalog.jsonc", "{}");
    write(&project, "units/notes.txt", "a project's own files\n");
    succeeds(fixture.run(&project, &["compose"]));
    succeeds(fixture.run(&project, &["status"]));
}

#[test]
fn a_catalog_unit_s_content_identity_is_the_root_tree() {
    let fixture = Fixture::new("root-tree");
    let catalog = fixture.three_units();
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    git(&catalog, &["add", "-A"]);
    git(&catalog, &["commit", "-q", "-m", "1.0.0"]);
    succeeds(fixture.run(&catalog, &["unit", "publish"]));
    succeeds(fixture.run(
        &fixture.dir,
        &["registry", "add", catalog.to_str().unwrap()],
    ));
    // The root tree holds the lockfile and the stored bases, which a
    // dependent's composition reads, so it is every unit's identity.
    let root = git(&catalog, &["rev-parse", "v1.0.0^{tree}"]);
    let output = succeeds(fixture.run(&fixture.dir, &["registry", "list", "--json"]));
    let list: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let units = list["units"].as_array().unwrap();
    for name in ["@acme/a", "@acme/b", "@acme/c"] {
        let unit = units.iter().find(|unit| unit["identity"] == name).unwrap();
        assert_eq!(unit["versions"][0]["tree"], root.as_str(), "{name}");
    }
}

#[test]
fn a_catalog_build_declares_the_oldest_assemblr_in_catalog_jsonc_and_publish_requires_it() {
    let fixture = Fixture::new("assemblr-field");
    let catalog = fixture.three_units();
    let output = succeeds(fixture.run(&catalog, &["unit", "build"]));
    assert_eq!(
        stderr(&output)
            .matches("note: set `assemblr` to 0.1.0-alpha.1 in catalog.jsonc")
            .count(),
        1,
        "{}",
        stderr(&output)
    );
    assert!(
        read(&catalog.join("catalog.jsonc")).contains(r#""assemblr": "0.1.0-alpha.1""#),
        "{}",
        read(&catalog.join("catalog.jsonc"))
    );
    // The units' manifests leave it to the catalog.
    assert!(!read(&catalog.join("units/a/manifest.jsonc")).contains("assemblr\""));

    // A commit without it is not published.
    write(
        &catalog,
        "catalog.jsonc",
        r#"{"name": "@acme/web", "version": "1.0.0"}"#,
    );
    git(&catalog, &["add", "-A"]);
    git(&catalog, &["commit", "-q", "-m", "undeclared"]);
    let err = stderr(&fails(fixture.run(&catalog, &["unit", "publish"])));
    assert!(
        err.contains("the committed catalog.jsonc does not declare `assemblr`"),
        "{err}"
    );
    assert_eq!(git(&catalog, &["tag", "--list"]), "");
    succeeds(fixture.run(&catalog, &["unit", "build"]));
    git(&catalog, &["add", "-A"]);
    git(&catalog, &["commit", "-q", "-m", "declared"]);
    succeeds(fixture.run(&catalog, &["unit", "publish"]));
    assert_eq!(git(&catalog, &["tag", "--list"]), "v1.0.0");
}
