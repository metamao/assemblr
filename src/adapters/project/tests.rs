//! Fault injection at each step of writing a composition, and what the
//! next composition's recovery does with what each one leaves.

#![allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::pending::{abort_compose, continue_with};
use super::store::Store;
use super::{
    Application, Change, ComposeFailure, ComposeProblem, ComposeRecovery, DeclarationWrite, Step,
    WriteStep, apply, recover,
};
use crate::adapters::lockfile::serialize_lockfile;
use crate::domain::BlobId;
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

/// The directories of `root` are not created again.
fn plan_of(
    root: &Path,
    files: &[(&str, &str, bool)],
    changes: BTreeMap<RelativePath, Change>,
) -> Plan {
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
            (!segments.is_empty() && !root.join(segments.join("/")).exists())
                .then(|| self::path(&segments.join("/")))
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
fn first(root: &Path) -> Plan {
    let files = [
        ("README.md", "readme\n", false),
        ("bin/run", "#!/bin/sh\n", true),
    ];
    let changes = files
        .iter()
        .map(|(name, text, executable)| (path(name), Change::Create(unit_file(text, *executable))))
        .collect();
    plan_of(root, &files, changes)
}

/// A reconciliation over the first composition: `README.md` replaced,
/// `bin/run` removed, `new.txt` created.
fn second(root: &Path) -> Plan {
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
        root,
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
        declaration: None,
        marked: &BTreeSet::new(),
        declaration_used: BlobId::of(&fs::read(root.join("assemblr.jsonc")).unwrap_or_default()),
    };
    apply(root, &store, &application, &mut fault)
}

/// The project's declaration, opened for reading and writing as a
/// composition holds it, and created with `{}` when there is none.
fn guard(root: &Path) -> fs::File {
    let declaration = root.join("assemblr.jsonc");
    if !declaration.exists() {
        fs::write(&declaration, "{}\n").unwrap();
    }
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(declaration)
        .unwrap()
}

/// [`apply_with`] over a composition that also rewrites the declaration
/// to `text`.
fn apply_declaring(
    root: &Path,
    plan: &Plan,
    step: Option<Step>,
    text: &str,
) -> Result<Vec<&'static str>, ComposeFailure> {
    let mut fault = |current: Step| {
        if Some(current) == step {
            return Err(io::Error::other("injected"));
        }
        Ok(())
    };
    let guard = guard(root);
    let previous = fs::read(root.join("assemblr.jsonc")).unwrap();
    let store = Store::new(root);
    let application = Application {
        files: &plan.files,
        changes: &plan.changes,
        snapshot: &plan.snapshot,
        unbased: &BTreeSet::new(),
        lock_bytes: &plan.lock_bytes,
        directories: &plan.directories,
        declaration: Some(DeclarationWrite {
            guard: &guard,
            text: text.as_bytes(),
            previous: &previous,
        }),
        marked: &BTreeSet::new(),
        declaration_used: BlobId::of(text.as_bytes()),
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
    apply_with(&root, &first(&root), None, 0).unwrap();
    assert!(!exists(&root, ".assemblr/journal.json"));
    root
}

#[test]
fn a_failure_before_the_journal_leaves_the_project_untouched() {
    let root = project("before-journal");
    let plan = first(&root);
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
        let plan = first(&root);
        let failure = apply_failing(&root, &plan, step, n);
        assert!(matches!(failure, ComposeFailure::Write { .. }), "{name}");
        let store = Store::new(&root);
        let journal = store.journal().unwrap().expect("the journal remains");
        assert!(!exists(&root, "assemblr-lock.json"), "{name}");

        let recovered = recover(&root, &store, &journal, &guard(&root), &[]).unwrap();
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
    let plan = first(&root);
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

    let recovered = recover(&root, &store, &journal, &guard(&root), &[]).unwrap();
    assert_eq!(recovered, ComposeRecovery::Completed);
    assert_eq!(text(&root, "README.md"), "readme\n");
    assert!(exists(&root, "bin/run"));
    assert!(exists(&root, "assemblr-lock.json"));
    assert!(!exists(&root, ".assemblr/journal.json"));
}

