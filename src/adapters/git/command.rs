//! Running `git` against a repository that is not trusted.
//!
//! Every command runs with an environment Assemblr sets, not one inherited
//! from the caller, and reads only the object database. See ADR 0006 for why
//! each setting is needed.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};

use super::{GitError, GitErrorKind, GitRepository};
use crate::domain::ObjectId;

#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

const ENVIRONMENT: [(&str, &str); 6] = [
    // Ignore the user's and the system's configuration, so a read does not
    // depend on the machine. The repository's own configuration still
    // applies, which is why the arguments below override parts of it.
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_CONFIG_GLOBAL", NULL_DEVICE),
    // Never fetch a missing object, as a partial clone would: that reaches
    // the network and runs the transport the repository configures. Git
    // before 2.45 ignores the first setting, and the second, which allows
    // no transport at all, covers it.
    ("GIT_NO_LAZY_FETCH", "1"),
    ("GIT_ALLOW_PROTOCOL", "none"),
    ("GIT_TERMINAL_PROMPT", "0"),
    // Reading never needs to write, for example to refresh the index.
    ("GIT_OPTIONAL_LOCKS", "0"),
];

const ARGUMENTS: [&str; 4] = [
    "--no-pager",
    // A replace ref would substitute other content for an object, so what
    // is read would no longer be what the tag published.
    "--no-replace-objects",
    // Empty disables fsmonitor in every Git version. Before 2.36 the value
    // is a hook to run, so `false` would name a program.
    "-c",
    "core.fsmonitor=",
];

impl GitRepository {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        // Variables such as `GIT_DIR`, `GIT_SSH_COMMAND`, and
        // `GIT_CONFIG_PARAMETERS` would redirect or reconfigure the read.
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
            .current_dir(&self.path)
            .env("GIT_DIR", &self.git_dir)
            .envs(ENVIRONMENT)
            .args(ARGUMENTS)
            .args(args);
        command
    }

    pub(super) fn error(&self, kind: GitErrorKind) -> GitError {
        GitError {
            repository: self.path.clone(),
            kind,
        }
    }

    pub(super) fn failed(&self, args: &[&str], status: ExitStatus, stderr: &[u8]) -> GitError {
        self.error(GitErrorKind::Failed {
            command: describe(args),
            status,
            stderr: String::from_utf8_lossy(stderr).trim().to_owned(),
        })
    }

    /// Runs a command to completion and returns its standard output.
    pub(super) fn output(&self, args: &[&str]) -> Result<Vec<u8>, GitError> {
        self.run(self.command(args), args)
    }

    /// Runs a command that reads the working tree at `work_tree`, the
    /// repository's top level, and returns its standard output. Only
    /// commands that read are run this way.
    pub(super) fn work_tree_output(
        &self,
        work_tree: &Path,
        args: &[&str],
    ) -> Result<Vec<u8>, GitError> {
        let mut command = self.command(args);
        // Explicit, so the repository's `core.worktree` cannot point the
        // read at another directory.
        command.env("GIT_WORK_TREE", work_tree);
        self.run(command, args)
    }

    fn run(&self, mut command: Command, args: &[&str]) -> Result<Vec<u8>, GitError> {
        let output = command
            .stdin(Stdio::null())
            .output()
            .map_err(|error| self.error(GitErrorKind::Unavailable(error)))?;
        if !output.status.success() {
            return Err(self.failed(args, output.status, &output.stderr));
        }
        Ok(output.stdout)
    }

    /// Starts reading objects one at a time.
    pub(super) fn objects(&self) -> Result<Objects<'_>, GitError> {
        Ok(Objects {
            headers: CatFile::start(self, false)?,
            contents: CatFile::start(self, true)?,
        })
    }
}

fn describe(args: &[&str]) -> String {
    format!("git {}", args.join(" "))
}

/// What Git reports about an object before its contents.
#[derive(Debug)]
pub(super) struct Header {
    pub(super) id: ObjectId,
    pub(super) kind: String,
    pub(super) size: u64,
}

/// An object read from the object database.
#[derive(Debug)]
pub(super) struct Object {
    pub(super) header: Header,
    /// The object's contents, or `None` when it is larger than the limit
    /// the read allowed.
    pub(super) contents: Option<Vec<u8>>,
}

/// Reads objects one at a time. Every object's size is known before its
/// contents are requested, so contents over a limit are never read at all.
/// Git stores objects compressed, and a small object can expand to far more
/// than Assemblr should hold or even stream.
pub(super) struct Objects<'r> {
    headers: CatFile<'r>,
    contents: CatFile<'r>,
}

impl Objects<'_> {
    /// The header of the object `name` names, or `None` if there is no such
    /// object.
    pub(super) fn header(&mut self, name: &str) -> Result<Option<Header>, GitError> {
        Ok(self.headers.read(name)?.map(|object| object.header))
    }

    /// Reads the object `name` names, or `None` if there is no such object.
    /// Contents larger than `limit` bytes are not read.
    pub(super) fn read(&mut self, name: &str, limit: u64) -> Result<Option<Object>, GitError> {
        let Some(header) = self.header(name)? else {
            return Ok(None);
        };
        if header.size > limit {
            return Ok(Some(Object {
                header,
                contents: None,
            }));
        }
        match self.contents.read(header.id.as_str())? {
            Some(object) if object.header.id == header.id && object.header.size == header.size => {
                Ok(Some(object))
            }
            _ => Err(self
                .contents
                .failure(format!("object {} changed while it was read", header.id))),
        }
    }

    /// Ends reading, reporting whether Git succeeded.
    pub(super) fn finish(self) -> Result<(), GitError> {
        self.headers.finish()?;
        self.contents.finish()
    }
}

