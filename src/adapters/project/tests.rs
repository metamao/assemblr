//! Fault injection at each step of writing a composition, and what the
//! next composition's recovery does with what each one leaves.

#![allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::store::Store;
use super::{
    Application, Change, ComposeFailure, ComposeRecovery, Step, WriteStep, apply, recover,
};
use crate::adapters::lockfile::serialize_lockfile;
use crate::domain::{
    ReconciliationState, RelativePath, Snapshot, SnapshotFile, UnitFile, UnitLock,
};

/// A fresh project directory for `test`.
fn project(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("assemblr-apply-tests").join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn path(text: &str) -> RelativePath {
    RelativePath::parse(text).unwrap()
}

/// A composition to apply: its files, what changes in the project, and
/// the lock they belong to.
struct Plan {
    files: BTreeMap<RelativePath, UnitFile>,
    changes: BTreeMap<RelativePath, Change>,
    snapshot: Snapshot,
    lock_bytes: Vec<u8>,
    directories: Vec<RelativePath>,
}

fn unit_file(text: &str, executable: bool) -> UnitFile {
    UnitFile::new(text.as_bytes().to_vec(), executable)
}

fn plan_of(files: &[(&str, &str, bool)], changes: BTreeMap<RelativePath, Change>) -> Plan {
    let files: BTreeMap<RelativePath, UnitFile> = files
        .iter()
        .map(|(name, text, executable)| (path(name), unit_file(text, *executable)))
        .collect();
    let snapshot = Snapshot::new(
        files
            .iter()
            .map(|(p, f)| {
                (
                    p.clone(),
                    SnapshotFile::new(f.contents(), f.is_executable()),
                )
            })
            .collect(),
    );
    let state = ReconciliationState::new(
        snapshot.identity(),
        files.keys().cloned().collect(),
        BTreeSet::new(),
    );
    let lock_bytes = serialize_lockfile(&UnitLock::default(), Some(&state)).unwrap();
    let directories = changes
        .iter()
        .filter(|(_, change)| matches!(change, Change::Create(_)))
        .filter_map(|(path, _)| {
            let mut segments: Vec<&str> = path.segments().collect();
            segments.pop();
            (!segments.is_empty()).then(|| self::path(&segments.join("/")))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Plan {
        files,
        changes,
        snapshot,
        lock_bytes,
        directories,
    }
}

/// A first composition: two files, one in a new directory.
fn first() -> Plan {
    let files = [
        ("README.md", "readme\n", false),
        ("bin/run", "#!/bin/sh\n", true),
    ];
    let changes = files
        .iter()
        .map(|(name, text, executable)| (path(name), Change::Create(unit_file(text, *executable))))
        .collect();
    plan_of(&files, changes)
}

/// A reconciliation over the first composition: `README.md` replaced,
/// `bin/run` removed, `new.txt` created.
fn second() -> Plan {
    let changes = BTreeMap::from([
        (
            path("README.md"),
            Change::Modify {
                file: unit_file("readme v2\n", false),
                previous: b"readme\n".to_vec(),
                previous_executable: false,
                merged: false,
            },
        ),
        (
            path("bin/run"),
            Change::Remove {
                previous: b"#!/bin/sh\n".to_vec(),
                previous_executable: true,
            },
        ),
        (path("new.txt"), Change::Create(unit_file("new\n", false))),
    ]);
    plan_of(
        &[
            ("README.md", "readme v2\n", false),
            ("new.txt", "new\n", false),
        ],
        changes,
    )
}

/// Applies `plan` in `root`, failing at the `n`th occurrence of `step`,
/// or not at all when `step` is `None`.
fn apply_with(
    root: &Path,
    plan: &Plan,
    step: Option<Step>,
    n: usize,
) -> Result<Vec<&'static str>, ComposeFailure> {
    let mut seen = 0;
    let mut fault = |current: Step| {
        if Some(current) == step {
            seen += 1;
            if seen == n {
                return Err(io::Error::other("injected"));
            }
        }
        Ok(())
    };
    let store = Store::new(root);
    let application = Application {
        files: &plan.files,
        changes: &plan.changes,
        snapshot: &plan.snapshot,
        unbased: &BTreeSet::new(),
        lock_bytes: &plan.lock_bytes,
        directories: &plan.directories,
    };
    apply(root, &store, &application, &mut fault)
}

fn apply_failing(root: &Path, plan: &Plan, step: Step, n: usize) -> ComposeFailure {
    apply_with(root, plan, Some(step), n).unwrap_err()
}

fn exists(root: &Path, name: &str) -> bool {
    root.join(name).exists()
}

fn text(root: &Path, name: &str) -> String {
    fs::read_to_string(root.join(name)).unwrap()
}

/// The project after the first composition, applied in full.
fn composed(test: &str) -> PathBuf {
    let root = project(test);
    apply_with(&root, &first(), None, 0).unwrap();
    assert!(!exists(&root, ".assemblr/journal.json"));
    root
}

#[test]
fn a_failure_before_the_journal_leaves_the_project_untouched() {
    let root = project("before-journal");
    let plan = first();
    let failure = apply_failing(&root, &plan, Step::Store, 1);
    assert!(matches!(
        failure,
        ComposeFailure::Write {
            step: WriteStep::Store,
            ..
        }
    ));
    assert!(!exists(&root, ".assemblr"));

    for (step, written) in [
        (Step::Previous, WriteStep::Previous),
        (Step::Journal, WriteStep::Journal),
    ] {
        let failure = apply_failing(&root, &plan, step, 1);
        assert!(
            matches!(&failure, ComposeFailure::Write { step, .. } if std::mem::discriminant(step) == std::mem::discriminant(&written))
        );
        assert!(exists(&root, ".assemblr/objects"));
        assert!(!exists(&root, ".assemblr/journal.json"));
        assert!(!exists(&root, "README.md"));
        assert!(!exists(&root, "bin"));
        assert!(Store::new(&root).journal().unwrap().is_none());
    }
}

#[test]
fn a_first_composition_interrupted_among_its_files_or_at_the_lock_is_reverted() {
    for (name, step, n) in [("second-file", Step::Files, 2), ("lock", Step::Lock, 1)] {
        let root = project(name);
        let plan = first();
        let failure = apply_failing(&root, &plan, step, n);
        assert!(matches!(failure, ComposeFailure::Write { .. }), "{name}");
        let store = Store::new(&root);
        let journal = store.journal().unwrap().expect("the journal remains");
        assert!(!exists(&root, "assemblr-lock.json"), "{name}");

        let recovered = recover(&root, &store, &journal).unwrap();
        assert!(
            matches!(&recovered, ComposeRecovery::Reverted { kept, restored, .. } if kept.is_empty() && restored.is_empty()),
            "{name}: {recovered:?}"
        );
        assert!(!exists(&root, "README.md"), "{name}");
        assert!(!exists(&root, "bin"), "{name}");
        assert!(!exists(&root, ".assemblr/journal.json"), "{name}");
        // The store, content-addressed, stays.
        assert!(exists(&root, ".assemblr/objects"), "{name}");
    }
}

#[test]
fn a_failure_after_the_lock_is_completed_not_reverted() {
    let root = project("finish");
    let plan = first();
    let failure = apply_failing(&root, &plan, Step::Finish, 1);
    assert!(matches!(
        failure,
        ComposeFailure::Write {
            step: WriteStep::Finish,
            ..
        }
    ));
    assert!(exists(&root, "assemblr-lock.json"));
    let store = Store::new(&root);
    let journal = store.journal().unwrap().unwrap();

    let recovered = recover(&root, &store, &journal).unwrap();
    assert_eq!(recovered, ComposeRecovery::Completed);
    assert_eq!(text(&root, "README.md"), "readme\n");
    assert!(exists(&root, "bin/run"));
    assert!(exists(&root, "assemblr-lock.json"));
    assert!(!exists(&root, ".assemblr/journal.json"));
}

#[test]
fn an_edited_file_is_kept_by_a_revert() {
    let root = project("edited");
    let plan = first();
    apply_failing(&root, &plan, Step::Lock, 1);
    fs::write(root.join("README.md"), "mine").unwrap();
    let store = Store::new(&root);
    let journal = store.journal().unwrap().unwrap();

    let recovered = recover(&root, &store, &journal).unwrap();
    assert_eq!(
        recovered,
        ComposeRecovery::Reverted {
            removed: vec![path("bin/run")],
            restored: vec![],
            kept: vec![path("README.md")],
        }
    );
    assert_eq!(text(&root, "README.md"), "mine");
    assert!(!exists(&root, "bin"));
}

#[test]
fn a_reconciliation_interrupted_at_each_file_change_is_reverted_to_the_previous_content() {
    // The changes apply in path order: README.md, bin/run, new.txt.
    for n in 1..=3 {
        let root = composed(&format!("reconcile-file-{n}"));
        let plan = second();
        let failure = apply_failing(&root, &plan, Step::Files, n);
        assert!(
            matches!(
                failure,
                ComposeFailure::Write {
                    step: WriteStep::File(_),
                    ..
                }
            ),
            "{n}"
        );
        let store = Store::new(&root);
        let journal = store.journal().unwrap().expect("the journal remains");
        assert!(exists(&root, ".assemblr/previous"), "{n}");

        let recovered = recover(&root, &store, &journal).unwrap();
        let ComposeRecovery::Reverted {
            removed,
            restored,
            kept,
        } = recovered
        else {
            panic!("{n}: {recovered:?}");
        };
        assert!(kept.is_empty(), "{n}: {kept:?}");
        // What the interruption had done is undone, nothing more.
        assert_eq!(restored.len(), n - 1, "{n}: {restored:?}");
        assert!(removed.is_empty(), "{n}: {removed:?}");
        assert_eq!(text(&root, "README.md"), "readme\n", "{n}");
        assert_eq!(text(&root, "bin/run"), "#!/bin/sh\n", "{n}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(root.join("bin/run"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "{n}: the previous mode is restored");
        }
        assert!(!exists(&root, "new.txt"), "{n}");
        assert!(!exists(&root, "README.md.assemblr-new"), "{n}");
        assert!(!exists(&root, ".assemblr/journal.json"), "{n}");
        assert!(!exists(&root, ".assemblr/previous"), "{n}");
        // The lock still records the first composition.
        assert!(
            fs::read(root.join("assemblr-lock.json"))
                .unwrap()
                .windows(first().snapshot.identity().as_str().len())
                .any(|w| w == first().snapshot.identity().as_str().as_bytes())
        );
    }
}

#[test]
fn a_reconciliation_interrupted_at_the_lock_is_reverted_and_after_it_completed() {
    let root = composed("reconcile-lock");
    let plan = second();
    apply_failing(&root, &plan, Step::Lock, 1);
    let store = Store::new(&root);
    let journal = store.journal().unwrap().unwrap();
    let recovered = recover(&root, &store, &journal).unwrap();
    assert_eq!(
        recovered,
        ComposeRecovery::Reverted {
            removed: vec![path("new.txt")],
            restored: vec![path("README.md"), path("bin/run")],
            kept: vec![],
        }
    );
    assert_eq!(text(&root, "README.md"), "readme\n");
    assert!(exists(&root, "bin/run"));
    assert!(!exists(&root, "new.txt"));

    let root = composed("reconcile-finish");
    apply_failing(&root, &plan, Step::Finish, 1);
    let store = Store::new(&root);
    let journal = store.journal().unwrap().unwrap();
    assert_eq!(
        recover(&root, &store, &journal).unwrap(),
        ComposeRecovery::Completed
    );
    assert_eq!(text(&root, "README.md"), "readme v2\n");
    assert!(!exists(&root, "bin"));
    assert_eq!(text(&root, "new.txt"), "new\n");
    assert!(!exists(&root, ".assemblr/previous"));
}

#[test]
fn files_edited_during_an_interrupted_reconciliation_are_kept_and_named() {
    let root = composed("reconcile-edited");
    let plan = second();
    apply_failing(&root, &plan, Step::Lock, 1);
    // The replaced file edited again, the removed path holding something
    // new, the created file edited.
    fs::write(root.join("README.md"), "edited after").unwrap();
    fs::create_dir(root.join("bin")).unwrap();
    fs::write(root.join("bin/run"), "something else").unwrap();
    fs::write(root.join("new.txt"), "edited").unwrap();
    let store = Store::new(&root);
    let journal = store.journal().unwrap().unwrap();
    let recovered = recover(&root, &store, &journal).unwrap();
    assert_eq!(
        recovered,
        ComposeRecovery::Reverted {
            removed: vec![],
            restored: vec![],
            kept: vec![path("README.md"), path("bin/run"), path("new.txt")],
        }
    );
    assert_eq!(text(&root, "README.md"), "edited after");
    assert_eq!(text(&root, "bin/run"), "something else");
    assert_eq!(text(&root, "new.txt"), "edited");
}

#[test]
fn a_journal_without_actions_is_read_as_a_first_composition_s() {
    let root = composed("old-journal");
    let blob = crate::domain::BlobId::of(b"readme\n");
    let base = first().snapshot.identity();
    fs::write(
        root.join(".assemblr/journal.json"),
        format!(
            "{{\"format\": 1, \"base\": \"{}\", \"files\": [{{\"path\": \"README.md\", \"blob\": \"{blob}\"}}], \"directories\": []}}\n",
            base
        ),
    )
    .unwrap();
    let store = Store::new(&root);
    let journal = store.journal().unwrap().unwrap();
    assert_eq!(journal.files.len(), 1);
    assert!(matches!(
        journal.files[0].1,
        super::JournalEntry::Create { .. }
    ));
}
