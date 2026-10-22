//! The pending-resolution state (see `docs/domain/consumer-projects.md`
//! and `docs/adr/0020-pending-resolution.md`).
//!
//! A reconciliation whose only conflicts are between text the project and
//! the composition both changed writes everything that does not conflict,
//! and leaves each conflicted file at its normal path holding the project's
//! content, the base's, and the composition's between conflict markers. The
//! composition is then written except for its lock, which the journal keeps
//! until `compose --continue` finds every marker gone and writes it.
//! `compose --abort` restores the project from the journal, the marked
//! files included, to what it held before.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use super::lines;
use super::lockfile::{self, Lockfile};
use super::store::{Journal, Store};
use super::{
    Change, ComposeFailure, ComposeProblem, ComposeRecovery, CurrentFile, FileConflict,
    FileConflictKind, Step, StoreError, lock_declaration, recover, write_lock,
};
use crate::domain::{
    BaseIdentity, Contributor, Disagreement, Mergeability, RelativePath, Snapshot, TextSegment,
    UnitFile,
};

/// Opens the project's side of a conflict. A marker line starts with one
/// of the two distinctive prefixes, which is what a leftover is looked for
/// by.
const CURRENT_MARK: &str = "<<<<<<< current";
const BASE_MARK: &str = "||||||| base";
const SEPARATOR: &str = "=======";
const INCOMING_MARK: &str = ">>>>>>> incoming";

/// The change that leaves `conflict` marked in the project's file, or
/// `None` when it cannot be: the conflict is not between text the project
/// and the composition both changed, or one of the three sides is not UTF-8
/// text.
pub(super) fn marked_change(
    store: &Store,
    base: &Snapshot,
    current: &BTreeMap<RelativePath, CurrentFile>,
    files: &BTreeMap<RelativePath, UnitFile>,
    conflict: &FileConflict,
) -> Option<Change> {
    match &conflict.kind {
        FileConflictKind::Disagreement(Disagreement::BothChanged(Mergeability::Format))
        | FileConflictKind::Values(_)
        | FileConflictKind::Lines { .. }
        | FileConflictKind::Unmergeable(_) => {}
        FileConflictKind::Disagreement(_) => return None,
    }
    let held = current.get(&conflict.path)?;
    let incoming = files.get(&conflict.path)?;
    let base_file = base.files().get(&conflict.path)?;
    let base_bytes = store.read_blob(base_file.blob()).ok()?;
    // A text file the merge could not finish marks only the lines that
    // disagree; every other conflict marks the whole file, because a
    // structured file's merged parts cannot be told apart from its data.
    let bytes = match conflict.kind {
        FileConflictKind::Lines { .. } => {
            mark_lines(&base_bytes, &held.bytes, incoming.contents())?
        }
        _ => mark(&base_bytes, &held.bytes, incoming.contents())?,
    };
    // The composition's change to the executable bit wins when it made
    // one; otherwise the project's mode stands, as in reconciliation.
    let executable = if incoming.is_executable() == base_file.is_executable() {
        held.executable
    } else {
        incoming.is_executable()
    };
    Some(Change::Modify {
        file: UnitFile::new(bytes, executable),
        previous: held.bytes.clone(),
        previous_executable: held.executable,
        merged: false,
    })
}

