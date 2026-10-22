//! Reconciling a text file line by line (see
//! `docs/adr/0022-text-reconciliation.md`).
//!
//! The project's file and the composition's are both derived from the
//! stored base, and are merged with the domain's line-level three-way merge.
//! Neither side is placed before the other where both insert at one position,
//! so that is a conflict, unlike the merge of several units' amendments.
//!
//! Line endings: a project file with CRLF line endings over a base and a
//! composition with LF ones is compared as LF, and what is written back has
//! CRLF line endings again, as it does for a file the project did not edit.

use std::borrow::Cow;

use crate::domain::{Contributor, InsertTie, TextMerge, merge_text};

/// The three versions of a file as text, with the project's file read as LF
/// when only it has CRLF line endings.
pub(super) struct Versions<'a> {
    pub(super) base: &'a str,
    pub(super) current: Cow<'a, str>,
    pub(super) incoming: &'a str,
    /// Whether `current` was read as LF and the result is written with CRLF.
    pub(super) crlf: bool,
}

impl<'a> Versions<'a> {
    pub(super) fn new(base: &'a str, current: &'a str, incoming: &'a str) -> Self {
        let crlf = current.contains("\r\n") && !base.contains("\r\n") && !incoming.contains("\r\n");
        Self {
            base,
            current: if crlf {
                Cow::Owned(current.replace("\r\n", "\n"))
            } else {
                Cow::Borrowed(current)
            },
            incoming,
            crlf,
        }
    }

    /// The merge of the project's lines and the composition's.
    pub(super) fn merge(&self) -> TextMerge<'_, Contributor> {
        merge_text(
            self.base,
            &[
                (Contributor::Current, self.current.as_ref()),
                (Contributor::Incoming, self.incoming),
            ],
            InsertTie::Conflict,
        )
    }

    /// `text` as the file is written: with CRLF line endings when the
    /// project's file has them over an LF base.
    pub(super) fn written(&self, text: String) -> String {
        if self.crlf {
            text.replace('\n', "\r\n")
        } else {
            text
        }
    }
}

/// The merged text, or the number of places where the project's lines and
/// the composition's cannot be merged.
pub(super) fn reconcile(base: &str, current: &str, incoming: &str) -> Result<String, usize> {
    let versions = Versions::new(base, current, incoming);
    let merge = versions.merge();
    match merge.text() {
        Some(text) => Ok(versions.written(text)),
        None => Err(merge.conflicts().count()),
    }
}

#[cfg(test)]
mod tests {
    use super::reconcile;

    const BASE: &str = "a\nb\nc\nd\ne\nf\ng\n";

    #[test]
    fn changes_in_different_places_are_both_kept() {
        let current = "a\nb\nc\nd\ne\nf\nmine\n";
        let incoming = "theirs\nb\nc\nd\ne\nf\ng\n";
        assert_eq!(
            reconcile(BASE, current, incoming).as_deref(),
            Ok("theirs\nb\nc\nd\ne\nf\nmine\n")
        );
    }

    #[test]
    fn insertions_at_one_position_are_a_conflict_whoever_made_them() {
        let current = "a\nb\nc\nd\ne\nf\ng\nmine\n";
        let incoming = "a\nb\nc\nd\ne\nf\ng\ntheirs\n";
        assert_eq!(reconcile(BASE, current, incoming), Err(1));
    }

    #[test]
    fn a_crlf_project_file_is_merged_as_lf_and_written_with_crlf() {
        let current = "a\r\nb\r\nc\r\nd\r\ne\r\nf\r\nmine\r\n";
        let incoming = "theirs\nb\nc\nd\ne\nf\ng\n";
        assert_eq!(
            reconcile(BASE, current, incoming).as_deref(),
            Ok("theirs\r\nb\r\nc\r\nd\r\ne\r\nf\r\nmine\r\n")
        );
    }

    #[test]
    fn overlapping_changes_count_their_places() {
        let current = "1\nb\nc\nd\ne\nf\n7\n";
        let incoming = "x\nb\nc\nd\ne\nf\ny\n";
        assert_eq!(reconcile(BASE, current, incoming), Err(2));
    }
}
