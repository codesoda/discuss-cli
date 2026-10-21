//! Maps the browser's comment-anchor index space onto markdown source lines,
//! and splices edited blocks back into the source.
//!
//! The renderer ([`crate::render`]) plans every commentable block once and
//! stamps its 1-based `data-anchor-idx` into the served HTML. Live block
//! editing needs the inverse view: for anchor index N, which source lines
//! produced that element? This module reads that same plan — so the map and
//! the stamps can never disagree — and adds the two pieces of line arithmetic
//! the editor needs: a byte-exact [`splice`] of one block's line range, and
//! [`carry_anchor_across_edit`], which moves comment anchors mechanically
//! after an edit changes the block count.

pub use crate::render::BlockKind;
use crate::render::render_markdown;

/// One commentable element and the source lines that produce it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnchorBlock {
    /// 1-based, equal to the element's `data-anchor-idx` in the browser.
    pub anchor_idx: usize,
    /// 1-based first source line (inclusive), in whole-file coordinates.
    pub line_start: usize,
    /// 1-based last source line (inclusive). An empty front-matter block has
    /// `line_end == line_start - 1`, meaning "insert before `line_start`".
    pub line_end: usize,
    pub kind: BlockKind,
}

/// The anchor-index → source-line map for one document.
#[derive(Clone, Debug, Default)]
pub struct BlockMap {
    pub blocks: Vec<AnchorBlock>,
}

impl BlockMap {
    pub fn get(&self, anchor_idx: usize) -> Option<&AnchorBlock> {
        self.blocks.get(anchor_idx.checked_sub(1)?)
    }

    /// How many consecutive anchors, starting at `anchor_idx`, live inside
    /// that block's source lines — the block itself plus any anchors nested
    /// in it (a table's rows). Editing the block replaces this whole run.
    pub fn span_len(&self, anchor_idx: usize) -> usize {
        let Some(block) = self.get(anchor_idx) else {
            return 0;
        };
        1 + self
            .blocks
            .iter()
            .skip(anchor_idx)
            .take_while(|nested| {
                nested.line_start >= block.line_start && nested.line_end <= block.line_end
            })
            .count()
    }
}

/// Build the anchor map for a document from the renderer's block plan.
pub fn anchor_blocks(markdown: &str) -> BlockMap {
    BlockMap {
        blocks: render_markdown(markdown)
            .blocks
            .into_iter()
            .map(|block| AnchorBlock {
                anchor_idx: block.index,
                line_start: block.line_start,
                line_end: block.line_end,
                kind: block.kind,
            })
            .collect(),
    }
}

/// Where a thread's anchor range lands after one block is edited.
///
/// `edited_idx` is the block that was replaced and `old_span_len` how many
/// anchors it covered before the edit (itself plus nested anchors, see
/// [`BlockMap::span_len`]). `delta` is the change in the document's total
/// anchor count and `new_total` the count afterwards.
///
/// Anchors before the edit stay put; anchors after it shift by `delta`; a
/// range touching the edited span stretches or shrinks with it; anchors
/// nested inside the span keep their index while it still exists. `None`
/// means the range can't be carried (its content is gone) and the thread
/// should orphan.
pub fn carry_anchor_across_edit(
    anchor_start: usize,
    anchor_end: usize,
    edited_idx: usize,
    old_span_len: usize,
    delta: isize,
    new_total: usize,
) -> Option<(usize, usize)> {
    let old_span_len = old_span_len.max(1);
    let span_end_old = edited_idx + old_span_len - 1;
    // A single-block edit can at most remove its own span. A more negative
    // delta means the replacement restructured *following* blocks too — say
    // an unterminated code fence swallowing the rest of the document — and
    // ranges touching or past the edit can't be carried mechanically. Ranges
    // fully before it are safe: a splice never changes earlier lines, and
    // markdown block structure never depends on later content.
    if delta < -(old_span_len as isize) && anchor_end >= edited_idx {
        return None;
    }
    let span_end_new = edited_idx as isize + old_span_len as isize + delta - 1;

    // Strictly nested in the edited span (a row of an edited table): keep the
    // index while the new span still reaches it.
    if anchor_start > edited_idx && anchor_end <= span_end_old {
        return (anchor_end as isize <= span_end_new).then_some((anchor_start, anchor_end));
    }

    let new_start = if anchor_start > span_end_old {
        anchor_start as isize + delta
    } else {
        anchor_start as isize
    };
    let new_end = if anchor_end > span_end_old {
        anchor_end as isize + delta
    } else if anchor_end == span_end_old {
        span_end_new
    } else if anchor_end >= edited_idx {
        (anchor_end as isize).min(span_end_new)
    } else {
        anchor_end as isize
    };
    if new_start < 1 || new_end < new_start || new_end > new_total as isize {
        return None;
    }
    Some((new_start as usize, new_end as usize))
}

