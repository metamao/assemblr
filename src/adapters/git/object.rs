//! The raw formats of the objects Assemblr reads: commits, for their tree,
//! and trees, for their entries.
//!
//! Parsing is pure. Git's output is untrusted like any other input, so a
//! malformed object is reported as `None`, never a panic.

use std::collections::BTreeSet;

use crate::domain::ObjectId;

/// The id stored as raw bytes in a tree entry, in hexadecimal.
fn id_from_bytes(bytes: &[u8]) -> Option<ObjectId> {
    let hexadecimal: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    ObjectId::parse(&hexadecimal)
}

/// The tree a raw commit object records, from its first line, `tree <id>`.
pub(super) fn commit_tree(commit: &[u8]) -> Option<ObjectId> {
    let line = commit.split(|&byte| byte == b'\n').next()?;
    let id = std::str::from_utf8(line.strip_prefix(b"tree ")?).ok()?;
    ObjectId::parse(id)
}

/// What a tree entry is, from its mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EntryKind {
    File {
        executable: bool,
    },
    Directory,
    SymbolicLink,
    Submodule,
    /// A mode Git does not write today, such as the `100664` of very old
    /// repositories.
    Other(String),
}

impl EntryKind {
    pub(super) fn from_mode(mode: &[u8]) -> Self {
        match mode {
            b"100644" => Self::File { executable: false },
            b"100755" => Self::File { executable: true },
            b"40000" => Self::Directory,
            b"120000" => Self::SymbolicLink,
            b"160000" => Self::Submodule,
            _ => Self::Other(String::from_utf8_lossy(mode).into_owned()),
        }
    }
}

/// One entry of a tree. Names are bytes, because Git does not require them
/// to be UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TreeEntry {
    pub(super) name: Vec<u8>,
    pub(super) kind: EntryKind,
    pub(super) id: ObjectId,
}

/// The entries of the raw tree object `tree`. Each entry is
/// `<mode> <name>\0<id>`, with the id as raw bytes as long as the tree's own.
///
/// A name that contains `/` or repeats another entry's name is malformed:
/// Git never writes one, and it would give two entries the same path.
pub(super) fn parse_tree(tree: &ObjectId, mut contents: &[u8]) -> Option<Vec<TreeEntry>> {
    let mut entries = Vec::new();
    let mut names = BTreeSet::new();
    while !contents.is_empty() {
        let space = contents.iter().position(|&byte| byte == b' ')?;
        let (mode, rest) = (&contents[..space], &contents[space + 1..]);
        let nul = rest.iter().position(|&byte| byte == 0)?;
        let (name, rest) = (&rest[..nul], &rest[nul + 1..]);
        // A tree's entries hold ids as raw bytes, half as many as the tree's
        // own hexadecimal digits.
        let id_len = tree.as_str().len() / 2;
        let id = rest.get(..id_len)?;
        if mode.is_empty() || name.is_empty() || name.contains(&b'/') || !names.insert(name) {
            return None;
        }
        entries.push(TreeEntry {
            name: name.to_vec(),
            kind: EntryKind::from_mode(mode),
            id: id_from_bytes(id)?,
        });
        contents = &rest[id_len..];
    }
    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::{EntryKind, TreeEntry, commit_tree, parse_tree};
    use crate::domain::ObjectId;

    const SHA1: &str = "0123456789abcdef0123456789abcdef01234567";

    fn sha256() -> String {
        SHA1.repeat(2)[..64].to_owned()
    }

    #[test]
    fn a_commit_records_its_tree_on_its_first_line() {
        let commit = format!("tree {SHA1}\nauthor A <a@example.com> 0 +0000\n\nmessage\n");
        assert_eq!(
            commit_tree(commit.as_bytes()),
            Some(ObjectId::parse(SHA1).unwrap())
        );
        for invalid in [
            format!("parent {SHA1}\ntree {SHA1}\n"),
            format!("tree {}\n", &SHA1[..39]),
            String::new(),
        ] {
            assert_eq!(commit_tree(invalid.as_bytes()), None, "{invalid:?}");
        }
    }

    fn raw_entry(mode: &str, name: &[u8], id: &[u8]) -> Vec<u8> {
        [mode.as_bytes(), b" ", name, b"\0", id].concat()
    }

    #[test]
    fn tree_entries_are_read_with_their_kind() {
        let id = [0xab; 20];
        let contents = [
            raw_entry("100644", b"README.md", &id),
            raw_entry("100755", b"run.sh", &id),
            raw_entry("40000", b"src", &id),
            raw_entry("120000", b"link", &id),
            raw_entry("160000", b"vendor", &id),
            raw_entry("100664", b"old", &id),
            raw_entry("100644", b"caf\xe9", &id),
        ]
        .concat();
        let tree = ObjectId::parse(SHA1).unwrap();
        let id = ObjectId::parse(&"ab".repeat(20)).unwrap();
        let entry = |name: &[u8], kind| TreeEntry {
            name: name.to_vec(),
            kind,
            id: id.clone(),
        };

        assert_eq!(
            parse_tree(&tree, &contents),
            Some(vec![
                entry(b"README.md", EntryKind::File { executable: false }),
                entry(b"run.sh", EntryKind::File { executable: true }),
                entry(b"src", EntryKind::Directory),
                entry(b"link", EntryKind::SymbolicLink),
                entry(b"vendor", EntryKind::Submodule),
                entry(b"old", EntryKind::Other("100664".to_owned())),
                entry(b"caf\xe9", EntryKind::File { executable: false }),
            ])
        );
    }

    #[test]
    fn sha256_trees_hold_longer_ids() {
        let tree = ObjectId::parse(&sha256()).unwrap();
        let contents = raw_entry("100644", b"a", &[0xcd; 32]);

        let entries = parse_tree(&tree, &contents).unwrap();
        assert_eq!(entries[0].id.as_str(), "cd".repeat(32));
    }

    #[test]
    fn malformed_trees_are_rejected() {
        let tree = ObjectId::parse(SHA1).unwrap();
        for contents in [
            b"100644 a".to_vec(),
            raw_entry("100644", b"a", &[0; 19]),
            raw_entry("", b"a", &[0; 20]),
            raw_entry("100644", b"", &[0; 20]),
            raw_entry("100644", b"a/b", &[0; 20]),
            [
                raw_entry("100644", b"a", &[0; 20]),
                raw_entry("40000", b"a", &[0; 20]),
            ]
            .concat(),
        ] {
            assert_eq!(parse_tree(&tree, &contents), None, "{contents:?}");
        }
        assert_eq!(parse_tree(&tree, b""), Some(Vec::new()));
    }
}
