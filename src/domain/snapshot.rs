//! The identity of a materialized composition: a canonical, versioned
//! listing of its files, hashed (see `docs/domain/consumer-projects.md` and
//! `docs/adr/0016-consumer-compose.md`).
//!
//! A consumer project's lock names the composition last materialized by an
//! identity that the next composition verifies its stored copy against.
//! The identity is SHA-256 over a canonical text listing every managed
//! file's mode, content hash, and path, in path order, and nothing else:
//! not the consumer's Git configuration, operating system, or checkout, so
//! that the same files give the same identity everywhere. The listing is
//! versioned by its first line and the identity by its prefix, so that a
//! later change to either is distinguishable from corruption.

use std::collections::BTreeMap;
use std::fmt;

use sha2::{Digest, Sha256};

use super::path::RelativePath;

/// The first line of a listing, naming its format.
const LISTING_HEADER: &str = "assemblr-base-v1";
/// The prefix of an identity, naming the algorithm over the listing.
const IDENTITY_PREFIX: &str = "sha256-v1:";

/// A materialized composition, as its identity sees it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    files: BTreeMap<RelativePath, SnapshotFile>,
}

/// One file of a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFile {
    executable: bool,
    blob: BlobId,
}

impl SnapshotFile {
    /// A file with these contents and executable bit.
    pub fn new(contents: &[u8], executable: bool) -> Self {
        Self {
            executable,
            blob: BlobId::of(contents),
        }
    }

    /// Whether the file is executable.
    pub fn is_executable(&self) -> bool {
        self.executable
    }

    /// The identity of the file's contents.
    pub fn blob(&self) -> &BlobId {
        &self.blob
    }

    /// The file's mode in a listing, as Git spells it.
    fn mode(&self) -> &'static str {
        if self.executable { "100755" } else { "100644" }
    }
}

impl Snapshot {
    /// A snapshot of `files`, by path.
    pub fn new(files: BTreeMap<RelativePath, SnapshotFile>) -> Self {
        Self { files }
    }

    /// The files, by path.
    pub fn files(&self) -> &BTreeMap<RelativePath, SnapshotFile> {
        &self.files
    }

    /// The canonical listing: the header line, then one line per file,
    /// `<mode> <blob> <path>`, in path order, each ended by `\n`. The path
    /// is the last field, so a path holding spaces reads back whole; a
    /// path never holds a line break (see [`RelativePath`]).
    pub fn listing(&self) -> String {
        let mut text = format!("{LISTING_HEADER}\n");
        for (path, file) in &self.files {
            text.push_str(file.mode());
            text.push(' ');
            text.push_str(file.blob.as_str());
            text.push(' ');
            text.push_str(path.as_str());
            text.push('\n');
        }
        text
    }

    /// The identity: the hash of the listing, prefixed by its algorithm.
    pub fn identity(&self) -> BaseIdentity {
        BaseIdentity::of_listing(&self.listing())
    }

    /// Reads a listing back, or says what is wrong with it.
    pub fn parse_listing(text: &str) -> Result<Self, ListingError> {
        let mut lines = text.split('\n');
        if lines.next() != Some(LISTING_HEADER) {
            return Err(ListingError::Header);
        }
        let mut files = BTreeMap::new();
        let mut previous: Option<RelativePath> = None;
        for (index, line) in lines.enumerate() {
            if line.is_empty() {
                // Only the final `\n` leaves an empty line.
                continue;
            }
            let number = index + 2;
            let mut parts = line.splitn(3, ' ');
            let (Some(mode), Some(blob), Some(path)) = (parts.next(), parts.next(), parts.next())
            else {
                return Err(ListingError::Line(number));
            };
            let executable = match mode {
                "100644" => false,
                "100755" => true,
                _ => return Err(ListingError::Line(number)),
            };
            let blob = BlobId::parse(blob).ok_or(ListingError::Line(number))?;
            let path = RelativePath::parse(path).map_err(|_| ListingError::Line(number))?;
            if previous.as_ref().is_some_and(|previous| *previous >= path) {
                return Err(ListingError::Order(number));
            }
            previous = Some(path.clone());
            files.insert(path, SnapshotFile { executable, blob });
        }
        if !text.ends_with('\n') {
            return Err(ListingError::Line(text.lines().count()));
        }
        Ok(Self { files })
    }
}

/// Why text is not a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListingError {
    /// The first line is not the header of a known format.
    Header,
    /// A line is not `<mode> <blob> <path>`.
    Line(usize),
    /// A line's path is not after the previous line's.
    Order(usize),
}

impl fmt::Display for ListingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header => write!(f, "its first line is not `{LISTING_HEADER}`"),
            Self::Line(number) => write!(f, "line {number} is not `<mode> <blob> <path>`"),
            Self::Order(number) => write!(f, "line {number} is out of path order"),
        }
    }
}

impl std::error::Error for ListingError {}

/// The SHA-256 of a file's contents, as lowercase hexadecimal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobId(String);

