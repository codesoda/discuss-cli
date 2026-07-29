//! Maps the browser's comment-anchor index space onto markdown source lines.
//!
//! `discuss.html` assigns 1-based `data-anchor-idx` values to the outermost
//! "commentable" elements inside `#doc-content` (its `COMMENTABLE_SELECTOR`:
//! `h1..h5, p, li, blockquote, .pre-wrap, .quote, .decision, .callout,
//! .context`, filtered to elements not nested inside another match). Live
//! block editing needs the inverse view on the server: for anchor index N,
//! which source lines produced that element?
//!
//! This module rebuilds the mapping from the comrak AST. Sourcepos is tracked
//! on every node during parsing regardless of render options, so the served
//! HTML stays byte-identical. Two comrak behaviors make AST order equal DOM
//! order: footnote definitions are re-appended to the end of the document in
//! reference order during parsing, and everything else renders where it
//! parses.
//!
//! Raw HTML blocks are the one place the AST can't predict the DOM: the
//! browser may materialize zero, one, or several commentable elements out of
//! one `HtmlBlock`. We classify the shapes we can prove (nothing-commentable,
//! or exactly one commentable element) and mark the map unreliable from the
//! first block we can't.

use comrak::nodes::{AstNode, NodeValue, Sourcepos};
use comrak::{Arena, parse_document};

use crate::render::{render_options, split_frontmatter};

/// What kind of source block an anchor element came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BlockKind {
    /// The YAML front-matter block (rendered as a collapsible `<details>`
    /// whose inner `<pre>` gets wrapped in a commentable `.pre-wrap`).
    Frontmatter,
    Heading,
    Paragraph,
    /// One `<li>` — list items are individually commentable, their parent
    /// list is not.
    ListItem,
    BlockQuote,
    CodeBlock,
    /// A raw HTML block we could prove renders exactly one commentable
    /// element (a commentable tag, or a container with one of the template's
    /// known classes).
    HtmlBlock,
    /// One footnote definition — rendered as an `<li>` in the footnotes
    /// section at the end of the document, in reference order.
    FootnoteDefinition,
}

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
    /// First anchor index whose mapping is unreliable because an
    /// unclassifiable raw-HTML block precedes it. Splices at or beyond this
    /// index must be refused; indices before it are exact.
    pub unsafe_from: Option<usize>,
}

impl BlockMap {
    /// Whether `anchor_idx` maps to source lines we can trust.
    pub fn is_reliable(&self, anchor_idx: usize) -> bool {
        self.unsafe_from.is_none_or(|first| anchor_idx < first)
    }

    pub fn get(&self, anchor_idx: usize) -> Option<&AnchorBlock> {
        self.blocks.get(anchor_idx.checked_sub(1)?)
    }

    fn push(&mut self, sourcepos: Sourcepos, line_offset: usize, kind: BlockKind) {
        self.blocks.push(AnchorBlock {
            anchor_idx: self.blocks.len() + 1,
            line_start: sourcepos.start.line + line_offset,
            line_end: sourcepos.end.line + line_offset,
            kind,
        });
    }
}

/// Build the anchor map for a document.
pub fn anchor_blocks(markdown: &str) -> BlockMap {
    let mut map = BlockMap::default();

    let (body, body_line_offset) = match split_frontmatter(markdown) {
        Some((frontmatter, body)) => {
            let yaml_lines = frontmatter.matches('\n').count();
            map.blocks.push(AnchorBlock {
                anchor_idx: 1,
                line_start: 2,
                line_end: 1 + yaml_lines,
                kind: BlockKind::Frontmatter,
            });
            // Opening fence + YAML lines + closing fence precede the body.
            (body, yaml_lines + 2)
        }
        None => (markdown, 0),
    };

    let arena = Arena::new();
    let root = parse_document(&arena, body, &render_options());
    for node in root.children() {
        visit_top_level(node, body_line_offset, &mut map);
    }

    // Comrak lets some blocks absorb the blank line that follows them (a
    // list's final item, indented code). Trim trailing blank lines so an
    // editor never presents them as part of the block — deleting one in a
    // textarea could merge adjacent blocks.
    let lines: Vec<&str> = markdown.split_inclusive('\n').collect();
    for block in &mut map.blocks {
        while block.line_end > block.line_start
            && lines
                .get(block.line_end - 1)
                .is_some_and(|line| line.trim().is_empty())
        {
            block.line_end -= 1;
        }
    }

    map
}