/// The three versions of a file between conflict markers, or `None` when
/// any of them is not UTF-8 text. Each version is kept as it is, and the
/// marker lines end as the project's file does.
fn mark(base: &[u8], current: &[u8], incoming: &[u8]) -> Option<Vec<u8>> {
    let text = |bytes: &[u8]| !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok();
    if !(text(base) && text(current) && text(incoming)) {
        return None;
    }
    let eol = if current.windows(2).any(|pair| pair == b"\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut out = Vec::new();
    let mut section = |line: &str, body: &[u8]| {
        out.extend_from_slice(line.as_bytes());
        out.extend_from_slice(eol.as_bytes());
        out.extend_from_slice(body);
        if !body.is_empty() && !body.ends_with(b"\n") {
            out.extend_from_slice(eol.as_bytes());
        }
    };
    section(&format!("{CURRENT_MARK} (your file)"), current);
    section(&format!("{BASE_MARK} (the last composition)"), base);
    section(SEPARATOR, incoming);
    out.extend_from_slice(format!("{INCOMING_MARK} (the declared composition){eol}").as_bytes());
    Some(out)
}

/// The project's file with the composition's changes merged in, and a
/// conflict marked around each place where the two disagree: the project's
/// lines, the base's, and the composition's. `None` when a version is not
/// UTF-8 text or the merge finds nothing to mark.
fn mark_lines(base: &[u8], current: &[u8], incoming: &[u8]) -> Option<Vec<u8>> {
    fn text(bytes: &[u8]) -> Option<&str> {
        if bytes.contains(&0) {
            return None;
        }
        std::str::from_utf8(bytes).ok()
    }
    let (base, current, incoming) = (text(base)?, text(current)?, text(incoming)?);
    let versions = lines::Versions::new(base, current, incoming);
    let merge = versions.merge();
    merge.conflicts().next()?;
    // Marker lines end as the file's lines do.
    let eol = if !versions.crlf && current.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut out = String::new();
    for segment in merge.segments() {
        match segment {
            TextSegment::Text(lines) => lines.iter().for_each(|line| out.push_str(line)),
            TextSegment::Conflict(conflict) => {
                let side = |contributor: Contributor| {
                    conflict
                        .sides
                        .iter()
                        .find(|(side, _)| *side == contributor)
                        .map(|(_, lines)| lines.as_slice())
                        .unwrap_or_default()
                };
                let mut section = |header: &str, lines: &[&str]| {
                    out.push_str(header);
                    out.push_str(eol);
                    lines.iter().for_each(|line| out.push_str(line));
                    if lines.last().is_some_and(|line| !line.ends_with('\n')) {
                        out.push_str(eol);
                    }
                };
                section(
                    &format!("{CURRENT_MARK} (your file)"),
                    side(Contributor::Current),
                );
                section(
                    &format!("{BASE_MARK} (the last composition)"),
                    &conflict.base,
                );
                section(SEPARATOR, side(Contributor::Incoming));
                out.push_str(&format!("{INCOMING_MARK} (the declared composition){eol}"));
            }
        }
    }
    Some(versions.written(out).into_bytes())
}

/// Whether `bytes` still holds a line that opens or closes a conflict.
fn has_markers(bytes: &[u8]) -> bool {
    bytes.split(|byte| *byte == b'\n').any(|line| {
        line.starts_with(CURRENT_MARK.as_bytes()) || line.starts_with(INCOMING_MARK.as_bytes())
    })
}

/// Whether the lock records the journal's composition: it is written
/// after every file, so the composition is then complete but for the
/// journal's removal.
pub(super) fn lock_records(root: &Path, journal: &Journal) -> bool {
    matches!(
        lockfile::read_lockfile(root),
        Ok(Some((_, Lockfile { project: Some(state), .. }))) if *state.base() == journal.base
    )
}

/// What `compose --continue` finalized.
#[derive(Debug)]
pub struct Continued {
    base: BaseIdentity,
    files: Vec<RelativePath>,
    completed: bool,
}

impl Continued {
    /// The identity of the composition, the reconciliation base of the
    /// next one.
    pub fn base(&self) -> &BaseIdentity {
        &self.base
    }

    /// The files that held conflicts, whose content is now the project's.
    pub fn files(&self) -> &[RelativePath] {
        &self.files
    }

    /// Whether an earlier `--continue` had written the lock already, and
    /// only the journal was left to remove.
    pub fn completed(&self) -> bool {
        self.completed
    }
}

/// Finalizes the composition of the project at `root` that waits for the
/// consumer: when no file of it holds a conflict marker any longer, the
/// lock is written and the journal removed. A file the consumer deleted
/// counts as resolved; the project's content is the resolution.
pub fn continue_compose(root: &Path) -> Result<Continued, ComposeFailure> {
    continue_with(root, &mut |_| Ok(()))
}

pub(super) fn continue_with(
    root: &Path,
    fault: &mut dyn FnMut(Step) -> io::Result<()>,
) -> Result<Continued, ComposeFailure> {
    let rejected = |problem| ComposeFailure::Rejected {
        problems: vec![problem],
        recovered: None,
    };
    let (guard, store, journal) = open(root).map_err(rejected)?;
    if lock_records(root, &journal) {
        recover(root, &store, &journal, &guard, &[])
            .map_err(|error| rejected(ComposeProblem::Store(error)))?;
        return Ok(Continued {
            base: journal.base,
            files: journal
                .pending
                .map(|pending| pending.files)
                .unwrap_or_default(),
            completed: true,
        });
    }
    let Some(pending) = &journal.pending else {
        return Err(rejected(ComposeProblem::NotPending));
    };
    let mut unresolved = Vec::new();
    for path in &pending.files {
        match read_resolution(root, path) {
            Ok(Resolution::Absent) => {}
            Ok(Resolution::Text(bytes)) => {
                if has_markers(&bytes) {
                    unresolved.push((path.clone(), "it still holds conflict markers".to_owned()));
                }
            }
            Ok(Resolution::Other) => unresolved.push((
                path.clone(),
                "it is not a regular file in the project".to_owned(),
            )),
            Err(error) => return Err(rejected(error)),
        }
    }
    if !unresolved.is_empty() {
        return Err(rejected(ComposeProblem::Unresolved(unresolved)));
    }
    // The lock was kept when the conflicts were marked; it must still be
    // the one for this composition, over a base that verifies.
    let lock_bytes = store
        .read_previous(&pending.lock)
        .map_err(|error| rejected(ComposeProblem::Store(error)))?;
    let state = match lockfile::parse(&lock_bytes) {
        Ok(Lockfile {
            project: Some(state),
            ..
        }) if *state.base() == journal.base => state,
        _ => {
            return Err(rejected(ComposeProblem::Store(StoreError::Corrupt {
                path: root.join(".assemblr"),
                detail: "the lock kept for this composition is not its own".to_owned(),
            })));
        }
    };
    store
        .verify_base(state.base(), state.unbased())
        .map_err(|error| rejected(ComposeProblem::Base(error)))?;
    write_lock(root, &lock_bytes, fault)?;
    let finished = fault(Step::Finish).and_then(|()| {
        store.finish().map_err(|error| match error {
            StoreError::Io { error, .. } => error,
            other => io::Error::other(other.to_string()),
        })
    });
    if let Err(error) = finished {
        return Err(ComposeFailure::Write {
            step: super::WriteStep::Finish,
            error,
        });
    }
    Ok(Continued {
        base: journal.base,
        files: pending.files.clone(),
        completed: false,
    })
}

/// Restores the project at `root` to what it held before the composition
/// that waits for the consumer: every file it changed, the marked files
/// whatever they hold now, and the declaration `add` or `remove` rewrote.
/// A file edited since that the composition changed without marking it is
/// kept and named, as in any revert. Refused when the journal is an
/// interrupted composition's, which `compose` recovers.
pub fn abort_compose(root: &Path) -> Result<ComposeRecovery, ComposeFailure> {
    let rejected = |problem| ComposeFailure::Rejected {
        problems: vec![problem],
        recovered: None,
    };
    let (guard, store, journal) = open(root).map_err(rejected)?;
    // An interrupted composition is for `compose` to complete or revert,
    // as `--continue` also says; only a lock that records this one means an
    // earlier `--continue` got as far as writing it.
    let resolving = match &journal.pending {
        Some(pending) => pending.files.clone(),
        None if lock_records(root, &journal) => Vec::new(),
        None => return Err(rejected(ComposeProblem::NotPending)),
    };
    recover(root, &store, &journal, &guard, &resolving)
        .map_err(|error| rejected(ComposeProblem::Store(error)))
}

/// Takes the project's lock and reads the journal.
fn open(root: &Path) -> Result<(fs::File, Store, Journal), ComposeProblem> {
    let guard = lock_declaration(root)?;
    let store = Store::new(root);
    match store.journal() {
        Ok(Some(journal)) => Ok((guard, store, journal)),
        Ok(None) => Err(ComposeProblem::NothingPending),
        Err(error) => Err(ComposeProblem::Store(error)),
    }
}

/// What a file being resolved holds.
enum Resolution {
    Absent,
    Text(Vec<u8>),
    Other,
}

/// Reads `path` of the project at `root` through real directories only: a
/// symbolic link on the way, or at the file, is not a file of the project.
fn read_resolution(root: &Path, path: &RelativePath) -> Result<Resolution, ComposeProblem> {
    let io = |path: &Path, error| ComposeProblem::Io {
        path: path.to_owned(),
        error,
    };
    let mut prefix = root.to_owned();
    let mut segments: Vec<&str> = path.segments().collect();
    segments.pop();
    for segment in segments {
        prefix.push(segment);
        match fs::symlink_metadata(&prefix) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Ok(Resolution::Other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Resolution::Absent),
            Err(error) => return Err(io(&prefix, error)),
        }
    }
    let target = root.join(path.as_str());
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.is_file() => fs::read(&target)
            .map(Resolution::Text)
            .map_err(|error| io(&target, error)),
        Ok(_) => Ok(Resolution::Other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Resolution::Absent),
        Err(error) => Err(io(&target, error)),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]
