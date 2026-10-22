//! Block-level diff for held-back rewrites.
//!
//! A proposal is a whole new document. To let the reviewer accept some of it,
//! both versions are cut into *units* — one per top-level anchor block, each
//! owning its source lines up to the next block — and lined up with a longest
//! common subsequence. Runs of units that don't match become *hunks*.
//! [`BlockDiff::compose`] rebuilds a document taking the new text for
//! accepted hunks and the old text everywhere else, and reports where every
//! old anchor index lands so comment threads can follow.

use crate::block_map::anchor_blocks;

#[derive(Clone, Debug)]
struct Unit {
    /// Exact source bytes, including trailing blank lines.
    text: String,
    /// 1-based anchor index of the unit's block (0 when it has none).
    first_anchor: usize,
    /// The block plus any anchors nested in it (a table's rows).
    anchor_count: usize,
}

impl Unit {
    fn key(&self) -> &str {
        self.text.trim()
    }
}

/// One run of changed units: old units `old` replaced by new units `new`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// 1-based, stable for the life of the proposal.
    pub id: usize,
    /// Old anchor range the hunk replaces; `None` for a pure insertion.
    pub old_anchors: Option<(usize, usize)>,
    /// For a pure insertion, the old anchor it lands after (0 = top of doc).
    pub insert_after: usize,
    pub old_markdown: String,
    pub new_markdown: String,
}

#[derive(Clone, Debug)]
enum Op {
    Same {
        old: usize,
        new: usize,
    },
    Change {
        hunk: usize,
        old: (usize, usize),
        new: (usize, usize),
    },
}

#[derive(Clone, Debug)]
pub struct BlockDiff {
    old: Vec<Unit>,
    new: Vec<Unit>,
    ops: Vec<Op>,
    pub hunks: Vec<Hunk>,
}

/// The document built from a set of accepted hunks.
#[derive(Clone, Debug)]
pub struct Composed {
    pub markdown: String,
    /// `anchor_map[i]` is where old anchor `i + 1` lands, or `None` when its
    /// block was removed by an accepted hunk.
    pub anchor_map: Vec<Option<usize>>,
}

impl Composed {
    /// Carry an old anchor range into the composed document. `None` means
    /// the start's content is gone and the thread should orphan.
    pub fn carry(&self, start: usize, end: usize) -> Option<(usize, usize)> {
        let new_start = (*self.anchor_map.get(start.checked_sub(1)?)?)?;
        let new_end = end
            .checked_sub(1)
            .and_then(|i| self.anchor_map.get(i).copied().flatten())
            .unwrap_or(new_start);
        Some((new_start, new_end.max(new_start)))
    }
}

fn units(markdown: &str) -> Vec<Unit> {
    let map = anchor_blocks(markdown);
    // Top-level blocks: skip anchors nested inside the previous top block.
    let mut tops: Vec<(usize, usize, usize)> = Vec::new(); // (anchor, line_start, count)
    let mut top_end = 0usize;
    for block in &map.blocks {
        if let Some(last) = tops.last_mut()
            && block.line_start <= top_end
            && block.line_end <= top_end
        {
            last.2 += 1;
            continue;
        }
        top_end = block.line_end.max(block.line_start);
        tops.push((block.anchor_idx, block.line_start.max(1), 1));
    }

    let lines: Vec<&str> = markdown.split_inclusive('\n').collect();
    if tops.is_empty() {
        return vec![Unit {
            text: markdown.to_string(),
            first_anchor: 0,
            anchor_count: 0,
        }];
    }
    let mut out = Vec::with_capacity(tops.len());
    for (i, &(anchor, line_start, count)) in tops.iter().enumerate() {
        // The first unit also owns any lines before the first block.
        let from = if i == 0 { 0 } else { line_start - 1 };
        let to = tops
            .get(i + 1)
            .map_or(lines.len(), |next| next.1 - 1)
            .min(lines.len());
        out.push(Unit {
            text: lines[from.min(to)..to].concat(),
            first_anchor: anchor,
            anchor_count: count,
        });
    }
    out
}