#[test]
fn an_edited_file_is_kept_by_a_revert() {
    let root = project("edited");
    let plan = first(&root);
    apply_failing(&root, &plan, Step::Lock, 1);
    fs::write(root.join("README.md"), "mine").unwrap();
    let store = Store::new(&root);
    let journal = store.journal().unwrap().unwrap();

    let recovered = recover(&root, &store, &journal, &guard(&root), &[]).unwrap();
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
    // Creations and replacements apply in path order, then removals:
    // README.md replaced, new.txt created, bin/run removed.
    for n in 1..=3 {
        let root = composed(&format!("reconcile-file-{n}"));
        let plan = second(&root);
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

        let recovered = recover(&root, &store, &journal, &guard(&root), &[]).unwrap();
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
        let expected_restored: Vec<RelativePath> =
            (n >= 2).then(|| path("README.md")).into_iter().collect();
        let expected_removed: Vec<RelativePath> =
            (n >= 3).then(|| path("new.txt")).into_iter().collect();
        assert_eq!(restored, expected_restored, "{n}");
        assert_eq!(removed, expected_removed, "{n}");
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
        let identity = first(&root).snapshot.identity();
        assert!(
            String::from_utf8(fs::read(root.join("assemblr-lock.json")).unwrap())
                .unwrap()
                .contains(identity.as_str()),
            "{n}"
        );
    }
}

#[test]
fn a_reconciliation_interrupted_at_the_lock_is_reverted_and_after_it_completed() {
    let root = composed("reconcile-lock");
    let plan = second(&root);
    apply_failing(&root, &plan, Step::Lock, 1);
    let store = Store::new(&root);
    let journal = store.journal().unwrap().unwrap();
    let recovered = recover(&root, &store, &journal, &guard(&root), &[]).unwrap();
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
    let plan = second(&root);
    apply_failing(&root, &plan, Step::Finish, 1);
    let store = Store::new(&root);
    let journal = store.journal().unwrap().unwrap();
    assert_eq!(
        recover(&root, &store, &journal, &guard(&root), &[]).unwrap(),
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
    let plan = second(&root);
    apply_failing(&root, &plan, Step::Lock, 1);
    // The replaced file edited again, the removed path holding something
    // new, the created file edited.
    fs::write(root.join("README.md"), "edited after").unwrap();
    fs::create_dir(root.join("bin")).unwrap();
    fs::write(root.join("bin/run"), "something else").unwrap();
    fs::write(root.join("new.txt"), "edited").unwrap();
    let store = Store::new(&root);
    let journal = store.journal().unwrap().unwrap();
    let recovered = recover(&root, &store, &journal, &guard(&root), &[]).unwrap();
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
    let base = first(&root).snapshot.identity();
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

#[test]
fn a_journal_is_written_in_format_2_and_both_formats_are_read() {
    let root = composed("journal-formats");
    let base = first(&root).snapshot.identity();
    let blob = crate::domain::BlobId::of(b"readme\n");
    let store = Store::new(&root);
    let journal = super::store::Journal {
        base: base.clone(),
        files: vec![(
            path("README.md"),
            super::JournalEntry::Create { blob: blob.clone() },
        )],
        directories: vec![],
        pending: Some(super::store::Pending {
            lock: blob.clone(),
            files: vec![path("README.md")],
            declaration: Some(blob.clone()),
        }),
    };
    store.begin(&journal).unwrap();
    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".assemblr/journal.json")).unwrap()).unwrap();
    assert_eq!(written["format"], 3);
    assert_eq!(store.journal().unwrap().unwrap(), journal);

    // Format 1 has the same members, `pending` included: a journal an
    // earlier version left is still recovered.
    let one = String::from_utf8(fs::read(root.join(".assemblr/journal.json")).unwrap())
        .unwrap()
        .replace("\"format\": 2", "\"format\": 1");
    fs::write(root.join(".assemblr/journal.json"), one).unwrap();
    assert_eq!(store.journal().unwrap().unwrap(), journal);
}

