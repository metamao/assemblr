//! Running `git` against a repository that is not trusted.
//!
//! Every command runs with an environment Assemblr sets, not one inherited
//! from the caller, and reads only the object database. See ADR 0006 for why
//! each setting is needed.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};

use super::object::ObjectId;
use super::{GitError, GitErrorKind, GitRepository};

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
    "-c",
    "core.fsmonitor=false",
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

    fn failed(&self, args: &[&str], status: ExitStatus, stderr: &[u8]) -> GitError {
        self.error(GitErrorKind::Failed {
            command: describe(args),
            status,
            stderr: String::from_utf8_lossy(stderr).trim().to_owned(),
        })
    }

    /// Runs a command to completion and returns its standard output.
    pub(super) fn output(&self, args: &[&str]) -> Result<Vec<u8>, GitError> {
        let output = self
            .command(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| self.error(GitErrorKind::Unavailable(error)))?;
        if !output.status.success() {
            return Err(self.failed(args, output.status, &output.stderr));
        }
        Ok(output.stdout)
    }

    /// Starts a session that reads objects one at a time.
    pub(super) fn cat_file(&self) -> Result<CatFile<'_>, GitError> {
        let mut child = self
            .command(CatFile::ARGS)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| self.error(GitErrorKind::Unavailable(error)))?;
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
        Ok(CatFile {
            repository: self,
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            stderr: Some(stderr),
        })
    }
}

fn describe(args: &[&str]) -> String {
    format!("git {}", args.join(" "))
}

/// An object read from the object database.
#[derive(Debug)]
pub(super) struct Object {
    pub(super) id: ObjectId,
    pub(super) kind: String,
    pub(super) size: u64,
    /// The object's contents, or `None` when it is larger than the limit
    /// the read allowed.
    pub(super) contents: Option<Vec<u8>>,
}

/// A running `git cat-file --batch`, which answers each object name written
/// to it with the object.
pub(super) struct CatFile<'r> {
    repository: &'r GitRepository,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr: Option<JoinHandle<Vec<u8>>>,
}

impl CatFile<'_> {
    const ARGS: &'static [&'static str] = &["cat-file", "--batch"];

    /// Reads the object `name` names, or `None` if there is no such object.
    /// Contents larger than `limit` bytes are skipped, not held in memory.
    pub(super) fn read(&mut self, name: &str, limit: u64) -> Result<Option<Object>, GitError> {
        self.exchange(name, limit)
            .map_err(|detail| self.failure(detail))
    }

    fn exchange(&mut self, name: &str, limit: u64) -> Result<Option<Object>, String> {
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
        let id = ObjectId::parse(id).ok_or_else(unexpected)?;
        let size: u64 = size.parse().map_err(|_| unexpected())?;

        let mut body = (&mut self.stdout).take(size);
        let (contents, read) = if size <= limit {
            let mut contents = Vec::new();
            let read = body.read_to_end(&mut contents).map(|read| read as u64);
            (Some(contents), read)
        } else {
            (None, io::copy(&mut body, &mut io::sink()))
        };
        let mut newline = [0];
        let complete = read.map_err(|error| error.to_string())? == size
            && self.stdout.read_exact(&mut newline).is_ok()
            && newline == *b"\n";
        if !complete {
            return Err(format!("the contents of {name} are incomplete"));
        }
        Ok(Some(Object {
            id,
            kind: kind.to_owned(),
            size,
            contents,
        }))
    }

    /// Ends the session, reporting whether Git succeeded.
    pub(super) fn finish(mut self) -> Result<(), GitError> {
        let (status, stderr) = self.wait()?;
        if !status.success() {
            return Err(self.repository.failed(Self::ARGS, status, &stderr));
        }
        Ok(())
    }

    /// The error for a session that broke off. Git's own explanation, when
    /// it failed, says more than what was observed here.
    fn failure(&mut self, detail: String) -> GitError {
        match self.wait() {
            Ok((status, stderr)) if !status.success() => {
                self.repository.failed(Self::ARGS, status, &stderr)
            }
            Ok(_) => self.repository.error(GitErrorKind::UnexpectedOutput {
                command: describe(Self::ARGS),
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
