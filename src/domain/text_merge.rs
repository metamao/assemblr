//! A conservative line-level three-way merge of text.
//!
//! Every side is compared line by line with the same base, which gives its
//! changes as *hunks*: a range of base lines replaced by other lines. Hunks
//! of different sides that overlap or touch form one *cluster*, and the
//! clusters are decided independently:
//!
//! - a cluster of one hunk, or of identical hunks, is that change;
//! - a cluster of insertions at one position keeps every distinct insertion
//!   when the caller asks to [`InsertTie::Chain`] them, in the order of the
//!   sides, and is a conflict otherwise;
//! - any other cluster is a conflict, which keeps what the base and each
//!   side hold there. Lines that every side agrees on at the start and at
//!   the end of a conflict are moved out of it, so that a conflict marks
//!   only the lines that differ.
//!
//! Nothing is dropped and no side takes precedence: a change either applies
//! as made or is reported. The result depends only on the texts and the
//! order of the sides, never on the order they were given in.
//!
//! The comparison keeps line terminators, so lines that differ only by a
//! terminator differ, and a last line without one is not the same line as
//! that line with one. Files are compared as given; a caller that wants
//! CRLF read as LF converts first.

use std::ops::Range;

/// Whether `bytes` are text the merge can read: UTF-8 without a NUL.
pub fn is_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

/// The lines of `text`, each with its terminator.
pub fn split_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// What to do with insertions by several sides at the same position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertTie {
    /// Keep each distinct insertion, in the order of the sides. The order
    /// is a layout, not a precedence: nothing is lost.
    Chain,
    /// Report them as a conflict.
    Conflict,
}

/// A range of base lines replaced by a range of a side's lines.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Hunk {
    base: Range<usize>,
    other: Range<usize>,
}

/// The largest comparison table, in cells. A middle section that would need
/// a bigger one is one hunk, which is coarse but never wrong: it can only
/// make more changes collide.
const TABLE_LIMIT: usize = 4_000_000;

/// The hunks that turn `base` into `other`, in order.
fn diff(base: &[&str], other: &[&str]) -> Vec<Hunk> {
    let shortest = base.len().min(other.len());
    let prefix = base
        .iter()
        .zip(other)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = base[prefix..]
        .iter()
        .rev()
        .zip(other[prefix..].iter().rev())
        .take(shortest - prefix)
        .take_while(|(left, right)| left == right)
        .count();
    let (base_end, other_end) = (base.len() - suffix, other.len() - suffix);
    let (middle_base, middle_other) = (&base[prefix..base_end], &other[prefix..other_end]);
    if middle_base.is_empty() && middle_other.is_empty() {
        return Vec::new();
    }
    let (rows, columns) = (middle_base.len() + 1, middle_other.len() + 1);
    if middle_base.is_empty() || middle_other.is_empty() || rows * columns > TABLE_LIMIT {
        return vec![Hunk {
            base: prefix..base_end,
            other: prefix..other_end,
        }];
    }
    // lcs[i * columns + j]: the longest common subsequence of
    // middle_base[i..] and middle_other[j..].
    let mut lcs = vec![0_u32; rows * columns];
    for i in (0..rows - 1).rev() {
        for j in (0..columns - 1).rev() {
            lcs[i * columns + j] = if middle_base[i] == middle_other[j] {
                lcs[(i + 1) * columns + j + 1] + 1
            } else {
                lcs[(i + 1) * columns + j].max(lcs[i * columns + j + 1])
            };
        }
    }
    let mut hunks = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut open: Option<(usize, usize)> = None;
    while i < rows - 1 || j < columns - 1 {
        if i < rows - 1 && j < columns - 1 && middle_base[i] == middle_other[j] {
            if let Some((base_start, other_start)) = open.take() {
                hunks.push(Hunk {
                    base: prefix + base_start..prefix + i,
                    other: prefix + other_start..prefix + j,
                });
            }
            i += 1;
            j += 1;
            continue;
        }
        open.get_or_insert((i, j));
        // Remove a base line first when that loses nothing, so that the
        // same inputs always align the same way.
        if j == columns - 1
            || (i < rows - 1 && lcs[(i + 1) * columns + j] >= lcs[i * columns + j + 1])
        {
            i += 1;
        } else {
            j += 1;
        }
    }
    if let Some((base_start, other_start)) = open {
        hunks.push(Hunk {
            base: prefix + base_start..prefix + i,
            other: prefix + other_start..prefix + j,
        });
    }
    hunks
}

