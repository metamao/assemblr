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
    assert!(err.contains("units/c/manifest.jsonc"), "{err}");
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
fn a_unit_cannot_amend_a_sibling_yet() {
    let fixture = Fixture::new("sibling-amendment");
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
        err.contains("cannot amend a file of another unit of the same catalog"),
        "{err}"
    );
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
    for args in [
        &["unit", "add", "@acme/base"][..],
        &["unit", "amend", "base.txt"],
        &["unit", "update"],
    ] {
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
