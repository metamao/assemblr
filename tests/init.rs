//! `assemblr unit init`, run as a real process in temporary directories, and
//! the authoring loop from `unit init` to a dependent's build.
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

/// A test's directories: unit repositories, registries, and a home.
struct Fixture {
    dir: PathBuf,
    data: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new(test: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("init")
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

    /// Runs `assemblr` in `dir` against the registry at `data`.
    fn assemblr_with(&self, data: &Path, dir: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_assemblr"))
            .current_dir(dir)
            .env("ASSEMBLR_DATA_DIR", data)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
            .env("GIT_AUTHOR_NAME", "Author")
            .env("GIT_AUTHOR_EMAIL", "author@example.com")
            .env("GIT_COMMITTER_NAME", "Author")
            .env("GIT_COMMITTER_EMAIL", "author@example.com")
            .args(args)
            .output()
            .unwrap()
    }

    fn assemblr(&self, dir: &Path, args: &[&str]) -> Output {
        self.assemblr_with(&self.data, dir, args)
    }

    fn init(&self, dir: &Path, args: &[&str]) -> Output {
        let mut all = vec!["unit", "init"];
        all.extend_from_slice(args);
        self.assemblr(dir, &all)
    }
}

/// `git` in `dir`, isolated from the developer's configuration.
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

/// The entries of `dir`, sorted, `.git` excepted.
fn entries(dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != ".git")
        .collect();
    found.sort();
    found
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn init_creates_the_scaffold_and_a_repository_in_a_new_directory() {
    let fixture = Fixture::new("fresh");
    let unit = fixture.dir.join("units").join("app");

    let output = succeeds(fixture.init(&fixture.dir, &["@acme/app", "units/app"]));
    let first = stdout(&output).lines().next().unwrap().to_owned();
    assert!(
        first.starts_with(&format!(
            "Initialized @acme/app in {}: created the directory, a Git repository",
            unit.display()
        )),
        "{first}"
    );
    assert!(
        first.ends_with(", manifest.jsonc, manifest.schema.json, .gitignore."),
        "{first}"
    );
    assert!(stdout(&output).contains("Next: add files under src/"));
    assert_eq!(
        entries(&unit),
        [".gitignore", "manifest.jsonc", "manifest.schema.json"]
    );
    assert_eq!(
        read(&unit.join("manifest.jsonc")),
        "{\n  \"$schema\": \"./manifest.schema.json\",\n  \"name\": \"@acme/app\",\n  \
         \"version\": \"0.1.0\",\n  \"description\": \"The @acme/app unit.\"\n}\n"
    );
    assert_eq!(read(&unit.join(".gitignore")), "/build/\n");
    let schema: Value = serde_json::from_str(&read(&unit.join("manifest.schema.json"))).unwrap();
    assert_eq!(schema["title"], "Assemblr unit manifest");
    assert!(unit.join(".git").is_dir());
    assert!(git(&unit, &["status", "--porcelain"]).contains("?? manifest.jsonc"));
    // No commit was made.
    assert_eq!(git(&unit, &["rev-parse", "--is-inside-work-tree"]), "true");
    assert!(git(&unit, &["log", "--oneline", "--all"]).is_empty());

    // The scaffold checks and builds once it has a file.
    fs::create_dir(unit.join("src")).unwrap();
    fs::write(unit.join("src/app.txt"), "app").unwrap();
    succeeds(fixture.assemblr(&unit, &["unit", "build"]));
    let output = succeeds(fixture.assemblr(&unit, &["unit", "check"]));
    assert!(stdout(&output).contains("Checked @acme/app 0.1.0: no problems"));
}

#[test]
fn init_takes_a_description_and_defaults_to_the_current_directory() {
    let fixture = Fixture::new("description");
    let unit = fixture.dir.join("app");
    fs::create_dir(&unit).unwrap();

    succeeds(fixture.init(&unit, &["@acme/app", "--description", "An \"app\"."]));
    let manifest = read(&unit.join("manifest.jsonc"));
    assert!(
        manifest.contains(r#""description": "An \"app\".""#),
        "{manifest}"
    );
    assert!(unit.join(".git").is_dir());

    let err = stderr(&fails(fixture.init(
        &fixture.dir,
        &["@acme/blank", "blank", "--description", "  "],
    )));
    assert!(
        err.contains("must not be empty or only whitespace"),
        "{err}"
    );
    assert!(!fixture.dir.join("blank").exists());
}

#[test]
fn init_adopts_the_top_level_of_an_existing_repository() {
    let fixture = Fixture::new("adopt");
    let unit = fixture.dir.join("existing");
    fs::create_dir(&unit).unwrap();
    git(&unit, &["init", "-q"]);
    fs::write(unit.join("README.md"), "readme").unwrap();
    git(&unit, &["add", "-A"]);
    git(&unit, &["commit", "-q", "-m", "readme"]);
    fs::write(unit.join(".gitignore"), "*.log").unwrap();

    let output = succeeds(fixture.init(&unit, &["@acme/existing"]));
    assert!(
        stdout(&output).contains(
            "created manifest.jsonc, manifest.schema.json; replaced .gitignore (/build/ added); \
             kept the Git repository."
        ),
        "{}",
        stdout(&output)
    );
    assert_eq!(read(&unit.join(".gitignore")), "*.log\n/build/\n");
    assert_eq!(git(&unit, &["log", "--oneline"]).lines().count(), 1);

    // Inside a repository that is not a unit's, the unit gets its own
    // nested repository, and the note says so.
    let plain = fixture.dir.join("plain");
    fs::create_dir(&plain).unwrap();
    git(&plain, &["init", "-q"]);
    let output = succeeds(fixture.init(&plain, &["@acme/inner", "tools/inner"]));
    assert!(
        stdout(&output).contains(&format!(
            "created the directory, a Git repository (nested in the repository at {}), \
             manifest.jsonc",
            git(&plain, &["rev-parse", "--show-toplevel"])
        )),
        "{}",
        stdout(&output)
    );
    assert!(plain.join("tools/inner/.git").is_dir());
    assert!(git(&plain, &["status", "--porcelain"]).contains("?? tools/"));
}

#[test]
fn init_again_completes_the_scaffold_and_changes_nothing_else() {
    let fixture = Fixture::new("again");
    let unit = fixture.dir.join("app");
    succeeds(fixture.init(&fixture.dir, &["@acme/app", "app"]));
    // The author edits the manifest and loses the schema, and the ignore
    // file keeps `build/` out under another spelling.
    let edited = "{\n  // Mine.\n  \"name\": \"@acme/app\",\n  \"version\": \"0.2.0\",\n  \
                  \"description\": \"Edited.\"\n}\n";
    fs::write(unit.join("manifest.jsonc"), edited).unwrap();
    fs::remove_file(unit.join("manifest.schema.json")).unwrap();
    fs::write(unit.join(".gitignore"), "build\r\n*.log\r\n").unwrap();

    let output = succeeds(fixture.init(&unit, &["@acme/app"]));
    assert_eq!(
        stdout(&output).lines().next().unwrap(),
        format!(
            "Completed the scaffold of @acme/app in {}: created manifest.schema.json; already \
             in place: the Git repository, manifest.jsonc, .gitignore.",
            unit.display()
        )
    );
    assert_eq!(read(&unit.join("manifest.jsonc")), edited);
    assert_eq!(read(&unit.join(".gitignore")), "build\r\n*.log\r\n");

    // A schema from another version of Assemblr is refreshed; nothing else
    // changes, and running it once more changes nothing at all.
    fs::write(unit.join("manifest.schema.json"), "{}").unwrap();
    let output = succeeds(fixture.init(&unit, &["@acme/app"]));
    assert!(
        stdout(&output).contains("replaced manifest.schema.json (refreshed)"),
        "{}",
        stdout(&output)
    );
    let schema = read(&unit.join("manifest.schema.json"));
    assert!(schema.contains("Assemblr unit manifest"));
    let output = succeeds(fixture.init(&unit, &["@acme/app"]));
    assert!(
        stdout(&output).contains(
            "already in place: the Git repository, manifest.jsonc, manifest.schema.json, \
             .gitignore."
        ),
        "{}",
        stdout(&output)
    );
    assert_eq!(read(&unit.join("manifest.schema.json")), schema);
    assert_eq!(
        entries(&unit),
        [".gitignore", "manifest.jsonc", "manifest.schema.json"]
    );
}

#[test]
fn init_refuses_without_changing_anything() {
    let fixture = Fixture::new("refusals");

    // An invalid identity never reaches the file system.
    let output = fixture.init(&fixture.dir, &["acme/app", "bad"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("invalid unit identity"),
        "{}",
        stderr(&output)
    );
    assert!(!fixture.dir.join("bad").exists());

    // A directory inside a unit's repository, below its top level.
    let outer = fixture.dir.join("outer");
    succeeds(fixture.init(&fixture.dir, &["@acme/outer", "outer"]));
    fs::create_dir_all(outer.join("nested")).unwrap();
    let err = stderr(&fails(fixture.init(&outer, &["@acme/nested", "nested"])));
    assert!(err.contains("is inside the unit repository at"), "{err}");
    assert!(err.contains("a repository holds exactly one unit"), "{err}");
    assert_eq!(entries(&outer.join("nested")), Vec::<String>::new());
    // Also when the directory does not exist yet.
    let err = stderr(&fails(fixture.init(&outer, &["@acme/deeper", "a/b/c"])));
    assert!(err.contains("is inside the unit repository at"), "{err}");
    assert!(!outer.join("a").exists());

    // A bare repository.
    let bare = fixture.dir.join("bare");
    fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare"]);
    let err = stderr(&fails(fixture.init(&bare, &["@acme/bare"])));
    assert!(err.contains("is in a bare Git repository"), "{err}");
    assert!(!bare.join("manifest.jsonc").exists());

    // A manifest of another identity, and an unreadable one.
    let other = fixture.dir.join("other");
    succeeds(fixture.init(&fixture.dir, &["@acme/other", "other"]));
    let before = read(&other.join("manifest.jsonc"));
    let err = stderr(&fails(fixture.init(&other, &["@acme/app"])));
    assert!(err.contains("already declares @acme/other"), "{err}");
    assert!(
        err.contains("run `assemblr unit init @acme/other`"),
        "{err}"
    );
    assert_eq!(read(&other.join("manifest.jsonc")), before);
    fs::write(other.join("manifest.jsonc"), "{").unwrap();
    let err = stderr(&fails(fixture.init(&other, &["@acme/other"])));
    assert!(
        err.contains("is not a valid manifest, so it was left as it is"),
        "{err}"
    );
    assert_eq!(read(&other.join("manifest.jsonc")), "{");

    // A path that is a file, and a scaffold file that is a directory.
    fs::write(fixture.dir.join("file"), "").unwrap();
    let err = stderr(&fails(fixture.init(&fixture.dir, &["@acme/file", "file"])));
    assert!(err.contains("exists and is not a directory"), "{err}");
    let odd = fixture.dir.join("odd");
    fs::create_dir_all(odd.join(".gitignore")).unwrap();
    let err = stderr(&fails(fixture.init(&odd, &["@acme/odd"])));
    assert!(err.contains(".gitignore is not a regular file"), "{err}");
    assert!(!odd.join("manifest.jsonc").exists());
    assert!(!odd.join(".git").exists());
}

#[test]
fn the_loop_runs_from_init_to_a_dependent_s_build_through_a_second_registry() {
    let fixture = Fixture::new("loop");
    let other_data = fixture.dir.join("other-data");
    fs::create_dir_all(&other_data).unwrap();

    // A base unit, initialized, built, committed, and published.
    let node = fixture.dir.join("node");
    succeeds(fixture.init(&fixture.dir, &["@acme/node", "node"]));
    fs::create_dir(node.join("src")).unwrap();
    fs::write(node.join("src/node.txt.assemblr.tmpl"), "port {{ port }}\n").unwrap();
    let manifest = read(&node.join("manifest.jsonc")).replace(
        "\"description\": \"The @acme/node unit.\"",
        "\"description\": \"Node.\",\n  \"params\": {\"port\": {\"type\": \"integer\", \
         \"description\": \"The port.\", \"default\": 3000}}",
    );
    fs::write(node.join("manifest.jsonc"), manifest).unwrap();
    succeeds(fixture.assemblr(&node, &["unit", "build"]));
    git(&node, &["add", "-A"]);
    git(&node, &["commit", "-q", "-m", "node"]);
    let output = succeeds(fixture.assemblr(&node, &["unit", "publish"]));
    assert!(stdout(&output).contains("Published @acme/node 0.1.0 as tag v0.1.0"));

    // An amending unit on top of it, published the same way.
    let app = fixture.dir.join("app");
    succeeds(fixture.init(&fixture.dir, &["@acme/app", "app"]));
    succeeds(fixture.assemblr(&fixture.dir, &["registry", "add", node.to_str().unwrap()]));
    let manifest = read(&app.join("manifest.jsonc")).replace(
        "\"description\": \"The @acme/app unit.\"",
        "\"description\": \"App.\",\n  \"dependencies\": {\"@acme/node\": \"^0.1\"},\n  \
         \"options\": {\"@acme/node\": {\"port\": 8080}}",
    );
    fs::write(app.join("manifest.jsonc"), manifest).unwrap();
    fs::create_dir(app.join("src")).unwrap();
    fs::write(app.join("src/app.txt"), "app").unwrap();
    succeeds(fixture.assemblr(&app, &["unit", "build"]));
    succeeds(fixture.assemblr(&app, &["unit", "amend", "node.txt"]));
    fs::write(
        app.join("amendments/node.txt.assemblr.tmpl"),
        "port {{ port }}, amended\n",
    )
    .unwrap();
    succeeds(fixture.assemblr(&app, &["unit", "build"]));
    succeeds(fixture.assemblr(&app, &["unit", "check"]));
    git(&app, &["add", "-A"]);
    git(&app, &["commit", "-q", "-m", "app"]);
    succeeds(fixture.assemblr(&app, &["unit", "publish"]));
    assert_eq!(git(&app, &["tag"]), "v0.1.0");

    // A second registry learns both through registration, and a dependent
    // builds against the published versions, lockfile verified.
    let registry = |args: &[&str]| fixture.assemblr_with(&other_data, &fixture.dir, args);
    succeeds(registry(&["registry", "add", node.to_str().unwrap()]));
    let output = succeeds(registry(&["registry", "add", app.to_str().unwrap()]));
    assert!(stdout(&output).contains("with published versions 0.1.0"));
    let site = fixture.dir.join("site");
    succeeds(fixture.assemblr_with(
        &other_data,
        &fixture.dir,
        &["unit", "init", "@acme/site", "site"],
    ));
    let manifest = read(&site.join("manifest.jsonc")).replace(
        "\"description\": \"The @acme/site unit.\"",
        "\"description\": \"Site.\",\n  \"dependencies\": {\"@acme/app\": \"^0.1\"}",
    );
    fs::write(site.join("manifest.jsonc"), manifest).unwrap();
    let output = succeeds(fixture.assemblr_with(&other_data, &site, &["unit", "build"]));
    assert!(
        stdout(&output).contains("with 2 dependencies"),
        "{}",
        stdout(&output)
    );
    assert_eq!(read(&site.join("build/node.txt")), "port 8080, amended\n");
    assert_eq!(read(&site.join("build/app.txt")), "app");
}