/// One part of a merged text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment<'a, C> {
    /// Lines the merge decided.
    Text(Vec<&'a str>),
    /// Lines the sides disagree on.
    Conflict(Conflict<'a, C>),
}

/// Lines that the sides changed incompatibly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict<'a, C> {
    /// The base lines the changes cover, as indexes into the base's lines.
    pub base_lines: Range<usize>,
    /// What the base holds there.
    pub base: Vec<&'a str>,
    /// What each side holds in place of those lines, once the lines every
    /// side agrees on at both ends are taken out. A side that did not change
    /// them holds the base's. Ordered like the sides.
    pub sides: Vec<(C, Vec<&'a str>)>,
    /// For each side, in the same order, whether it changed the lines the
    /// conflict covers at all.
    pub changed: Vec<bool>,
}

/// Insertions by several sides at one position that were all kept, in the
/// order of the sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chained<C> {
    /// The position, as the number of base lines before it.
    pub after_line: usize,
    /// The sides whose distinct insertions were kept, in the order they
    /// stand in the result.
    pub sides: Vec<C>,
}

/// The result of merging the sides onto the base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextMerge<'a, C> {
    segments: Vec<Segment<'a, C>>,
    chained: Vec<Chained<C>>,
}

impl<'a, C> TextMerge<'a, C> {
    /// The merged text and its conflicts, in order.
    pub fn segments(&self) -> &[Segment<'a, C>] {
        &self.segments
    }

    /// The conflicts, in order.
    pub fn conflicts(&self) -> impl Iterator<Item = &Conflict<'a, C>> {
        self.segments.iter().filter_map(|segment| match segment {
            Segment::Conflict(conflict) => Some(conflict),
            Segment::Text(_) => None,
        })
    }

    /// The positions where insertions by several sides were chained, in
    /// order. Nothing was lost there, but their order is the sides' order
    /// and not something any side asked for.
    pub fn chained(&self) -> &[Chained<C>] {
        &self.chained
    }

    /// The merged text, or `None` when there is a conflict.
    pub fn text(&self) -> Option<String> {
        let mut text = String::new();
        for segment in &self.segments {
            match segment {
                Segment::Text(lines) => lines.iter().for_each(|line| text.push_str(line)),
                Segment::Conflict(_) => return None,
            }
        }
        Some(text)
    }
}