impl BlobId {
    /// The identity of `contents`.
    pub fn of(contents: &[u8]) -> Self {
        Self(hex(&Sha256::digest(contents)))
    }

    /// An identity as written, or `None` when it is not 64 lowercase
    /// hexadecimal digits.
    pub fn parse(text: &str) -> Option<Self> {
        (text.len() == 64
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        .then(|| Self(text.to_owned()))
    }

    /// The hexadecimal digits.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The identity of a reconciliation base: `sha256-v1:` and the SHA-256 of
/// its listing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BaseIdentity(String);

impl BaseIdentity {
    fn of_listing(listing: &str) -> Self {
        Self(format!(
            "{IDENTITY_PREFIX}{}",
            hex(&Sha256::digest(listing.as_bytes()))
        ))
    }

    /// An identity as written, or `None` when its algorithm is unknown or
    /// its digits malformed.
    pub fn parse(text: &str) -> Option<Self> {
        let digits = text.strip_prefix(IDENTITY_PREFIX)?;
        BlobId::parse(digits).map(|_| Self(text.to_owned()))
    }

    /// The identity as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The hexadecimal digits without the prefix, for a file name.
    pub fn digits(&self) -> &str {
        &self.0[IDENTITY_PREFIX.len()..]
    }

    /// Whether `listing` is the listing this identity was computed from.
    pub fn verifies(&self, listing: &str) -> bool {
        Self::of_listing(listing) == *self
    }
}

impl fmt::Display for BaseIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]

    use super::{BaseIdentity, BlobId, ListingError, Snapshot, SnapshotFile};
    use crate::domain::RelativePath;

    fn snapshot(files: &[(&str, &str, bool)]) -> Snapshot {
        Snapshot::new(
            files
                .iter()
                .map(|(path, contents, executable)| {
                    (
                        RelativePath::parse(path).unwrap(),
                        SnapshotFile::new(contents.as_bytes(), *executable),
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn the_listing_is_canonical_and_round_trips() {
        let a = snapshot(&[("src/b.txt", "b", false), ("a.txt", "a", true)]);
        let b = snapshot(&[("a.txt", "a", true), ("src/b.txt", "b", false)]);
        assert_eq!(a.listing(), b.listing());
        assert_eq!(
            a.listing(),
            "assemblr-base-v1\n100755 ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb \
             a.txt\n100644 3e23e8160039594a33894f6564e1b1348bbd7a0088d42c4acb73eeaed59c009d \
             src/b.txt\n"
        );
        assert_eq!(Snapshot::parse_listing(&a.listing()).unwrap(), a);
        assert_eq!(a.identity(), b.identity());
        assert!(a.identity().as_str().starts_with("sha256-v1:"));
        assert!(a.identity().verifies(&a.listing()));
        assert_eq!(
            BaseIdentity::parse(a.identity().as_str()).as_ref(),
            Some(&a.identity())
        );
        assert_eq!(Snapshot::default().listing(), "assemblr-base-v1\n");
    }

    #[test]
    fn paths_with_spaces_and_non_ascii_names_round_trip() {
        let snapshot = snapshot(&[("docs/read me.md", "x", false), ("src/año.txt", "y", false)]);
        let listing = snapshot.listing();
        assert!(listing.contains(" docs/read me.md\n"), "{listing}");
        assert!(listing.contains(" src/año.txt\n"), "{listing}");
        assert_eq!(Snapshot::parse_listing(&listing).unwrap(), snapshot);
    }

    #[test]
    fn content_mode_path_and_line_endings_each_change_the_identity() {
        let base = snapshot(&[("a.txt", "line\n", false)]);
        for other in [
            snapshot(&[("a.txt", "line\r\n", false)]),
            snapshot(&[("a.txt", "line\n", true)]),
            snapshot(&[("b.txt", "line\n", false)]),
            snapshot(&[("a.txt", "line\n", false), ("c.txt", "", false)]),
        ] {
            assert_ne!(other.identity(), base.identity(), "{}", other.listing());
        }
    }

    #[test]
    fn malformed_listings_and_identities_are_rejected() {
        assert_eq!(
            Snapshot::parse_listing("other\n"),
            Err(ListingError::Header)
        );
        let blob = BlobId::of(b"x");
        assert_eq!(
            Snapshot::parse_listing(&format!("assemblr-base-v1\n100600 {blob} a\n")),
            Err(ListingError::Line(2))
        );
        assert_eq!(
            Snapshot::parse_listing(&format!(
                "assemblr-base-v1\n100644 {blob} b\n100644 {blob} a\n"
            )),
            Err(ListingError::Order(3))
        );
        assert_eq!(
            Snapshot::parse_listing(&format!("assemblr-base-v1\n100644 {blob} a")),
            Err(ListingError::Line(2))
        );
        assert!(BaseIdentity::parse("sha1-v1:abc").is_none());
        assert!(BaseIdentity::parse(&format!("sha256-v1:{}", "A".repeat(64))).is_none());
        assert!(BlobId::parse(&"0".repeat(63)).is_none());
    }
}