#[test]
fn a_journal_with_a_newer_format_or_an_unknown_member_is_refused_as_a_journal_and_a_damaged_one_as_damaged()
 {
    let root = composed("journal-unknown");
    let base = first(&root).snapshot.identity();
    let blob = crate::domain::BlobId::of(b"readme\n");
    let entry =
        format!("{{\"path\": \"README.md\", \"action\": \"create\", \"blob\": \"{blob}\"}}");
    let cases = [
        (
            format!("{{\"format\": 4, \"base\": \"{base}\", \"files\": [], \"directories\": []}}"),
            "it has format 4, which this version of Assemblr does not read",
        ),
        (
            format!(
                "{{\"format\": 2, \"base\": \"{base}\", \"files\": [], \"directories\": [], \"keep\": true}}"
            ),
            "it has the member `keep`",
        ),
        (
            format!(
                "{{\"format\": 2, \"base\": \"{base}\", \"files\": [{}], \"directories\": []}}",
                entry.replace("}", ", \"mode\": \"755\"}")
            ),
            "a `files` entry has the member `mode`",
        ),
        (
            format!(
                "{{\"format\": 1, \"base\": \"{base}\", \"files\": [{entry}], \"directories\": [], \"pending\": {{\"lock\": \"{blob}\", \"files\": [], \"since\": 1}}}}"
            ),
            "its `pending` has the member `since`",
        ),
    ];
    let store = Store::new(&root);
    for (text, detail) in cases {
        fs::write(root.join(".assemblr/journal.json"), &text).unwrap();
        let error = store.journal().unwrap_err();
        assert!(
            matches!(&error, super::StoreError::Journal { detail: found, .. } if found.contains(detail)),
            "{text}: {error}"
        );
        assert!(error.to_string().contains("is left as it is"), "{error}");
    }

    // A damaged journal is not one a newer version wrote, and is reported
    // as damaged.
    let damaged = [
        ("{\"format\": 2, \"base\": ".to_owned(), "EOF while parsing"),
        (
            "{\"format\": 2, \"files\": [], \"directories\": []}".to_owned(),
            "it has no valid `base` identity",
        ),
        (
            format!(
                "{{\"format\": 2, \"base\": \"{base}\", \"files\": [{}], \"directories\": []}}",
                entry.replace("create", "rename")
            ),
            "has the action \"rename\"",
        ),
    ];
    for (text, detail) in damaged {
        fs::write(root.join(".assemblr/journal.json"), &text).unwrap();
        let error = store.journal().unwrap_err();
        assert!(
            matches!(&error, super::StoreError::Corrupt { detail: found, .. } if found.contains(detail)),
            "{text}: {error}"
        );
    }
}

#[test]
fn a_file_created_in_a_directory_another_leaves_finds_the_directory() {
    // `bin/run` leaves and `bin/zzz` arrives: the removal would empty and
    // remove `bin/` first in path order, so creations go first.
    let root = composed("rename");
    let changes = BTreeMap::from([
        (
            path("bin/run"),
            Change::Remove {
                previous: b"#!/bin/sh\n".to_vec(),
                previous_executable: true,
            },
        ),
        (path("bin/zzz"), Change::Create(unit_file("zzz\n", false))),
    ]);
    let plan = plan_of(
        &root,
        &[
            ("README.md", "readme\n", false),
            ("bin/zzz", "zzz\n", false),
        ],
        changes,
    );
    apply_with(&root, &plan, None, 0).unwrap();
    assert!(!exists(&root, "bin/run"));
    assert_eq!(text(&root, "bin/zzz"), "zzz\n");
}

#[cfg(unix)]
#[test]
fn a_managed_path_behind_a_symbolic_link_is_not_a_file_of_the_project() {
    use super::read_current;
    use crate::domain::Current;

    let root = composed("symlink");
    let elsewhere = project("symlink-elsewhere");
    fs::create_dir(elsewhere.join("config")).unwrap();
    fs::write(elsewhere.join("config/app.json"), "{}").unwrap();
    std::os::unix::fs::symlink(elsewhere.join("config"), root.join("config")).unwrap();
    let base = Snapshot::new(BTreeMap::from([(
        path("config/app.json"),
        SnapshotFile::new(b"{}", false),
    )]));
    let current = read_current(&root, &base, &Snapshot::default()).unwrap();
    assert_eq!(
        current
            .get(&path("config/app.json"))
            .map(|file| &file.compared),
        Some(&Current::Other)
    );
}

#[test]
fn a_declaration_rewritten_by_an_interrupted_composition_is_restored_with_the_files() {
    // Before the declaration is written, nothing of it has changed; after
    // it and before the lock, the revert puts the previous text back
    // through the handle.
    for (name, step, declaration) in [
        ("declaration-step", Step::Declaration, "old\n"),
        ("declaration-lock", Step::Lock, "new\n"),
    ] {
        let root = composed(name);
        fs::write(root.join("assemblr.jsonc"), "old\n").unwrap();
        let plan = second(&root);
        let failure = apply_declaring(&root, &plan, Some(step), "new\n").unwrap_err();
        assert!(matches!(failure, ComposeFailure::Write { .. }), "{name}");
        assert_eq!(text(&root, "assemblr.jsonc"), declaration, "{name}");

        let store = Store::new(&root);
        let journal = store.journal().unwrap().unwrap();
        let recovered = recover(&root, &store, &journal, &guard(&root), &[]).unwrap();
        assert!(
            matches!(recovered, ComposeRecovery::Reverted { .. }),
            "{name}: {recovered:?}"
        );
        assert_eq!(text(&root, "assemblr.jsonc"), "old\n", "{name}");
        assert_eq!(text(&root, "README.md"), "readme\n", "{name}");
        assert!(!exists(&root, ".assemblr/journal.json"), "{name}");
    }
}