/// The source text of one anchor block, without the final line's newline —
/// the shape an editor wants. Splicing the same text back (via [`splice`],
/// which restores the trailing newline) reproduces the document exactly.
pub fn block_source(markdown: &str, block: &AnchorBlock) -> String {
    if block.line_end < block.line_start {
        return String::new();
    }
    let text: String = markdown
        .split_inclusive('\n')
        .skip(block.line_start - 1)
        .take(block.line_end - block.line_start + 1)
        .collect();
    text.strip_suffix('\n').map(str::to_owned).unwrap_or(text)
}

/// Replace the inclusive 1-based line range `[line_start, line_end]` with
/// `replacement`, leaving every byte outside the range untouched.
///
/// `line_end == line_start - 1` inserts before `line_start` (the empty
/// front-matter case). A non-empty replacement gets a trailing newline if it
/// lacks one and more content follows, so the next block stays on its own
/// line.
pub fn splice(
    markdown: &str,
    line_start: usize,
    line_end: usize,
    replacement: &str,
) -> Result<String, SpliceError> {
    let mut line_starts = vec![0usize];
    for (offset, byte) in markdown.bytes().enumerate() {
        if byte == b'\n' {
            line_starts.push(offset + 1);
        }
    }
    // A trailing newline opens a phantom final entry; drop it so line_count
    // reflects real lines.
    if markdown.ends_with('\n') || markdown.is_empty() {
        line_starts.pop();
    }
    let line_count = line_starts.len();

    let out_of_range = line_start == 0
        || line_end.wrapping_add(1) < line_start // only start - 1 means insertion
        || line_start > line_count + 1
        || (line_end >= line_start && line_end > line_count);
    if out_of_range {
        return Err(SpliceError::OutOfRange {
            line_start,
            line_end,
            line_count,
        });
    }

    let byte_at = |line: usize| line_starts.get(line - 1).copied().unwrap_or(markdown.len());
    let prefix = &markdown[..byte_at(line_start)];
    let suffix = if line_end < line_start {
        &markdown[byte_at(line_start)..]
    } else {
        &markdown[byte_at(line_end + 1)..]
    };

    let mut result = String::with_capacity(prefix.len() + replacement.len() + suffix.len() + 1);
    result.push_str(prefix);
    result.push_str(replacement);
    if !replacement.is_empty()
        && !replacement.ends_with('\n')
        && (!suffix.is_empty() || markdown.ends_with('\n'))
    {
        result.push('\n');
    }
    result.push_str(suffix);
    Ok(result)
}

#[derive(Debug, Eq, PartialEq)]
pub enum SpliceError {
    OutOfRange {
        line_start: usize,
        line_end: usize,
        line_count: usize,
    },
}

impl std::fmt::Display for SpliceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpliceError::OutOfRange {
                line_start,
                line_end,
                line_count,
            } => write!(
                f,
                "line range {line_start}-{line_end} is outside the document ({line_count} lines)"
            ),
        }
    }
}