/// A running `git cat-file --batch-check`, which answers each object name
/// written to it with the object's header, or `git cat-file --batch`, which
/// also sends the contents.
struct CatFile<'r> {
    repository: &'r GitRepository,
    args: &'static [&'static str],
    with_contents: bool,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr: Option<JoinHandle<Vec<u8>>>,
}

impl<'r> CatFile<'r> {
    fn start(repository: &'r GitRepository, with_contents: bool) -> Result<Self, GitError> {
        let args: &'static [&'static str] = if with_contents {
            &["cat-file", "--batch"]
        } else {
            &["cat-file", "--batch-check"]
        };
        let mut child = repository
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| repository.error(GitErrorKind::Unavailable(error)))?;
        let (Some(stdin), Some(stdout), Some(mut stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            unreachable!("every standard stream of `git cat-file` is piped");
        };
        // Drained separately, so Git never blocks on a full error pipe while
        // Assemblr waits for an object.
        let stderr = thread::spawn(move || {
            let mut text = Vec::new();
            let _ = stderr.read_to_end(&mut text);
            text
        });
        Ok(Self {
            repository,
            args,
            with_contents,
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            stderr: Some(stderr),
        })
    }
}

impl CatFile<'_> {
    /// Asks for the object `name` names: its header and, in a session with
    /// contents, its contents. `None` if there is no such object.
    fn read(&mut self, name: &str) -> Result<Option<Object>, GitError> {
        self.exchange(name).map_err(|detail| self.failure(detail))
    }

    fn exchange(&mut self, name: &str) -> Result<Option<Object>, String> {
        let stdin = self.stdin.as_mut().ok_or("the session has ended")?;
        writeln!(stdin, "{name}")
            .and_then(|()| stdin.flush())
            .map_err(|error| format!("could not ask for {name}: {error}"))?;

        let mut header = Vec::new();
        self.stdout
            .read_until(b'\n', &mut header)
            .map_err(|error| error.to_string())?;
        if header.pop() != Some(b'\n') {
            return Err(format!("no answer for {name}"));
        }
        let header = String::from_utf8_lossy(&header);
        let (id, kind, size) = match header.split(' ').collect::<Vec<_>>()[..] {
            [object, "missing"] if object == name => return Ok(None),
            [id, kind, size] => (id, kind, size),
            _ => return Err(format!("unexpected answer {header:?} for {name}")),
        };
        let unexpected = || format!("unexpected answer {header:?} for {name}");
        let header = Header {
            id: ObjectId::parse(id).ok_or_else(unexpected)?,
            kind: kind.to_owned(),
            size: size.parse().map_err(|_| unexpected())?,
        };
        if !self.with_contents {
            return Ok(Some(Object {
                header,
                contents: None,
            }));
        }

        // The caller has checked the size against its limit.
        let mut contents = Vec::new();
        let read = (&mut self.stdout)
            .take(header.size)
            .read_to_end(&mut contents)
            .map_err(|error| error.to_string())?;
        let mut newline = [0];
        let complete = read as u64 == header.size
            && self.stdout.read_exact(&mut newline).is_ok()
            && newline == *b"\n";
        if !complete {
            return Err(format!("the contents of {name} are incomplete"));
        }
        Ok(Some(Object {
            header,
            contents: Some(contents),
        }))
    }

    /// Ends the session, reporting whether Git succeeded.
    fn finish(mut self) -> Result<(), GitError> {
        let (status, stderr) = self.wait()?;
        if !status.success() {
            return Err(self.repository.failed(self.args, status, &stderr));
        }
        Ok(())
    }

    /// The error for a session that broke off. Git's own explanation, when
    /// it failed, says more than what was observed here.
    fn failure(&mut self, detail: String) -> GitError {
        match self.wait() {
            Ok((status, stderr)) if !status.success() => {
                self.repository.failed(self.args, status, &stderr)
            }
            Ok(_) => self.repository.error(GitErrorKind::UnexpectedOutput {
                command: describe(self.args),
                detail,
            }),
            Err(error) => error,
        }
    }

    fn wait(&mut self) -> Result<(ExitStatus, Vec<u8>), GitError> {
        drop(self.stdin.take());
        // Git may still be writing an answer that will not be read, and it
        // cannot exit while the pipe is full.
        let _ = io::copy(&mut self.stdout, &mut io::sink());
        let status = self
            .child
            .wait()
            .map_err(|error| self.repository.error(GitErrorKind::Unavailable(error)))?;
        let stderr = self
            .stderr
            .take()
            .and_then(|stderr| stderr.join().ok())
            .unwrap_or_default();
        Ok((status, stderr))
    }
}

impl Drop for CatFile<'_> {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