#[test]
fn a_declaration_written_before_an_interruption_at_the_end_is_kept() {
    let root = composed("declaration-finish");
    fs::write(root.join("assemblr.jsonc"), "old\n").unwrap();
    let plan = second(&root);
    apply_declaring(&root, &plan, Some(Step::Finish), "new\n").unwrap_err();
    let store = Store::new(&root);
    let journal = store.journal().unwrap().unwrap();
    assert_eq!(
        recover(&root, &store, &journal, &guard(&root), &[]).unwrap(),
        ComposeRecovery::Completed
    );
    assert_eq!(text(&root, "assemblr.jsonc"), "new\n");
    assert_eq!(text(&root, "README.md"), "readme v2\n");
}

#[test]
fn a_declaration_edited_during_an_interruption_is_kept_and_named() {
    let root = composed("declaration-edited");
    fs::write(root.join("assemblr.jsonc"), "old\n").unwrap();
    let plan = second(&root);
    apply_declaring(&root, &plan, Some(Step::Lock), "new\n").unwrap_err();
    fs::write(root.join("assemblr.jsonc"), "edited by hand\n").unwrap();
    let store = Store::new(&root);
    let journal = store.journal().unwrap().unwrap();
    let recovered = recover(&root, &store, &journal, &guard(&root), &[]).unwrap();
    let ComposeRecovery::Reverted { kept, .. } = recovered else {
        panic!("expected a revert");
    };
    assert_eq!(kept, vec![path("assemblr.jsonc")]);
    assert_eq!(text(&root, "assemblr.jsonc"), "edited by hand\n");
}

/// A reconciliation over the first composition that leaves `README.md`
/// marked: the project's `readme\n` and the composition's `readme v2\n`
/// are in conflict. `new.txt` is created.
fn conflicting(root: &Path) -> Plan {
    let marked = "<<<<<<< current (your file)\nreadme mine\n=======\nreadme v2\n\
                  >>>>>>> incoming (the declared composition)\n";
    let changes = BTreeMap::from([
        (
            path("README.md"),
            Change::Modify {
                file: unit_file(marked, false),
                previous: b"readme mine\n".to_vec(),
                previous_executable: false,
                merged: false,
            },
        ),
        (path("new.txt"), Change::Create(unit_file("new\n", false))),
    ]);
    plan_of(
        root,
        &[
            ("README.md", "readme v2\n", false),
            ("new.txt", "new\n", false),
        ],
        changes,
    )
}

/// Applies `plan`, whose `README.md` is marked, in `root`, failing at
/// `step` when given.
fn apply_marked(
    root: &Path,
    plan: &Plan,
    step: Option<Step>,
) -> Result<Vec<&'static str>, ComposeFailure> {
    let mut fault = |current: Step| {
        if Some(current) == step {
            return Err(io::Error::other("injected"));
        }
        Ok(())
    };
    // The pending record names the declaration in place, which
    // `--continue` reads back.
    guard(root);
    let store = Store::new(root);
    let marked = BTreeSet::from([path("README.md")]);
    let application = Application {
        files: &plan.files,
        changes: &plan.changes,
        snapshot: &plan.snapshot,
        unbased: &BTreeSet::new(),
        lock_bytes: &plan.lock_bytes,
        directories: &plan.directories,
        declaration: None,
        marked: &marked,
        declaration_used: BlobId::of(&fs::read(root.join("assemblr.jsonc")).unwrap_or_default()),
    };
    apply(root, &store, &application, &mut fault)
}

/// The project after the first composition, with the consumer's own
/// edit to `README.md`, and a conflicting reconciliation applied.
fn pending(test: &str) -> PathBuf {
    let root = composed(test);
    fs::write(root.join("README.md"), "readme mine\n").unwrap();
    apply_marked(&root, &conflicting(&root), None).unwrap();
    root
}