mod tests {
    use super::{has_markers, mark, mark_lines};

    #[test]
    fn the_three_versions_are_kept_between_markers() {
        let marked = mark(b"base\n", b"mine\n", b"theirs").unwrap();
        assert_eq!(
            String::from_utf8(marked).unwrap(),
            "<<<<<<< current (your file)\nmine\n\
             ||||||| base (the last composition)\nbase\n\
             =======\ntheirs\n\
             >>>>>>> incoming (the declared composition)\n"
        );
    }

    #[test]
    fn markers_end_lines_as_the_project_s_file_does() {
        let marked = mark(b"a", b"b\r\nc\r\n", b"d").unwrap();
        let text = String::from_utf8(marked).unwrap();
        assert!(text.starts_with("<<<<<<< current (your file)\r\nb\r\nc\r\n"));
        assert!(text.ends_with(">>>>>>> incoming (the declared composition)\r\n"));
    }

    #[test]
    fn binary_or_non_utf8_content_is_not_marked() {
        assert!(mark(b"a", b"b\0", b"c").is_none());
        assert!(mark(b"a", b"b", &[0xff, 0xfe]).is_none());
    }

    #[test]
    fn a_leftover_marker_line_is_found_and_a_heading_underline_is_not() {
        let marked = mark(b"a\n", b"b\n", b"c\n").unwrap();
        assert!(has_markers(&marked));
        assert!(!has_markers(b"Title\n=======\ntext\n"));
        assert!(has_markers(b"x\r\n<<<<<<< current (your file)\r\n"));
        assert!(!has_markers(b"resolved\n"));
    }