fn visit_top_level<'a>(node: &'a AstNode<'a>, line_offset: usize, map: &mut BlockMap) {
    let ast = node.data();
    let sourcepos = ast.sourcepos;
    match &ast.value {
        NodeValue::Heading(heading) if heading.level <= 5 => {
            map.push(sourcepos, line_offset, BlockKind::Heading);
        }
        // h6 is absent from COMMENTABLE_SELECTOR.
        NodeValue::Heading(_) => {}
        NodeValue::Paragraph => map.push(sourcepos, line_offset, BlockKind::Paragraph),
        NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
            map.push(sourcepos, line_offset, BlockKind::BlockQuote);
        }
        NodeValue::CodeBlock(_) => map.push(sourcepos, line_offset, BlockKind::CodeBlock),
        NodeValue::List(_) => {
            drop(ast);
            for item in node.children() {
                let item_ast = item.data();
                if matches!(item_ast.value, NodeValue::Item(_) | NodeValue::TaskItem(_)) {
                    let item_sourcepos = item_ast.sourcepos;
                    drop(item_ast);
                    map.push(item_sourcepos, line_offset, BlockKind::ListItem);
                }
            }
        }
        NodeValue::FootnoteDefinition(_) => {
            map.push(sourcepos, line_offset, BlockKind::FootnoteDefinition);
        }
        NodeValue::HtmlBlock(html) => match classify_html_block(&html.literal) {
            HtmlBlockClass::SingleCommentable => {
                map.push(sourcepos, line_offset, BlockKind::HtmlBlock);
            }
            HtmlBlockClass::Invisible => {}
            HtmlBlockClass::Unknown => {
                let next_idx = map.blocks.len() + 1;
                map.unsafe_from.get_or_insert(next_idx);
            }
        },
        // Tables and thematic breaks render, but nothing in them matches the
        // commentable selector.
        NodeValue::Table(_) | NodeValue::ThematicBreak => {}
        _ => {}
    }
}