impl std::error::Error for SpliceError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::render;

    fn kinds(map: &BlockMap) -> Vec<BlockKind> {
        map.blocks.iter().map(|b| b.kind).collect()
    }

    fn ranges(map: &BlockMap) -> Vec<(usize, usize)> {
        map.blocks
            .iter()
            .map(|b| (b.line_start, b.line_end))
            .collect()
    }

    /// The source lines an anchor block spans, exactly as they appear.
    fn lines_for(markdown: &str, block: &AnchorBlock) -> String {
        if block.line_end < block.line_start {
            return String::new();
        }
        markdown
            .split_inclusive('\n')
            .skip(block.line_start - 1)
            .take(block.line_end - block.line_start + 1)
            .collect()
    }

    /// Every anchor the map produces must be exactly one stamped element in
    /// the rendered HTML, in the same order — the map is the plan, so this
    /// pins the two together.
    fn assert_map_matches_stamps(markdown: &str) {
        let map = anchor_blocks(markdown);
        let html = render(markdown);
        let stamped: Vec<usize> = html
            .match_indices("data-anchor-idx=\"")
            .map(|(at, marker)| {
                let rest = &html[at + marker.len()..];
                let end = rest.find('"').expect("closing quote");
                rest[..end].parse().expect("numeric anchor index")
            })
            .collect();
        let mut expected: Vec<usize> = map.blocks.iter().map(|b| b.anchor_idx).collect();
        let mut stamped_sorted = stamped.clone();
        stamped_sorted.sort_unstable();
        expected.sort_unstable();
        assert_eq!(
            stamped_sorted, expected,
            "map anchors must equal the stamped anchors\n{html}"
        );
    }

    #[test]
    fn headings_h1_to_h5_are_anchors_h6_is_not() {
        let md = "# a\n\n## b\n\n### c\n\n#### d\n\n##### e\n\n###### f\n";
        let map = anchor_blocks(md);

        assert_eq!(kinds(&map), vec![BlockKind::Heading; 5]);
        assert_eq!(ranges(&map), vec![(1, 1), (3, 3), (5, 5), (7, 7), (9, 9)]);
        assert_map_matches_stamps(md);
    }

    #[test]
    fn setext_heading_spans_both_lines() {
        let md = "Title\n=====\n\npara\n";
        let map = anchor_blocks(md);

        assert_eq!(kinds(&map), vec![BlockKind::Heading, BlockKind::Paragraph]);
        assert_eq!(ranges(&map), vec![(1, 2), (4, 4)]);
    }

    #[test]
    fn list_items_are_individual_anchors_and_nesting_stays_inside_its_item() {
        let md = "- one\n- two\n  - nested a\n  - nested b\n- three\n";
        let map = anchor_blocks(md);

        assert_eq!(kinds(&map), vec![BlockKind::ListItem; 3]);
        assert_eq!(ranges(&map), vec![(1, 1), (2, 4), (5, 5)]);
        assert_map_matches_stamps(md);
    }

    #[test]
    fn ordered_and_task_list_items_are_anchors() {
        let md = "1. first\n2. second\n\n- [x] done\n- [ ] todo\n";
        let map = anchor_blocks(md);

        assert_eq!(kinds(&map), vec![BlockKind::ListItem; 4]);
        assert_eq!(ranges(&map), vec![(1, 1), (2, 2), (4, 4), (5, 5)]);
    }

    #[test]
    fn blockquote_is_one_anchor_swallowing_its_children() {
        let md = "> quoted line\n> - inner item\n\nafter\n";
        let map = anchor_blocks(md);

        assert_eq!(
            kinds(&map),
            vec![BlockKind::BlockQuote, BlockKind::Paragraph]
        );
        assert_eq!(ranges(&map), vec![(1, 2), (4, 4)]);
    }

    #[test]
    fn fenced_indented_and_mermaid_code_blocks_are_anchors() {
        let md = "```rust\nfn main() {}\n```\n\n    indented\n\n```mermaid\ngraph TD\n```\n";
        let map = anchor_blocks(md);

        assert_eq!(kinds(&map), vec![BlockKind::CodeBlock; 3]);
        assert_eq!(ranges(&map), vec![(1, 3), (5, 5), (7, 9)]);
    }

    #[test]
    fn tables_anchor_as_a_whole_then_per_row_and_thematic_breaks_do_not() {
        let md = "before\n\n| a | b |\n| - | - |\n| c | d |\n| e | f |\n\n---\n\nafter\n";
        let map = anchor_blocks(md);

        assert_eq!(
            kinds(&map),
            vec![
                BlockKind::Paragraph,
                BlockKind::Table,
                BlockKind::TableRow,
                BlockKind::TableRow,
                BlockKind::TableRow,
                BlockKind::Paragraph,
            ]
        );
        assert_eq!(
            ranges(&map),
            vec![(1, 1), (3, 6), (3, 3), (5, 5), (6, 6), (10, 10)]
        );
        // The table owns its rows: editing anchor 2 replaces anchors 2..=5.
        assert_eq!(map.span_len(2), 4);
        assert_eq!(map.span_len(3), 1);
        assert_eq!(map.span_len(6), 1);
        assert_map_matches_stamps(md);
    }

    #[test]
    fn footnote_definitions_anchor_at_end_in_reference_order() {
        // beta is referenced before alpha, so the rendered footnotes section
        // lists beta first — the map must too, with sourcepos still pointing
        // at the original definition lines.
        let md = "\
[^alpha]: alpha detail

first uses beta[^beta] here

second uses alpha[^alpha] here

[^beta]: beta detail
";
        let map = anchor_blocks(md);

        assert_eq!(
            kinds(&map),
            vec![
                BlockKind::Paragraph,
                BlockKind::Paragraph,
                BlockKind::FootnoteDefinition,
                BlockKind::FootnoteDefinition,
            ]
        );
        // beta (line 7) precedes alpha (line 1) in the map.
        assert_eq!(ranges(&map), vec![(3, 3), (5, 5), (7, 7), (1, 1)]);
        assert_map_matches_stamps(md);
    }

    #[test]
    fn unreferenced_footnote_definition_renders_nothing_and_gets_no_anchor() {
        let md = "para\n\n[^orphan]: never referenced\n";
        let map = anchor_blocks(md);

        assert_eq!(kinds(&map), vec![BlockKind::Paragraph]);
    }

    #[test]
    fn frontmatter_owns_anchor_one_and_offsets_body_lines() {
        let md = "---\ntitle: Demo\nauthor: Chris\n---\n# Hello\n\npara\n";
        let map = anchor_blocks(md);

        assert_eq!(
            kinds(&map),
            vec![
                BlockKind::Frontmatter,
                BlockKind::Heading,
                BlockKind::Paragraph
            ]
        );
        assert_eq!(ranges(&map), vec![(2, 3), (5, 5), (7, 7)]);
        assert_map_matches_stamps(md);
    }

    #[test]
    fn empty_frontmatter_maps_to_insertion_range() {
        let md = "---\n---\n# Title\n";
        let map = anchor_blocks(md);

        assert_eq!(map.blocks[0].kind, BlockKind::Frontmatter);
        assert_eq!(map.blocks[0].line_start, 2);
        assert_eq!(map.blocks[0].line_end, 1);
        assert_eq!(map.blocks[1].kind, BlockKind::Heading);
        assert_eq!((map.blocks[1].line_start, map.blocks[1].line_end), (3, 3));
    }

    #[test]
    fn raw_html_blocks_and_comments_are_not_anchors() {
        let md = "<div class=\"quote\">\nstakeholder input\n</div>\n\n<!-- note -->\n\nafter\n";
        let map = anchor_blocks(md);

        assert_eq!(kinds(&map), vec![BlockKind::Paragraph]);
        assert_eq!(ranges(&map), vec![(7, 7)]);
        assert_map_matches_stamps(md);
    }

    #[test]
    fn empty_documents_have_no_anchors() {
        assert!(anchor_blocks("").blocks.is_empty());
        assert!(anchor_blocks("\n\n").blocks.is_empty());
    }

    /// One fixture exercising every block shape at once.
    const KITCHEN_SINK: &str = "\
---
title: Kitchen sink
---
# Heading