    fn marked(base: &str, current: &str, incoming: &str) -> String {
        String::from_utf8(
            mark_lines(base.as_bytes(), current.as_bytes(), incoming.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    const BASE: &str = "a\nb\nc\nd\ne\nf\ng\n";

    #[test]
    fn each_place_that_disagrees_is_marked_apart_and_the_rest_is_merged() {
        let out = marked(
            BASE,
            "A\nb\nc\nd\ne\nf\nmine\n",
            "theirs\nb\nc\nd\ne\nf\nG\n",
        );
        assert_eq!(
            out,
            "<<<<<<< current (your file)\nA\n||||||| base (the last composition)\na\n=======\n\
             theirs\n>>>>>>> incoming (the declared composition)\nb\nc\nd\ne\nf\n\
             <<<<<<< current (your file)\nmine\n||||||| base (the last composition)\ng\n=======\n\
             G\n>>>>>>> incoming (the declared composition)\n"
        );
    }

    #[test]
    fn a_side_without_a_final_line_ending_still_gets_its_marker_on_its_own_line() {
        let out = marked("a\nb", "a\nmine", "a\ntheirs");
        assert_eq!(
            out,
            "a\n<<<<<<< current (your file)\nmine\n||||||| base (the last composition)\nb\n\
             =======\ntheirs\n>>>>>>> incoming (the declared composition)\n"
        );
    }

    #[test]
    fn nothing_is_marked_when_the_merge_has_no_conflict_or_a_side_is_not_text() {
        assert_eq!(mark_lines(b"a\nb\n", b"A\nb\n", b"a\nb\nC\n"), None);
        assert_eq!(mark_lines(b"a\n", b"a\0\n", b"b\n"), None);
        assert_eq!(mark_lines(b"a\n", &[0xff, 0xfe], b"b\n"), None);
    }
}
