//! Runs a command against a development build of `assemblr` in an isolated
//! environment, for validating the CLI by hand. See ADR 0028.
//!
//! ```text
//! cargo sandbox [--name <name>] [--reset] [-- <program> [<args>...]]
//! ```
//!
//! A sandbox is a directory under the system's temporary directory, outside
//! every Git repository, that keeps its state between runs: `data` holds the
//! registry, `work` is where the command starts, `tmp` its temporary
//! directory, and `gitconfig` its global Git configuration, with an identity
//! and no signing. Every run builds the binary, puts it first on `PATH`,
//! removes the caller's `GIT_*` variables, and sets the rest of the
//! environment again, so no step depends on what an earlier shell set. The
//! sandbox's own state persists, so a scenario can be run one command at a
//! time.
//!
//! Without a command, it prints where the sandbox is and what it sets.
//! `--reset` removes the sandbox's state first. `--name` selects another
//! sandbox, for example one per scenario; the default is `default`.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: cargo sandbox [--name <name>] [--reset] [-- <program> [<args>...]]";

const GITCONFIG: &str = "\
[user]
\tname = Sandbox
\temail = sandbox@example.com
[init]
\tdefaultBranch = main
[core]
\tautocrlf = false
[commit]
\tgpgSign = false
[tag]
\tgpgSign = false
";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("sandbox: {message}");
            ExitCode::FAILURE
        }
    }
}

struct Options {
    name: String,
    reset: bool,
    command: Vec<OsString>,
}

fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Options, String> {
    let mut options = Options {
        name: "default".to_owned(),
        reset: false,
        command: Vec::new(),
    };
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--name") => {
                let name = args.next().and_then(|name| name.into_string().ok());
                options.name = name.ok_or(USAGE)?;
            }
            Some("--reset") => options.reset = true,
            Some("--") => {
                options.command = args.collect();
                break;
            }
            Some("-h" | "--help") => return Err(USAGE.to_owned()),
            _ => {
                options.command = std::iter::once(arg).chain(args).collect();
                break;
            }
        }
    }
    // The name becomes a directory that `--reset` removes, so it must not
    // reach outside the sandboxes' directory.
    let valid = !options.name.is_empty()
        && options
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return Err(format!(
            "the name {:?} must be letters, digits, `-`, or `_`",
            options.name
        ));
    }
    Ok(options)
}

fn run() -> Result<ExitCode, String> {
    let options = parse(std::env::args_os().skip(1))?;
    let root = std::env::temp_dir()
        .join("assemblr-sandbox")
        .join(&options.name);
    if let Some(repository) = enclosing_repository(&root) {
        return Err(format!(
            "{} is inside the Git repository {}; set TMPDIR (TEMP on Windows) \
             to a directory outside any repository",
            root.display(),
            repository.display()
        ));
    }
    if options.reset && root.exists() {
        fs::remove_dir_all(&root)
            .map_err(|error| format!("cannot remove {}: {error}", root.display()))?;
    }
    for dir in ["data", "work", "tmp"] {
        let dir = root.join(dir);
        fs::create_dir_all(&dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    }
    let gitconfig = root.join("gitconfig");
    if !gitconfig.exists() {
        fs::write(&gitconfig, GITCONFIG)
            .map_err(|error| format!("cannot write {}: {error}", gitconfig.display()))?;
    }

    let bin = build()?;
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(
        std::env::var_os("PATH").map_or_else(Vec::new, |p| std::env::split_paths(&p).collect()),
    ))
    .map_err(|error| format!("cannot extend PATH: {error}"))?;
    let data = root.join("data");
    let tmp = root.join("tmp");
    let environment: Vec<(&str, &OsStr)> = vec![
        ("ASSEMBLR_SANDBOX", root.as_os_str()),
        ("ASSEMBLR_DATA_DIR", data.as_os_str()),
        ("GIT_CONFIG_NOSYSTEM", OsStr::new("1")),
        ("GIT_CONFIG_GLOBAL", gitconfig.as_os_str()),
        // Plain `git` run in the sandbox never finds a repository above it.
        ("GIT_CEILING_DIRECTORIES", root.as_os_str()),
        ("TMPDIR", tmp.as_os_str()),
        ("TMP", tmp.as_os_str()),
        ("TEMP", tmp.as_os_str()),
        ("PATH", path.as_os_str()),
    ];

    let Some((program, args)) = options.command.split_first() else {
        println!("sandbox {}: {}", options.name, root.display());
        println!("binary: {}", bin.display());
        println!("starts in: {}", root.join("work").display());
        for (name, value) in &environment {
            if *name != "PATH" {
                println!("{name}={}", value.to_string_lossy());
            }
        }
        println!("removes every inherited GIT_* variable; PATH starts with the binary's directory");
        return Ok(ExitCode::SUCCESS);
    };
    let mut command = Command::new(program);
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    let status = command
        .args(args)
        .current_dir(root.join("work"))
        .envs(environment)
        .status()
        .map_err(|error| format!("cannot run {}: {error}", program.to_string_lossy()))?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from))
}

/// Builds the `assemblr` binary of this checkout and returns its directory.
/// This example is built in `examples/` next to it, in the same profile.
fn build() -> Result<PathBuf, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--quiet", "--bin", "assemblr"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .map_err(|error| format!("cannot run cargo: {error}"))?;
    if !status.success() {
        return Err("building assemblr failed".to_owned());
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let bin = exe
        .parent()
        .and_then(Path::parent)
        .ok_or("cannot locate the build directory")?
        .to_owned();
    let binary = bin.join(format!("assemblr{}", std::env::consts::EXE_SUFFIX));
    if !binary.is_file() {
        return Err(format!("no binary at {}", binary.display()));
    }
    Ok(bin)
}

/// The nearest directory at or above `dir` that holds a `.git` entry.
fn enclosing_repository(dir: &Path) -> Option<&Path> {
    dir.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
}