A paragraph
spanning two lines.

- item one
- item two
  with continuation

> a quote

```rust
fn main() {}
```

| a | b |
| - | - |
| 1 | 2 |

<div class=\"decision\">ship it</div>

uses a note[^n]

[^n]: the note
";

    #[test]
    fn kitchen_sink_map_matches_stamps() {
        assert_map_matches_stamps(KITCHEN_SINK);
        let map = anchor_blocks(KITCHEN_SINK);
        assert_eq!(
            kinds(&map),
            vec![
                BlockKind::Frontmatter,
                BlockKind::Heading,
                BlockKind::Paragraph,
                BlockKind::ListItem,
                BlockKind::ListItem,
                BlockKind::BlockQuote,
                BlockKind::CodeBlock,
                BlockKind::Table,
                BlockKind::TableRow,
                BlockKind::TableRow,
                BlockKind::Paragraph,
                BlockKind::FootnoteDefinition,
            ]
        );
    }

    #[test]
    fn identity_splice_is_byte_identical_for_every_anchor() {
        let map = anchor_blocks(KITCHEN_SINK);
        assert!(map.blocks.len() >= 9, "fixture should exercise many kinds");

        for block in &map.blocks {
            let original = lines_for(KITCHEN_SINK, block);
            let spliced = splice(KITCHEN_SINK, block.line_start, block.line_end, &original)
                .expect("identity splice should be in range");
            assert_eq!(
                spliced, KITCHEN_SINK,
                "identity splice of anchor {} ({:?}) must not change the document",
                block.anchor_idx, block.kind
            );
        }
    }

    #[test]
    fn block_source_roundtrips_through_splice_for_every_anchor() {
        let map = anchor_blocks(KITCHEN_SINK);
        for block in &map.blocks {
            let source = block_source(KITCHEN_SINK, block);
            assert!(!source.ends_with('\n'));
            let spliced = splice(KITCHEN_SINK, block.line_start, block.line_end, &source)
                .expect("identity splice should be in range");
            assert_eq!(
                spliced, KITCHEN_SINK,
                "block_source of anchor {} ({:?}) must splice back losslessly",
                block.anchor_idx, block.kind
            );
        }
    }

    #[test]
    fn identity_splice_renders_identically() {
        let map = anchor_blocks(KITCHEN_SINK);
        let block = map.get(4).expect("anchor 4 exists");
        let original = lines_for(KITCHEN_SINK, block);
        let spliced =
            splice(KITCHEN_SINK, block.line_start, block.line_end, &original).expect("in range");

        assert_eq!(render(&spliced), render(KITCHEN_SINK));
    }

    #[test]
    fn replacing_a_block_touches_only_its_lines() {
        let map = anchor_blocks(KITCHEN_SINK);
        // Anchor 3: the two-line paragraph (after frontmatter and heading).
        let block = *map.get(3).expect("anchor 3 exists");
        assert_eq!(block.kind, BlockKind::Paragraph);

        let edited =
            splice(KITCHEN_SINK, block.line_start, block.line_end, "Rewritten.").expect("in range");

        let before: String = KITCHEN_SINK
            .split_inclusive('\n')
            .take(block.line_start - 1)
            .collect();
        let after: String = KITCHEN_SINK
            .split_inclusive('\n')
            .skip(block.line_end)
            .collect();
        assert!(edited.starts_with(&before));
        assert!(edited.ends_with(&after));
        assert_eq!(edited, format!("{before}Rewritten.\n{after}"));
    }

    #[test]
    fn edit_that_splits_a_block_shifts_following_anchors_by_line_delta() {
        let map = anchor_blocks(KITCHEN_SINK);
        let block = *map.get(3).expect("anchor 3 exists");
        let old_len = map.blocks.len();

        // One paragraph becomes two: +1 anchor, +2 lines (para + blank).
        let edited = splice(
            KITCHEN_SINK,
            block.line_start,
            block.line_end,
            "First half.\n\nSecond half.",
        )
        .expect("in range");
        let new_map = anchor_blocks(&edited);

        assert_eq!(new_map.blocks.len(), old_len + 1);
        let old_next = map.get(4).expect("old anchor 4");
        let new_next = new_map.get(5).expect("shifted anchor 5");
        assert_eq!(old_next.kind, new_next.kind);
        assert_eq!(new_next.line_start, old_next.line_start + 1);
    }

    #[test]
    fn editing_a_table_row_touches_only_that_line() {
        let map = anchor_blocks(KITCHEN_SINK);
        let row = *map
            .blocks
            .iter()
            .find(|b| {
                b.kind == BlockKind::TableRow && b.line_start > map.get(8).unwrap().line_start
            })
            .expect("a body row");
        assert_eq!(block_source(KITCHEN_SINK, &row), "| 1 | 2 |");

        let edited =
            splice(KITCHEN_SINK, row.line_start, row.line_end, "| one | two |").expect("in range");
        let new_map = anchor_blocks(&edited);

        assert_eq!(new_map.blocks.len(), map.blocks.len());
        assert_eq!(kinds(&new_map), kinds(&map));
        assert!(edited.contains("| a | b |\n| - | - |\n| one | two |\n"));
    }

    #[test]
    fn deleting_a_block_with_empty_replacement_drops_its_lines() {
        let md = "one\n\ntwo\n\nthree\n";
        let map = anchor_blocks(md);
        let block = *map.get(2).expect("anchor 2 exists");

        let edited = splice(md, block.line_start, block.line_end, "").expect("in range");

        assert_eq!(edited, "one\n\n\nthree\n");
        let new_map = anchor_blocks(&edited);
        assert_eq!(new_map.blocks.len(), 2);
    }

    #[test]
    fn splice_adds_trailing_newline_when_replacement_lacks_one() {
        let edited = splice("a\n\nb\n", 1, 1, "changed").expect("in range");
        assert_eq!(edited, "changed\n\nb\n");
    }

    #[test]
    fn splice_preserves_missing_final_newline() {
        let edited = splice("a\n\nb", 3, 3, "c").expect("in range");
        assert_eq!(edited, "a\n\nc");
    }

    #[test]
    fn splice_insertion_range_inserts_before_start_line() {
        // The empty-frontmatter shape: line_end == line_start - 1.
        let edited = splice("---\n---\nbody\n", 2, 1, "title: x").expect("in range");
        assert_eq!(edited, "---\ntitle: x\n---\nbody\n");
    }

    #[test]
    fn splice_rejects_out_of_range() {
        assert!(matches!(
            splice("one\n", 3, 3, "x"),
            Err(SpliceError::OutOfRange { .. })
        ));
        assert!(matches!(
            splice("one\n", 0, 0, "x"),
            Err(SpliceError::OutOfRange { .. })
        ));
        assert!(matches!(
            splice("one\ntwo\n", 2, 4, "x"),
            Err(SpliceError::OutOfRange { .. })
        ));
        // end more than one below start is malformed, not an insertion
        assert!(matches!(
            splice("one\ntwo\n", 3, 1, "x"),
            Err(SpliceError::OutOfRange { .. })
        ));
    }

    #[test]
    fn splice_into_empty_document_appends() {
        assert_eq!(splice("", 1, 0, "hello").expect("in range"), "hello");
    }

    #[test]
    fn carry_anchor_leaves_ranges_before_the_edit_alone() {
        assert_eq!(carry_anchor_across_edit(1, 2, 3, 1, 1, 6), Some((1, 2)));
        assert_eq!(carry_anchor_across_edit(1, 2, 3, 1, -1, 4), Some((1, 2)));
    }

    #[test]
    fn carry_anchor_shifts_ranges_after_the_edit_by_delta() {
        assert_eq!(carry_anchor_across_edit(4, 5, 3, 1, 1, 6), Some((5, 6)));
        assert_eq!(carry_anchor_across_edit(4, 5, 3, 1, -1, 4), Some((3, 4)));
        assert_eq!(carry_anchor_across_edit(4, 5, 3, 1, 0, 5), Some((4, 5)));
    }

    #[test]
    fn carry_anchor_stretches_a_range_over_a_split_block() {
        // Point thread on the edited block: covers all replacement anchors.
        assert_eq!(carry_anchor_across_edit(3, 3, 3, 1, 2, 7), Some((3, 5)));
        // Range ending at the edited block stretches with it.
        assert_eq!(carry_anchor_across_edit(2, 3, 3, 1, 1, 6), Some((2, 4)));
        // Range starting at the edited block keeps its start.
        assert_eq!(carry_anchor_across_edit(3, 4, 3, 1, 1, 6), Some((3, 5)));
    }

    #[test]
    fn carry_anchor_orphans_a_point_thread_on_a_deleted_block() {
        assert_eq!(carry_anchor_across_edit(3, 3, 3, 1, -1, 4), None);
    }

    #[test]
    fn carry_anchor_shrinks_ranges_spanning_a_deleted_block() {
        assert_eq!(carry_anchor_across_edit(2, 3, 3, 1, -1, 4), Some((2, 2)));
        assert_eq!(carry_anchor_across_edit(3, 4, 3, 1, -1, 4), Some((3, 3)));
        assert_eq!(carry_anchor_across_edit(2, 4, 3, 1, -1, 4), Some((2, 3)));
    }

    #[test]
    fn carry_anchor_orphans_ranges_that_fall_off_the_document() {
        // Replacement swallowed following blocks (e.g. unterminated fence):
        // a thread whose range no longer fits the new total orphans instead
        // of drifting onto the wrong block.
        assert_eq!(carry_anchor_across_edit(4, 5, 3, 1, -3, 2), None);
        // Document emptied entirely.
        assert_eq!(carry_anchor_across_edit(1, 1, 1, 1, -1, 0), None);
    }

    #[test]
    fn carry_anchor_across_an_edited_table_keeps_its_rows_and_shifts_after() {
        // Table at 3 with header + 2 rows (span 4, anchors 3..=6), a
        // paragraph at 7. Adding one row: delta +1, new total 8.
        assert_eq!(carry_anchor_across_edit(3, 3, 3, 4, 1, 8), Some((3, 3)));
        assert_eq!(carry_anchor_across_edit(4, 4, 3, 4, 1, 8), Some((4, 4)));
        assert_eq!(carry_anchor_across_edit(6, 6, 3, 4, 1, 8), Some((6, 6)));
        assert_eq!(carry_anchor_across_edit(7, 7, 3, 4, 1, 8), Some((8, 8)));
        // A range from before the table to its last row stretches with it.
        assert_eq!(carry_anchor_across_edit(2, 6, 3, 4, 1, 8), Some((2, 7)));
    }

    #[test]
    fn carry_anchor_across_a_table_losing_rows_orphans_only_the_lost_rows() {
        // Same table, rewritten to header + 1 row: delta -1, new total 6.
        assert_eq!(carry_anchor_across_edit(3, 3, 3, 4, -1, 6), Some((3, 3)));
        assert_eq!(carry_anchor_across_edit(5, 5, 3, 4, -1, 6), Some((5, 5)));
        assert_eq!(carry_anchor_across_edit(6, 6, 3, 4, -1, 6), None);
        assert_eq!(carry_anchor_across_edit(7, 7, 3, 4, -1, 6), Some((6, 6)));
        // The table replaced by a paragraph: delta -3 (its whole span minus
        // the one new block), nested rows orphan, the table thread survives.
        assert_eq!(carry_anchor_across_edit(3, 3, 3, 4, -3, 4), Some((3, 3)));
        assert_eq!(carry_anchor_across_edit(4, 4, 3, 4, -3, 4), None);
        assert_eq!(carry_anchor_across_edit(7, 7, 3, 4, -3, 4), Some((4, 4)));
        // The table deleted outright: delta -4, everything in it orphans.
        assert_eq!(carry_anchor_across_edit(3, 3, 3, 4, -4, 3), None);
        assert_eq!(carry_anchor_across_edit(7, 7, 3, 4, -4, 3), Some((3, 3)));
    }
}
