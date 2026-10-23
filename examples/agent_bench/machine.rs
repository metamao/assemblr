//! The isolated directories a run uses, and the commands that run in them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Variables passed through to every command, so that Claude Code can reach
/// its API. Everything else in the caller's environment is dropped.
const PASSTHROUGH: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
    // Windows cannot start processes without these.
    "COMSPEC",
    "PATHEXT",
    "SYSTEMDRIVE",
    "SYSTEMROOT",
    "WINDIR",
];

/// The variables that authenticate Claude Code without the caller's home.
pub(crate) const AUTHENTICATION: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_OAUTH_TOKEN",
];

const GITCONFIG: &str = "\
[user]
\tname = Dev
\temail = dev@example.com
[init]
\tdefaultBranch = main
[core]
\tautocrlf = false
[commit]
\tgpgSign = false
[tag]
\tgpgSign = false
";

/// Copies `binary` into `dir`, which then holds nothing else, for the
/// runs' `PATH`.
///
/// Every run shares the copy: copying an executable while another thread
/// starts a process can leave the copy busy, and unable to run, on Linux.
pub(crate) fn install(binary: &Path, dir: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let name = binary.file_name().ok_or("the binary has no file name")?;
    fs::copy(binary, dir.join(name))
        .map_err(|error| format!("cannot copy {}: {error}", binary.display()))?;
    Ok(dir.to_owned())
}

/// One run's directory, outside every Git repository: `units` holds the
/// unit repositories, and each machine a home of its own. `bin` holds the
/// binary under test.
pub(crate) struct Root {
    pub(crate) path: PathBuf,
    bin: PathBuf,
}

impl Root {
    /// Creates `path` from scratch, for commands that find `assemblr` in
    /// `bin` (see [`install`]).
    pub(crate) fn create(path: PathBuf, bin: &Path) -> Result<Self, String> {
        if path.exists() {
            fs::remove_dir_all(&path)
                .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
        }
        for dir in [&path.join("units"), &path.join("tmp")] {
            fs::create_dir_all(dir)
                .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        }
        Ok(Self {
            path,
            bin: bin.to_owned(),
        })
    }

    /// The machine the agent works on.
    pub(crate) fn agent(&self) -> Result<Machine<'_>, String> {
        self.machine("home")
    }

    /// A machine with its own home, and so its own registry and Git
    /// identity, such as the one that published the units.
    pub(crate) fn machine(&self, name: &str) -> Result<Machine<'_>, String> {
        let home = self.path.join(name);
        let work = home.join("work");
        fs::create_dir_all(&work)
            .map_err(|error| format!("cannot create {}: {error}", work.display()))?;
        let gitconfig = home.join(".gitconfig");
        if !gitconfig.exists() {
            fs::write(&gitconfig, GITCONFIG)
                .map_err(|error| format!("cannot write {}: {error}", gitconfig.display()))?;
        }
        Ok(Machine { root: self, home })
    }
}

/// A home directory and the environment that every command on it gets.
pub(crate) struct Machine<'a> {
    pub(crate) root: &'a Root,
    pub(crate) home: PathBuf,
}

impl Machine<'_> {
    /// `program` in `dir` with only this machine's environment.
    pub(crate) fn command(&self, program: &str, dir: &Path) -> Result<Command, String> {
        let path = std::env::join_paths(
            std::iter::once(self.root.bin.clone()).chain(
                std::env::var_os("PATH")
                    .map_or_else(Vec::new, |path| std::env::split_paths(&path).collect()),
            ),
        )
        .map_err(|error| format!("cannot extend PATH: {error}"))?;
        let tmp = self.root.path.join("tmp");
        let mut command = Command::new(program);
        command.current_dir(dir).env_clear();
        for name in PASSTHROUGH {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("HOME", &self.home)
            .env("PATH", path)
            .env("TERM", "dumb")
            .env("LANG", "C.UTF-8")
            // Set even though it is where Linux puts the registry anyway,
            // because elsewhere the registry's place does not follow HOME.
            .env("ASSEMBLR_DATA_DIR", self.home.join(".local/share/assemblr"))
            .env("TMPDIR", &tmp)
            .env("TMP", &tmp)
            .env("TEMP", &tmp)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CEILING_DIRECTORIES", &self.root.path);
        Ok(command)
    }

    /// Runs `program` and returns its standard output, or what went wrong.
    pub(crate) fn run(&self, dir: &Path, program: &str, args: &[&str]) -> Result<String, String> {
        let output = self
            .command(program, dir)?
            .args(args)
            .output()
            .map_err(|error| format!("cannot run {program}: {error}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(format!(
                "`{program} {}` in {} failed: {}{}",
                args.join(" "),
                dir.display(),
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            ))
        }
    }

    pub(crate) fn assemblr(&self, dir: &Path, args: &[&str]) -> Result<String, String> {
        self.run(dir, "assemblr", args)
    }

    pub(crate) fn git(&self, dir: &Path, args: &[&str]) -> Result<String, String> {
        self.run(dir, "git", args)
    }

    /// The directory the agent starts in for most scenarios.
    pub(crate) fn work(&self) -> PathBuf {
        self.home.join("work")
    }
}

/// `path` as an argument, for commands that take one.
pub(crate) fn arg(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("{} is not valid UTF-8", path.display()))
}

/// The nearest directory at or above `dir` that holds a `.git` entry.
pub(crate) fn enclosing_repository(dir: &Path) -> Option<&Path> {
    dir.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
}

/// Copies the files under `from` into `to`, replacing those that exist.
pub(crate) fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(from)
        .map_err(|error| format!("cannot read {}: {error}", from.display()))?
        .collect::<Result<_, _>>()
        .map_err(|error| format!("cannot read {}: {error}", from.display()))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    fs::create_dir_all(to).map_err(|error| format!("cannot create {}: {error}", to.display()))?;
    for entry in entries {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .map_err(|error| format!("cannot copy {}: {error}", entry.path().display()))?;
        }
    }
    Ok(())
}