fn continuing(root: &Path, step: Option<Step>) -> Result<(), ComposeFailure> {
    let mut fault = |current: Step| {
        if Some(current) == step {
            return Err(io::Error::other("injected"));
        }
        Ok(())
    };
    guard(root);
    continue_with(root, &mut fault).map(|_| ())
}

#[test]
fn conflicts_are_written_marked_and_the_lock_waits() {
    let root = pending("pending-written");
    assert!(text(&root, "README.md").starts_with("<<<<<<< current"));
    assert_eq!(text(&root, "new.txt"), "new\n");
    // The lock is still the first composition's.
    assert!(!text(&root, "assemblr-lock.json").is_empty());
    let journal = Store::new(&root).journal().unwrap().unwrap();
    assert_eq!(
        journal.pending.map(|pending| pending.files),
        Some(vec![path("README.md")])
    );
}

#[test]
fn a_failure_recording_the_wait_is_reverted_with_the_marked_files() {
    let root = composed("pending-fault");
    fs::write(root.join("README.md"), "readme mine\n").unwrap();
    let lock = text(&root, "assemblr-lock.json");
    let plan = conflicting(&root);
    let failure = apply_marked(&root, &plan, Some(Step::Pending)).unwrap_err();
    assert!(matches!(
        failure,
        ComposeFailure::Write {
            step: WriteStep::Pending,
            ..
        }
    ));
    let store = Store::new(&root);
    let journal = store.journal().unwrap().unwrap();
    assert!(journal.pending.is_none());
    recover(&root, &store, &journal, &guard(&root), &[]).unwrap();
    assert_eq!(text(&root, "README.md"), "readme mine\n");
    assert!(!exists(&root, "new.txt"));
    assert_eq!(text(&root, "assemblr-lock.json"), lock);
    assert!(!exists(&root, ".assemblr/journal.json"));
}

#[test]
fn continue_refuses_while_a_marker_remains_and_changes_nothing() {
    let root = pending("pending-unresolved");
    let before = text(&root, "README.md");
    let failure = continuing(&root, None).unwrap_err();
    match failure {
        ComposeFailure::Rejected { problems, .. } => match problems.as_slice() {
            [ComposeProblem::Unresolved(files)] => {
                assert_eq!(files[0].0, path("README.md"));
            }
            other => panic!("unexpected {other:?}"),
        },
        other @ ComposeFailure::Write { .. } => panic!("unexpected {other:?}"),
    }
    assert_eq!(text(&root, "README.md"), before);
    assert!(exists(&root, ".assemblr/journal.json"));
}

#[test]
fn continue_writes_the_lock_and_removes_the_journal_once_resolved() {
    let root = pending("pending-continue");
    let lock = text(&root, "assemblr-lock.json");
    fs::write(root.join("README.md"), "readme resolved\n").unwrap();
    continuing(&root, None).unwrap();
    assert_eq!(text(&root, "README.md"), "readme resolved\n");
    assert_ne!(text(&root, "assemblr-lock.json"), lock);
    assert!(!exists(&root, ".assemblr/journal.json"));
    assert!(!exists(&root, ".assemblr/previous"));
}

#[test]
fn a_deleted_file_counts_as_resolved() {
    let root = pending("pending-deleted");
    fs::remove_file(root.join("README.md")).unwrap();
    continuing(&root, None).unwrap();
    assert!(!exists(&root, "README.md"));
    assert!(!exists(&root, ".assemblr/journal.json"));
}

#[test]
fn a_continue_that_fails_at_the_lock_can_be_repeated() {
    let root = pending("pending-continue-lock");
    fs::write(root.join("README.md"), "readme resolved\n").unwrap();
    assert!(continuing(&root, Some(Step::Lock)).is_err());
    assert!(exists(&root, ".assemblr/journal.json"));
    continuing(&root, None).unwrap();
    assert!(!exists(&root, ".assemblr/journal.json"));
}

#[test]
fn a_continue_that_fails_after_the_lock_is_completed_by_the_next() {
    let root = pending("pending-continue-finish");
    fs::write(root.join("README.md"), "readme resolved\n").unwrap();
    assert!(continuing(&root, Some(Step::Finish)).is_err());
    assert!(exists(&root, ".assemblr/journal.json"));
    // The lock records the journal's composition: even a marker left in
    // the file no longer matters, and nothing is reverted.
    continuing(&root, None).unwrap();
    assert_eq!(text(&root, "README.md"), "readme resolved\n");
    assert!(!exists(&root, ".assemblr/journal.json"));
}

