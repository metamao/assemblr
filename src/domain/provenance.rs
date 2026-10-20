//! Where a published version's content comes from: the tagged Git commit,
//! and the tree that identifies the content.

use std::fmt;

/// A Git object id in lowercase hexadecimal: 40 digits in a SHA-1
/// repository, 64 in a SHA-256 one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId(String);

impl ObjectId {
    /// Parses an object id. Anything other than 40 or 64 lowercase
    /// hexadecimal digits is rejected.
    pub fn parse(text: &str) -> Option<Self> {
        let hexadecimal = text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        (matches!(text.len(), 40 | 64) && hexadecimal).then(|| Self(text.to_owned()))
    }

    /// The id in hexadecimal.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a published version's content was read from.
///
/// The commit is the version's provenance, and the commit's root tree is its
/// content identity (see `docs/adr/0006-reading-git-repositories.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    commit: ObjectId,
    tree: ObjectId,
}

impl Provenance {
    /// The provenance of content read from `tree`, the root tree of
    /// `commit`.
    pub fn new(commit: ObjectId, tree: ObjectId) -> Self {
        Self { commit, tree }
    }

    /// The commit the version tag points at.
    pub fn commit(&self) -> &ObjectId {
        &self.commit
    }

    /// The commit's root tree, which identifies the version's content. It
    /// covers the entire repository snapshot, not only the files a
    /// composition uses, so a change to any file gives a different tree.
    pub fn tree(&self) -> &ObjectId {
        &self.tree
    }
}

#[cfg(test)]
mod tests {
    use super::ObjectId;

    const SHA1: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn object_ids_are_sha1_or_sha256_lowercase_hexadecimal() {
        assert!(ObjectId::parse(SHA1).is_some());
        assert!(ObjectId::parse(&SHA1.repeat(2)[..64]).is_some());
        for invalid in [
            "",
            &SHA1[..39],
            &format!("{SHA1}0"),
            &SHA1.to_uppercase(),
            &SHA1.replace('a', "g"),
        ] {
            assert_eq!(ObjectId::parse(invalid), None, "{invalid:?}");
        }
    }
}