/// Mechanically carry a comment thread's anchor range across an edit of the
/// block at `edited_idx`. `delta` is the change in total anchor count — the
/// edited block turned into `1 + delta` anchors. Anchors before the edit
/// stay put; anchors after it shift by `delta`; a range touching the edited
/// block stretches or shrinks with it. `None` means the range can't be
/// carried (its content is gone) and the thread should orphan.
pub fn carry_anchor_across_edit(
    anchor_start: usize,
    anchor_end: usize,
    edited_idx: usize,
    delta: isize,
    new_total: usize,
) -> Option<(usize, usize)> {
    // A single-block edit can at most remove that block's own anchor
    // (delta >= -1). A more negative delta means the replacement
    // restructured *following* blocks too — say an unterminated code fence
    // swallowing the rest of the document — and ranges touching or past the
    // edit can't be carried mechanically. Ranges fully before it are safe:
    // a splice never changes earlier lines, and markdown block structure
    // never depends on later content.
    if delta < -1 && anchor_end >= edited_idx {
        return None;
    }
    let new_start = if anchor_start > edited_idx {
        anchor_start as isize + delta
    } else {
        anchor_start as isize
    };
    let new_end = if anchor_end >= edited_idx {
        anchor_end as isize + delta
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

enum HtmlBlockClass {
    /// Renders exactly one commentable element.
    SingleCommentable,
    /// Renders nothing the commentable selector matches.
    Invisible,
    /// Can't prove either — anchor indices after this block are unreliable.
    Unknown,
}

/// Tags the commentable selector matches directly.
const COMMENTABLE_TAGS: [&str; 8] = ["p", "h1", "h2", "h3", "h4", "h5", "blockquote", "pre"];
/// Container classes the selector matches; per the outermost-element rule
/// they swallow any commentable children.
const KNOWN_CONTAINER_CLASSES: [&str; 4] = ["quote", "decision", "callout", "context"];
/// Single elements that render nothing commentable.
const INVISIBLE_TAGS: [&str; 5] = ["script", "style", "hr", "br", "img"];

fn classify_html_block(literal: &str) -> HtmlBlockClass {
    let mut rest = literal.trim();

    // Strip leading comments; a block of only comments is invisible.
    while let Some(after) = rest.strip_prefix("<!--") {
        match after.find("-->") {
            Some(end) => rest = after[end + 3..].trim_start(),
            None => return HtmlBlockClass::Unknown,
        }
    }
    let rest = rest.trim_end();
    if rest.is_empty() {
        return HtmlBlockClass::Invisible;
    }

    let Some(tag) = parse_first_tag(rest) else {
        return HtmlBlockClass::Unknown;
    };

    if INVISIBLE_TAGS.contains(&tag.name.as_str()) {
        return if tag.self_contained_span(rest) {
            HtmlBlockClass::Invisible
        } else {
            HtmlBlockClass::Unknown
        };
    }

    let commentable = COMMENTABLE_TAGS.contains(&tag.name.as_str())
        || tag
            .class_words()
            .any(|word| KNOWN_CONTAINER_CLASSES.contains(&word));
    if commentable && tag.single_element_span(rest) {
        HtmlBlockClass::SingleCommentable
    } else {
        HtmlBlockClass::Unknown
    }
}

struct FirstTag {
    name: String,
    attrs: String,
    /// Byte offset just past the opening tag's `>`.
    open_end: usize,
    self_closing: bool,
}

impl FirstTag {
    fn class_words(&self) -> impl Iterator<Item = &str> {
        extract_class_attr(&self.attrs)
            .unwrap_or("")
            .split_whitespace()
    }

    /// Whether the element is the entire block: void/self-closing tag with
    /// nothing but whitespace after it.
    fn self_contained_span(&self, html: &str) -> bool {
        html[self.open_end..].trim().is_empty()
    }

    /// Whether the block is exactly this one element: scan `<name` opens and
    /// `</name` closes; when depth returns to zero, only whitespace may
    /// remain.
    fn single_element_span(&self, html: &str) -> bool {
        if self.self_closing {
            return self.self_contained_span(html);
        }
        let lower = html.to_ascii_lowercase();
        let open_pat = format!("<{}", self.name);
        let close_pat = format!("</{}", self.name);
        let mut depth = 0usize;
        let mut pos = 0usize;
        while pos < lower.len() {
            let next_open = find_tag(&lower, &open_pat, pos);
            let next_close = find_tag(&lower, &close_pat, pos);
            match (next_open, next_close) {
                (Some(open), Some(close)) if open < close => {
                    depth += 1;
                    pos = open + open_pat.len();
                }
                (_, Some(close)) => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                    let Some(end) = lower[close..].find('>') else {
                        return false;
                    };
                    pos = close + end + 1;
                    if depth == 0 {
                        return html[pos..].trim().is_empty();
                    }
                }
                (Some(open), None) => {
                    depth += 1;
                    pos = open + open_pat.len();
                }
                (None, None) => return false,
            }
        }
        false
    }
}

/// Find `pattern` at or after `from`, requiring the following character to
/// terminate a tag name (so `<div` doesn't match `<divider`).
fn find_tag(lower: &str, pattern: &str, from: usize) -> Option<usize> {
    let mut search_from = from;
    while let Some(found) = lower[search_from..].find(pattern) {
        let at = search_from + found;
        let after = lower[at + pattern.len()..].chars().next();
        if matches!(after, Some(c) if c.is_whitespace() || c == '>' || c == '/') || after.is_none()
        {
            return Some(at);
        }
        search_from = at + pattern.len();
    }
    None
}

fn parse_first_tag(html: &str) -> Option<FirstTag> {
    let after_open = html.strip_prefix('<')?;
    let name_len = after_open
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(after_open.len());
    if name_len == 0 {
        return None;
    }
    let name = after_open[..name_len].to_ascii_lowercase();

    // Scan to the closing '>' of the opening tag, skipping quoted values.
    let attrs_start = 1 + name_len;
    let mut quote: Option<char> = None;
    for (offset, c) in html[attrs_start..].char_indices() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"') | (None, '\'') => quote = Some(c),
            (None, '>') => {
                let attrs = &html[attrs_start..attrs_start + offset];
                return Some(FirstTag {
                    name,
                    attrs: attrs.trim().trim_end_matches('/').trim().to_string(),
                    open_end: attrs_start + offset + 1,
                    self_closing: attrs.trim_end().ends_with('/'),
                });
            }
            (None, _) => {}
        }
    }
    None
}