#[test]
fn abort_restores_the_marked_file_whatever_it_holds_and_the_rest() {
    let root = pending("pending-abort");
    fs::write(root.join("README.md"), "half resolved\n").unwrap();
    let store = Store::new(&root);
    let journal = store.journal().unwrap().unwrap();
    let lock = text(&root, "assemblr-lock.json");
    let resolving = journal.pending.clone().unwrap().files;
    let recovered = recover(&root, &store, &journal, &guard(&root), &resolving).unwrap();
    assert!(matches!(recovered, ComposeRecovery::Reverted { .. }));
    assert_eq!(text(&root, "README.md"), "readme mine\n");
    assert!(!exists(&root, "new.txt"));
    assert_eq!(text(&root, "assemblr-lock.json"), lock);
    assert!(!exists(&root, ".assemblr/journal.json"));
}

#[test]
fn continue_without_a_pending_composition_is_refused() {
    let root = composed("pending-none");
    guard(&root);
    assert!(matches!(
        continue_with(&root, &mut |_| Ok(())),
        Err(ComposeFailure::Rejected { problems, .. })
            if matches!(problems.as_slice(), [ComposeProblem::NothingPending])
    ));
}

#[test]
fn abort_and_continue_leave_an_interrupted_composition_to_compose() {
    let root = composed("pending-interrupted");
    let failure = apply_failing(&root, &second(&root), Step::Lock, 1);
    assert!(matches!(failure, ComposeFailure::Write { .. }));
    guard(&root);
    let before = text(&root, "README.md");
    for failure in [
        abort_compose(&root).map(|_| ()).unwrap_err(),
        continue_with(&root, &mut |_| Ok(()))
            .map(|_| ())
            .unwrap_err(),
    ] {
        assert!(matches!(
            failure,
            ComposeFailure::Rejected { problems, .. }
                if matches!(problems.as_slice(), [ComposeProblem::NotPending])
        ));
    }
    assert_eq!(text(&root, "README.md"), before);
    assert!(exists(&root, ".assemblr/journal.json"));
}

#[test]
fn discarding_a_base_keeps_every_object_another_base_names_and_the_kept_base() {
    let root = project("discard-base");
    let store = Store::new(&root);
    let snapshot = |files: &[(&str, &[u8])]| {
        Snapshot::new(
            files
                .iter()
                .map(|(name, bytes)| (path(name), SnapshotFile::new(bytes, false)))
                .collect(),
        )
    };
    let write = |snapshot: &Snapshot, files: &[(&str, &[u8])]| {
        let paths: Vec<(RelativePath, &[u8])> = files
            .iter()
            .map(|(name, bytes)| (path(name), *bytes))
            .collect();
        let refs: Vec<(&RelativePath, &[u8])> =
            paths.iter().map(|(path, bytes)| (path, *bytes)).collect();
        store.write_base(snapshot, &refs).unwrap();
    };
    // An old base, the lock's base, and an aborted one sharing objects with
    // both.
    let old_files: [(&str, &[u8]); 1] = [("a", b"old")];
    let locked_files: [(&str, &[u8]); 2] = [("a", b"shared"), ("b", b"locked")];
    let aborted_files: [(&str, &[u8]); 3] =
        [("a", b"shared"), ("b", b"old"), ("c", b"only aborted")];
    let (old, locked, aborted) = (
        snapshot(&old_files),
        snapshot(&locked_files),
        snapshot(&aborted_files),
    );
    write(&old, &old_files);
    write(&locked, &locked_files);
    write(&aborted, &aborted_files);

    // The lock's base is never discarded.
    store.discard_base(&locked.identity(), Some(&locked.identity()));
    assert!(
        store
            .verify_base(&locked.identity(), &BTreeSet::new())
            .is_ok()
    );

    store.discard_base(&aborted.identity(), Some(&locked.identity()));
    assert!(store.verify_base(&old.identity(), &BTreeSet::new()).is_ok());
    assert!(
        store
            .verify_base(&locked.identity(), &BTreeSet::new())
            .is_ok()
    );
    assert!(
        store
            .verify_base(&aborted.identity(), &BTreeSet::new())
            .is_err()
    );
    let only = SnapshotFile::new(b"only aborted", false);
    assert!(store.read_blob(only.blob()).is_err());
}