/// Longest common subsequence over unit keys, as matched index pairs.
fn lcs(old: &[Unit], new: &[Unit]) -> Vec<(usize, usize)> {
    let (n, m) = (old.len(), new.len());
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if old[i].key() == new[j].key() {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut pairs) = (0, 0, Vec::new());
    while i < n && j < m {
        if old[i].key() == new[j].key() {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

fn join(units: &[Unit]) -> String {
    let mut out = String::new();
    for unit in units {
        push_unit(&mut out, &unit.text);
    }
    out
}

fn push_unit(out: &mut String, text: &str) {
    if !out.is_empty() && !out.ends_with('\n') && !text.is_empty() {
        out.push('\n');
    }
    out.push_str(text);
}

impl BlockDiff {
    pub fn new(old_markdown: &str, new_markdown: &str) -> Self {
        let old = units(old_markdown);
        let new = units(new_markdown);
        let pairs = lcs(&old, &new);

        let mut ops = Vec::new();
        let mut hunks = Vec::new();
        let (mut i, mut j) = (0, 0);
        let mut last_old_anchor = 0usize;
        for &(pi, pj) in pairs.iter().chain(std::iter::once(&(old.len(), new.len()))) {
            if i < pi || j < pj {
                let id = hunks.len() + 1;
                let old_slice = &old[i..pi];
                let anchored: Vec<&Unit> =
                    old_slice.iter().filter(|u| u.anchor_count > 0).collect();
                let old_anchors = match (anchored.first(), anchored.last()) {
                    (Some(first), Some(last)) => Some((
                        first.first_anchor,
                        last.first_anchor + last.anchor_count - 1,
                    )),
                    _ => None,
                };
                hunks.push(Hunk {
                    id,
                    old_anchors,
                    insert_after: last_old_anchor,
                    old_markdown: join(old_slice),
                    new_markdown: join(&new[j..pj]),
                });
                ops.push(Op::Change {
                    hunk: id,
                    old: (i, pi),
                    new: (j, pj),
                });
                if let Some((_, end)) = old_anchors {
                    last_old_anchor = end;
                }
            }
            if pi < old.len() {
                ops.push(Op::Same { old: pi, new: pj });
                let unit = &old[pi];
                if unit.anchor_count > 0 {
                    last_old_anchor = unit.first_anchor + unit.anchor_count - 1;
                }
            }
            i = pi + 1;
            j = pj + 1;
        }
        Self {
            old,
            new,
            ops,
            hunks,
        }
    }

    /// Rebuild the document with only the hunks in `accepted` applied.
    pub fn compose(&self, accepted: impl Fn(usize) -> bool) -> Composed {
        let old_total = self
            .old
            .last()
            .map_or(0, |u| u.first_anchor + u.anchor_count.saturating_sub(1));
        let mut anchor_map = vec![None; old_total];
        let mut markdown = String::new();
        let mut next_anchor = 1usize;

        let keep_old = |unit: &Unit,
                        text: &str,
                        markdown: &mut String,
                        next: &mut usize,
                        map: &mut Vec<Option<usize>>| {
            push_unit(markdown, text);
            for offset in 0..unit.anchor_count {
                if let Some(slot) = map.get_mut(unit.first_anchor + offset - 1) {
                    *slot = Some(*next + offset);
                }
            }
            *next += unit.anchor_count;
        };

        for (index, op) in self.ops.iter().enumerate() {
            match *op {
                Op::Same { old, new } => {
                    // Matching blocks can differ only in trailing blank
                    // lines, which separate them from the next block: take
                    // them from whichever version that next block comes from.
                    let next_is_old = matches!(
                        self.ops.get(index + 1),
                        Some(Op::Change { hunk, .. }) if !accepted(*hunk)
                    );
                    let text = if next_is_old {
                        &self.old[old].text
                    } else {
                        &self.new[new].text
                    };
                    keep_old(
                        &self.old[old],
                        text,
                        &mut markdown,
                        &mut next_anchor,
                        &mut anchor_map,
                    );
                }
                Op::Change { hunk, old, new } if accepted(hunk) => {
                    let new_units = &self.new[new.0..new.1];
                    let added: usize = new_units.iter().map(|u| u.anchor_count).sum();
                    // Threads on replaced blocks move to the first new block.
                    let landing = (added > 0).then_some(next_anchor);
                    for unit in &self.old[old.0..old.1] {
                        for offset in 0..unit.anchor_count {
                            if let Some(slot) = anchor_map.get_mut(unit.first_anchor + offset - 1) {
                                *slot = landing;
                            }
                        }
                    }
                    for unit in new_units {
                        push_unit(&mut markdown, &unit.text);
                    }
                    next_anchor += added;
                }
                Op::Change { old, .. } => {
                    for unit in &self.old[old.0..old.1] {
                        keep_old(
                            unit,
                            &unit.text,
                            &mut markdown,
                            &mut next_anchor,
                            &mut anchor_map,
                        );
                    }
                }
            }
        }
        Composed {
            markdown,
            anchor_map,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "# Title\n\nFirst para.\n\nSecond para.\n\n- one\n- two\n\nLast para.\n";

    #[test]
    fn identical_documents_have_no_hunks() {
        let diff = BlockDiff::new(OLD, OLD);
        assert!(diff.hunks.is_empty());
        assert_eq!(diff.compose(|_| true).markdown, OLD);
    }

    #[test]
    fn finds_changed_inserted_and_deleted_blocks() {
        let new =
            "# Title\n\nFirst para, shorter.\n\nSecond para.\n\n- one\n\nNew para.\n\nLast para.\n";
        let diff = BlockDiff::new(OLD, new);
        let summary: Vec<_> = diff
            .hunks
            .iter()
            .map(|h| (h.old_anchors, h.old_markdown.trim(), h.new_markdown.trim()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (Some((2, 2)), "First para.", "First para, shorter."),
                (Some((5, 5)), "- two", "New para."),
            ]
        );
    }

    #[test]
    fn compose_all_reproduces_the_new_document() {
        let new = "# Retitled\n\nFirst para.\n\nInserted.\n\n- one\n- two\n- three\n\nLast para.\n";
        let diff = BlockDiff::new(OLD, new);
        assert_eq!(diff.compose(|_| true).markdown, new);
    }

    #[test]
    fn partial_accept_applies_only_chosen_hunks_and_maps_anchors() {
        let new = "# Retitled\n\nFirst para.\n\nSecond para.\n\n- one\n- two\n\n";
        let diff = BlockDiff::new(OLD, new);
        assert_eq!(diff.hunks.len(), 2);
        let composed = diff.compose(|id| id == 2);
        assert_eq!(
            composed.markdown,
            "# Title\n\nFirst para.\n\nSecond para.\n\n- one\n- two\n\n"
        );
        // "Last para." (anchor 6) is gone; the rest keep their places.
        assert_eq!(
            composed.anchor_map,
            vec![Some(1), Some(2), Some(3), Some(4), Some(5), None]
        );
        assert_eq!(composed.carry(6, 6), None);
        assert_eq!(composed.carry(5, 6), Some((5, 5)));
    }

    #[test]
    fn anchors_after_an_insertion_shift_down() {
        let new =
            "# Title\n\nBrand new.\n\nFirst para.\n\nSecond para.\n\n- one\n- two\n\nLast para.\n";
        let diff = BlockDiff::new(OLD, new);
        assert_eq!(diff.hunks[0].old_anchors, None);
        assert_eq!(diff.hunks[0].insert_after, 1);
        let composed = diff.compose(|_| true);
        assert_eq!(composed.carry(2, 2), Some((3, 3)));
        assert_eq!(composed.carry(1, 1), Some((1, 1)));
    }

    #[test]
    fn tables_count_their_rows_as_one_unit() {
        let old = "Intro.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nOutro.\n";
        let new = "Intro.\n\n| a | b |\n|---|---|\n| 1 | 3 |\n\nOutro.\n";
        let diff = BlockDiff::new(old, new);
        assert_eq!(diff.hunks.len(), 1);
        let composed = diff.compose(|_| true);
        assert_eq!(composed.markdown, new);
        let outro_old = composed.anchor_map.len();
        assert_eq!(
            composed.carry(outro_old, outro_old),
            Some((outro_old, outro_old))
        );
    }
}