fn extract_class_attr(attrs: &str) -> Option<&str> {
    let lower = attrs.to_ascii_lowercase();
    let mut search_from = 0;
    loop {
        let found = lower[search_from..].find("class")?;
        let at = search_from + found;
        // Must be attribute-position: start of attrs or preceded by whitespace.
        let standalone = at == 0
            || lower[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace());
        let after = &attrs[at + "class".len()..];
        let after_eq = after.trim_start();
        if standalone && after_eq.starts_with('=') {
            let value = after_eq[1..].trim_start();
            return Some(match value.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let inner = &value[1..];
                    &inner[..inner.find(q).unwrap_or(inner.len())]
                }
                _ => {
                    &value[..value
                        .find(|c: char| c.is_whitespace())
                        .unwrap_or(value.len())]
                }
            });
        }
        search_from = at + "class".len();
    }
}

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

    #[test]
    fn headings_h1_to_h5_are_anchors_h6_is_not() {
        let md = "# a\n\n## b\n\n### c\n\n#### d\n\n##### e\n\n###### f\n";
        let map = anchor_blocks(md);

        assert_eq!(kinds(&map), vec![BlockKind::Heading; 5]);
        assert_eq!(ranges(&map), vec![(1, 1), (3, 3), (5, 5), (7, 7), (9, 9)]);
        assert!(map.unsafe_from.is_none());
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
    fn tables_and_thematic_breaks_are_not_anchors() {
        let md = "before\n\n| a | b |\n| - | - |\n| c | d |\n\n---\n\nafter\n";
        let map = anchor_blocks(md);

        assert_eq!(
            kinds(&map),
            vec![BlockKind::Paragraph, BlockKind::Paragraph]
        );
        assert_eq!(ranges(&map), vec![(1, 1), (9, 9)]);
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
    fn known_class_html_block_is_a_single_anchor() {
        let md = "<div class=\"quote\">\nstakeholder input\n</div>\n\nafter\n";
        let map = anchor_blocks(md);

        assert_eq!(
            kinds(&map),
            vec![BlockKind::HtmlBlock, BlockKind::Paragraph]
        );
        assert_eq!(ranges(&map), vec![(1, 3), (5, 5)]);
        assert!(map.unsafe_from.is_none());
    }

    #[test]
    fn known_class_html_block_with_nested_div_still_single_anchor() {
        let md = "<div class=\"callout\"><div>inner</div></div>\n\nafter\n";
        let map = anchor_blocks(md);

        assert_eq!(
            kinds(&map),
            vec![BlockKind::HtmlBlock, BlockKind::Paragraph]
        );
        assert!(map.unsafe_from.is_none());
    }

    #[test]
    fn raw_p_tag_block_is_a_single_anchor() {
        let md = "<p>raw paragraph</p>\n\nafter\n";
        let map = anchor_blocks(md);

        assert_eq!(
            kinds(&map),
            vec![BlockKind::HtmlBlock, BlockKind::Paragraph]
        );
    }

    #[test]
    fn html_comment_is_invisible() {
        let md = "before\n\n<!-- note to self -->\n\nafter\n";
        let map = anchor_blocks(md);

        assert_eq!(
            kinds(&map),
            vec![BlockKind::Paragraph, BlockKind::Paragraph]
        );
        assert_eq!(ranges(&map), vec![(1, 1), (5, 5)]);
        assert!(map.unsafe_from.is_none());
    }

    #[test]
    fn unknown_html_block_poisons_following_indices_only() {
        let md = "reliable\n\n<div class=\"banner\">who knows</div>\n\nunreliable\n";
        let map = anchor_blocks(md);

        // The banner div is not commentable and not a known class: the
        // browser assigns it no anchor, but we can't prove that in general.
        assert_eq!(
            kinds(&map),
            vec![BlockKind::Paragraph, BlockKind::Paragraph]
        );
        assert_eq!(map.unsafe_from, Some(2));
        assert!(map.is_reliable(1));
        assert!(!map.is_reliable(2));
    }

    #[test]
    fn two_adjacent_known_class_divs_in_one_block_are_unknown() {
        // No blank line between them: comrak folds both divs into one
        // HtmlBlock, which renders two anchors — more than we can map.
        let md = "<div class=\"quote\">a</div>\n<div class=\"decision\">b</div>\n\nafter\n";
        let map = anchor_blocks(md);

        assert_eq!(map.unsafe_from, Some(1));
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

<div class=\"decision\">ship it</div>

uses a note[^n]

[^n]: the note
";

    #[test]
    fn identity_splice_is_byte_identical_for_every_anchor() {
        let map = anchor_blocks(KITCHEN_SINK);
        assert!(map.unsafe_from.is_none());
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
        assert_eq!(carry_anchor_across_edit(1, 2, 3, 1, 6), Some((1, 2)));
        assert_eq!(carry_anchor_across_edit(1, 2, 3, -1, 4), Some((1, 2)));
    }

    #[test]
    fn carry_anchor_shifts_ranges_after_the_edit_by_delta() {
        assert_eq!(carry_anchor_across_edit(4, 5, 3, 1, 6), Some((5, 6)));
        assert_eq!(carry_anchor_across_edit(4, 5, 3, -1, 4), Some((3, 4)));
        assert_eq!(carry_anchor_across_edit(4, 5, 3, 0, 5), Some((4, 5)));
    }

    #[test]
    fn carry_anchor_stretches_a_range_over_a_split_block() {
        // Point thread on the edited block: covers all replacement anchors.
        assert_eq!(carry_anchor_across_edit(3, 3, 3, 2, 7), Some((3, 5)));
        // Range ending at the edited block stretches with it.
        assert_eq!(carry_anchor_across_edit(2, 3, 3, 1, 6), Some((2, 4)));
        // Range starting at the edited block keeps its start.
        assert_eq!(carry_anchor_across_edit(3, 4, 3, 1, 6), Some((3, 5)));
    }

    #[test]
    fn carry_anchor_orphans_a_point_thread_on_a_deleted_block() {
        assert_eq!(carry_anchor_across_edit(3, 3, 3, -1, 4), None);
    }

    #[test]
    fn carry_anchor_shrinks_ranges_spanning_a_deleted_block() {
        assert_eq!(carry_anchor_across_edit(2, 3, 3, -1, 4), Some((2, 2)));
        assert_eq!(carry_anchor_across_edit(3, 4, 3, -1, 4), Some((3, 3)));
        assert_eq!(carry_anchor_across_edit(2, 4, 3, -1, 4), Some((2, 3)));
    }

    #[test]
    fn carry_anchor_orphans_ranges_that_fall_off_the_document() {
        // Replacement swallowed following blocks (e.g. unterminated fence):
        // a thread whose range no longer fits the new total orphans instead
        // of drifting onto the wrong block.
        assert_eq!(carry_anchor_across_edit(4, 5, 3, -3, 2), None);
        // Document emptied entirely.
        assert_eq!(carry_anchor_across_edit(1, 1, 1, -1, 0), None);
    }
}