/// Merges the `sides`, each a text derived from `base`, onto `base`. The
/// sides are ordered by `C`, whatever order they are given in.
pub fn merge_text<'a, C: Clone + Ord>(
    base: &'a str,
    sides: &[(C, &'a str)],
    tie: InsertTie,
) -> TextMerge<'a, C> {
    let base_lines = split_lines(base);
    let mut ordered: Vec<&(C, &'a str)> = sides.iter().collect();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let side_lines: Vec<Vec<&'a str>> = ordered.iter().map(|side| split_lines(side.1)).collect();
    let hunks: Vec<Vec<Hunk>> = side_lines
        .iter()
        .map(|lines| diff(&base_lines, lines))
        .collect();
    let mut all: Vec<(usize, &Hunk)> = hunks
        .iter()
        .enumerate()
        .flat_map(|(side, hunks)| hunks.iter().map(move |hunk| (side, hunk)))
        .collect();
    all.sort_by_key(|(side, hunk)| (hunk.base.start, hunk.base.end, *side));

    let mut segments: Vec<Segment<'a, C>> = Vec::new();
    let mut chained: Vec<Chained<C>> = Vec::new();
    let mut text: Vec<&'a str> = Vec::new();
    let mut position = 0;
    let mut index = 0;
    while index < all.len() {
        let start = all[index].1.base.start;
        let mut end = all[index].1.base.end;
        let mut next = index + 1;
        // Touching hunks belong together: changes to adjacent lines are
        // not independent.
        while next < all.len() && all[next].1.base.start <= end {
            end = end.max(all[next].1.base.end);
            next += 1;
        }
        text.extend_from_slice(&base_lines[position..start]);
        let cluster = &all[index..next];
        let lines_of = |side: usize, hunk: &Hunk| &side_lines[side][hunk.other.clone()];
        let first = cluster[0];
        let identical = cluster.iter().all(|(side, hunk)| {
            hunk.base == first.1.base && lines_of(*side, hunk) == lines_of(first.0, first.1)
        });
        if cluster.len() == 1 || identical {
            text.extend_from_slice(lines_of(first.0, first.1));
        } else if tie == InsertTie::Chain && cluster.iter().all(|(_, hunk)| hunk.base.is_empty()) {
            let mut kept: Vec<&[&'a str]> = Vec::new();
            let mut by: Vec<C> = Vec::new();
            for (side, hunk) in cluster {
                let lines = lines_of(*side, hunk);
                if !kept.contains(&lines) {
                    kept.push(lines);
                    by.push(ordered[*side].0.clone());
                }
            }
            kept.into_iter()
                .for_each(|lines| text.extend_from_slice(lines));
            if by.len() > 1 {
                chained.push(Chained {
                    after_line: start,
                    sides: by,
                });
            }
        } else {
            let held: Vec<Vec<&'a str>> = (0..ordered.len())
                .map(|side| {
                    let mut lines: Vec<&'a str> = Vec::new();
                    let mut at = start;
                    for hunk in hunks[side]
                        .iter()
                        .filter(|hunk| start <= hunk.base.start && hunk.base.end <= end)
                    {
                        lines.extend_from_slice(&base_lines[at..hunk.base.start]);
                        lines.extend_from_slice(&side_lines[side][hunk.other.clone()]);
                        at = hunk.base.end;
                    }
                    lines.extend_from_slice(&base_lines[at..end]);
                    lines
                })
                .collect();
            let shortest = held.iter().map(Vec::len).min().unwrap_or(0);
            let prefix = (0..shortest)
                .take_while(|line| held.iter().all(|side| side[*line] == held[0][*line]))
                .count();
            let suffix = (0..shortest - prefix)
                .take_while(|back| {
                    held.iter().all(|side| {
                        side[side.len() - 1 - back] == held[0][held[0].len() - 1 - back]
                    })
                })
                .count();
            text.extend_from_slice(&held[0][..prefix]);
            let inner = |side: &Vec<&'a str>| side[prefix..side.len() - suffix].to_vec();
            let changed: Vec<bool> = held
                .iter()
                .map(|side| side.as_slice() != &base_lines[start..end])
                .collect();
            if held[0].len() == prefix + suffix
                && held.iter().all(|side| side.len() == prefix + suffix)
            {
                // Every side holds the same lines after all.
                text.extend_from_slice(&held[0][prefix..held[0].len() - suffix]);
            } else {
                if !text.is_empty() {
                    segments.push(Segment::Text(std::mem::take(&mut text)));
                }
                segments.push(Segment::Conflict(Conflict {
                    base_lines: start..end,
                    base: base_lines[start..end].to_vec(),
                    sides: ordered
                        .iter()
                        .zip(&held)
                        .map(|(side, lines)| (side.0.clone(), inner(lines)))
                        .collect(),
                    changed,
                }));
            }
            text.extend_from_slice(&held[0][held[0].len() - suffix..]);
        }
        position = end;
        index = next;
    }
    text.extend_from_slice(&base_lines[position..]);
    if !text.is_empty() || segments.is_empty() {
        segments.push(Segment::Text(text));
    }
    TextMerge { segments, chained }
}

#[cfg(test)]
mod tests {
    use super::{InsertTie, Segment, merge_text, split_lines};

    fn merged(base: &str, sides: &[(&'static str, &str)], tie: InsertTie) -> Option<String> {
        merge_text(base, sides, tie).text()
    }

    const BASE: &str = "a\nb\nc\nd\ne\nf\ng\n";

    #[test]
    fn a_single_side_comes_back_exactly() {
        for side in [
            "",
            "a\n",
            "x\nb\nc\nd\ne\nf\ng\n",
            "a\nb\nc\nd\ne\nf\n",
            "a\nb\nc\nd\ne\nf\ng\nh\n",
            "a\nb\nc\nd\ne\nf\ng",
            "g\nf\ne\nd\nc\nb\na\n",
            "z\n",
        ] {
            assert_eq!(
                merged(BASE, &[("one", side)], InsertTie::Chain).as_deref(),
                Some(side),
                "{side:?}"
            );
        }
    }

    #[test]
    fn changes_in_different_places_are_both_kept() {
        let top = "A\nb\nc\nd\ne\nf\ng\n";
        let bottom = "a\nb\nc\nd\ne\nf\nG\n";
        assert_eq!(
            merged(BASE, &[("top", top), ("bottom", bottom)], InsertTie::Chain).as_deref(),
            Some("A\nb\nc\nd\ne\nf\nG\n")
        );
    }

    #[test]
    fn the_order_the_sides_are_given_in_does_not_matter() {
        let one = "a\nb\nONE\nd\ne\nf\ng\n";
        let two = "a\nb\nc\nd\ne\nTWO\ng\n";
        let three = "a\nb\nc\nd\ne\nf\ng\nTHREE\n";
        let sides = [("a", one), ("b", two), ("c", three)];
        let expected = merged(BASE, &sides, InsertTie::Chain);
        assert_eq!(
            expected.as_deref(),
            Some("a\nb\nONE\nd\ne\nTWO\ng\nTHREE\n")
        );
        for order in [[0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]] {
            let shuffled: Vec<_> = order.iter().map(|index| sides[*index]).collect();
            assert_eq!(merged(BASE, &shuffled, InsertTie::Chain), expected);
        }
    }

    #[test]
    fn identical_changes_are_one_change() {
        let edit = "a\nb\nX\nd\ne\nf\ng\n";
        assert_eq!(
            merged(BASE, &[("one", edit), ("two", edit)], InsertTie::Conflict).as_deref(),
            Some(edit)
        );
    }

    #[test]
    fn different_changes_to_one_line_conflict_and_keep_each_side() {
        let merge = merge_text(
            BASE,
            &[
                ("one", "a\nb\nX\nd\ne\nf\ng\n"),
                ("two", "a\nb\nY\nd\ne\nf\ng\n"),
            ],
            InsertTie::Chain,
        );
        assert_eq!(merge.text(), None);
        let conflict = merge.conflicts().next().unwrap();
        assert_eq!(conflict.base_lines, 2..3);
        assert_eq!(conflict.base, ["c\n"]);
        assert_eq!(conflict.sides[0], ("one", vec!["X\n"]));
        assert_eq!(conflict.sides[1], ("two", vec!["Y\n"]));
        // The lines around it are resolved text.
        assert!(matches!(&merge.segments()[0], Segment::Text(lines) if lines == &["a\n", "b\n"]));
    }

    #[test]
    fn a_side_that_left_the_lines_alone_is_not_marked_as_changed() {
        let merge = merge_text(
            BASE,
            &[
                ("one", "a\nb\nX\nd\ne\nf\ng\n"),
                ("two", "a\nb\nY\nd\ne\nf\ng\n"),
                ("three", BASE),
            ],
            InsertTie::Chain,
        );
        let conflict = merge.conflicts().next().unwrap();
        let sides: Vec<&str> = conflict.sides.iter().map(|(side, _)| *side).collect();
        assert_eq!(sides, ["one", "three", "two"]);
        assert_eq!(conflict.changed, [true, false, true]);
    }

    #[test]
    fn changes_to_adjacent_lines_conflict() {
        let merge = merge_text(
            BASE,
            &[
                ("one", "a\nb\nX\nd\ne\nf\ng\n"),
                ("two", "a\nb\nc\nY\ne\nf\ng\n"),
            ],
            InsertTie::Chain,
        );
        assert_eq!(merge.conflicts().count(), 1);
        let conflict = merge.conflicts().next().unwrap();
        assert_eq!(conflict.base_lines, 2..4);
        assert_eq!(conflict.sides[0].1, ["X\n", "d\n"]);
        assert_eq!(conflict.sides[1].1, ["c\n", "Y\n"]);
    }

    #[test]
    fn insertions_at_one_position_chain_in_side_order_or_conflict() {
        let one = "a\nb\nc\nd\ne\nf\ng\nONE\n";
        let two = "a\nb\nc\nd\ne\nf\ng\nTWO\n";
        assert_eq!(
            merged(BASE, &[("two", two), ("one", one)], InsertTie::Chain).as_deref(),
            Some("a\nb\nc\nd\ne\nf\ng\nONE\nTWO\n")
        );
        let merge = merge_text(BASE, &[("one", one), ("two", two)], InsertTie::Conflict);
        let conflict = merge.conflicts().next().unwrap();
        assert_eq!(conflict.base_lines, 7..7);
        assert_eq!(conflict.sides[0].1, ["ONE\n"]);
        assert_eq!(conflict.sides[1].1, ["TWO\n"]);
    }

    #[test]
    fn chained_insertions_are_reported_with_their_position_and_sides() {
        let one = "a\nb\nc\nd\ne\nf\ng\nONE\n";
        let two = "a\nb\nc\nd\ne\nf\ng\nTWO\n";
        let merge = merge_text(BASE, &[("two", two), ("one", one)], InsertTie::Chain);
        assert_eq!(merge.chained().len(), 1);
        assert_eq!(merge.chained()[0].after_line, 7);
        assert_eq!(merge.chained()[0].sides, ["one", "two"]);
        // One side's insertion, or the same insertion twice, chains nothing.
        let same = merge_text(BASE, &[("a", one), ("b", one)], InsertTie::Chain);
        assert!(same.chained().is_empty());
    }

    #[test]
    fn the_same_insertion_by_several_sides_is_kept_once() {
        let same = "a\nb\nc\nd\ne\nf\ng\nSAME\n";
        let other = "a\nb\nc\nd\ne\nf\ng\nOTHER\n";
        assert_eq!(
            merged(
                BASE,
                &[("a", same), ("b", other), ("c", same)],
                InsertTie::Chain
            )
            .as_deref(),
            Some("a\nb\nc\nd\ne\nf\ng\nSAME\nOTHER\n")
        );
    }

    #[test]
    fn an_insertion_touching_a_replacement_conflicts() {
        let merge = merge_text(
            BASE,
            &[
                ("one", "a\nb\nX\nd\ne\nf\ng\n"),
                ("two", "a\nb\nNEW\nc\nd\ne\nf\ng\n"),
            ],
            InsertTie::Chain,
        );
        assert_eq!(merge.conflicts().count(), 1);
    }

    #[test]
    fn lines_every_side_agrees_on_leave_the_conflict() {
        // Both replace c..e, starting with the same line and ending with the
        // same line, and differ only in the middle.
        let merge = merge_text(
            BASE,
            &[
                ("one", "a\nb\nS\none\nE\nf\ng\n"),
                ("two", "a\nb\nS\ntwo\nE\nf\ng\n"),
            ],
            InsertTie::Chain,
        );
        let conflict = merge.conflicts().next().unwrap();
        assert_eq!(conflict.sides[0].1, ["one\n"]);
        assert_eq!(conflict.sides[1].1, ["two\n"]);
        assert_eq!(conflict.base, ["c\n", "d\n", "e\n"]);
        assert!(matches!(&merge.segments()[0], Segment::Text(lines) if lines.ends_with(&["S\n"])));
        assert!(matches!(&merge.segments()[2], Segment::Text(lines) if lines[0] == "E\n"));
    }

    #[test]
    fn a_last_line_without_a_terminator_is_its_own_line() {
        let base = "a\nb";
        assert_eq!(
            merged(base, &[("one", "a\nb"), ("two", "A\nb")], InsertTie::Chain).as_deref(),
            Some("A\nb")
        );
        // Appending after it changes that line, so it collides with another
        // change to it.
        let merge = merge_text(
            base,
            &[("one", "a\nb\nc\n"), ("two", "a\nB")],
            InsertTie::Chain,
        );
        assert_eq!(merge.conflicts().count(), 1);
    }

    #[test]
    fn deleting_and_inserting_elsewhere_merge() {
        let deleted = "a\nb\nd\ne\nf\ng\n";
        let inserted = "a\nb\nc\nd\ne\nf\nNEW\ng\n";
        assert_eq!(
            merged(
                BASE,
                &[("del", deleted), ("ins", inserted)],
                InsertTie::Chain
            )
            .as_deref(),
            Some("a\nb\nd\ne\nf\nNEW\ng\n")
        );
    }

    #[test]
    fn a_line_in_the_middle_is_found_among_repeated_lines() {
        let base = "x\n\ny\n\nz\n";
        let one = "x\n\ny\n\nz\n\nNEW1\n";
        let two = "x\n\nNEW2\n\ny\n\nz\n";
        assert_eq!(
            merged(base, &[("one", one), ("two", two)], InsertTie::Chain).as_deref(),
            Some("x\n\nNEW2\n\ny\n\nz\n\nNEW1\n")
        );
    }

    #[test]
    fn lines_keep_their_terminators() {
        assert_eq!(split_lines("a\r\nb\n\nc"), ["a\r\n", "b\n", "\n", "c"]);
        assert!(split_lines("").is_empty());
    }

    #[test]
    fn a_huge_change_is_one_coarse_hunk_rather_than_a_slow_comparison() {
        let base: String = (0..3000).map(|n| format!("line {n}\n")).collect();
        let changed: String = (0..3000).map(|n| format!("changed {n}\n")).collect();
        assert_eq!(
            merged(&base, &[("one", &changed)], InsertTie::Chain).as_deref(),
            Some(changed.as_str())
        );
    }

    /// A small deterministic generator, so that the cases never change.
    struct Random(u64);

    impl Random {
        fn below(&mut self, limit: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            usize::try_from(self.0 % limit as u64).unwrap()
        }

        fn text(&mut self, lines: usize) -> String {
            // A small alphabet makes repeated lines, the hard case.
            (0..lines)
                .map(|_| format!("{}\n", ["a", "b", "c", "", "d"][self.below(5)]))
                .collect()
        }

        fn edit(&mut self, text: &str) -> String {
            let mut lines: Vec<String> =
                split_lines(text).iter().map(|l| (*l).to_owned()).collect();
            for _ in 0..=self.below(3) {
                let at = self.below(lines.len() + 1);
                match self.below(3) {
                    0 if at < lines.len() => {
                        lines.remove(at);
                    }
                    1 => lines.insert(at, format!("new{}\n", self.below(3))),
                    _ if at < lines.len() => lines[at] = format!("edit{}\n", self.below(3)),
                    _ => {}
                }
            }
            lines.concat()
        }
    }

    #[test]
    fn random_texts_keep_the_properties_the_merge_promises() {
        let mut random = Random(0x9E37_79B9_7F4A_7C15);
        for _ in 0..2000 {
            let lines = random.below(12);
            let base = random.text(lines);
            let (a, b) = (random.edit(&base), random.edit(&base));
            // One side alone is returned exactly, whatever the tie rule.
            for tie in [InsertTie::Chain, InsertTie::Conflict] {
                assert_eq!(
                    merged(&base, &[("a", &a)], tie).as_deref(),
                    Some(a.as_str())
                );
                // A side that changed nothing never gets in the way, and
                // the same change twice is that change.
                assert_eq!(
                    merged(&base, &[("a", &a), ("b", &base)], tie).as_deref(),
                    Some(a.as_str())
                );
                assert_eq!(
                    merged(&base, &[("a", &a), ("b", &a)], tie).as_deref(),
                    Some(a.as_str())
                );
                // The order the sides are given in changes nothing.
                assert_eq!(
                    merge_text(&base, &[("a", &a), ("b", &b)], tie),
                    merge_text(&base, &[("b", &b), ("a", &a)], tie),
                    "{base:?} {a:?} {b:?}"
                );
            }
        }
    }
}
