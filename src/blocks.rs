//! Server-side segmentation of markdown into 1-based commentable blocks.
//!
//! This mirrors the browser's anchor-index assignment in `discuss.html`
//! (`assignAnchorIndices` over `COMMENTABLE_SELECTOR`, filtered to outermost
//! elements in document order). If that selector changes, this walk must be
//! updated to match, and vice versa.

use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, parse_document};
use serde::Serialize;

use crate::render::{render_options, split_frontmatter};

const SNIPPET_MAX_BYTES: usize = 300;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    /// 1-based index among commentable blocks in document order — the same
    /// units as `anchorStart`/`anchorEnd` on threads.
    pub index: usize,
    /// Plain-text preview of the block, truncated on a char boundary.
    pub snippet: String,
    /// Heading path down to this block, e.g. "Rollout plan › Phases".
    pub breadcrumb: String,
}

/// Segments markdown into the commentable blocks the browser will index:
/// headings h1–h5, paragraphs, top-level list items, blockquotes, tables, and
/// code blocks (including the frontmatter `pre`, which the browser counts
/// first).
/// Footnote definitions render as list items at the end of the document, so
/// they are appended after all other blocks.
pub fn markdown_blocks(markdown: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut footnotes = Vec::new();
    let mut heading_stack: Vec<(u8, String)> = Vec::new();

    let body = if let Some((frontmatter, body)) = split_frontmatter(markdown) {
        blocks.push((truncate_snippet(frontmatter.trim()), String::new()));
        body
    } else {
        markdown
    };

    let arena = Arena::new();
    let root = parse_document(&arena, body, &render_options());

    for node in root.children() {
        match &node.data.borrow().value {
            NodeValue::Heading(heading) => {
                // h6 is not in the browser's COMMENTABLE_SELECTOR.
                if heading.level > 5 {
                    continue;
                }
                let text = plain_text(node);
                heading_stack.retain(|(level, _)| *level < heading.level);
                heading_stack.push((heading.level, text.clone()));
                blocks.push((truncate_snippet(&text), breadcrumb(&heading_stack)));
            }
            NodeValue::Paragraph | NodeValue::BlockQuote => {
                blocks.push((
                    truncate_snippet(&plain_text(node)),
                    breadcrumb(&heading_stack),
                ));
            }
            NodeValue::Table(_) => {
                // The browser wraps each <table> in a .table-wrap anchor —
                // one block per table.
                blocks.push((
                    truncate_snippet(&plain_text(node)),
                    breadcrumb(&heading_stack),
                ));
            }
            NodeValue::CodeBlock(code_block) => {
                blocks.push((
                    truncate_snippet(code_block.literal.trim_end()),
                    breadcrumb(&heading_stack),
                ));
            }
            NodeValue::List(_) => {
                // The ul/ol itself is not commentable; each top-level item is
                // the outermost commentable element (nested lists stay inside
                // their parent item's block).
                for item in node.children() {
                    if matches!(
                        item.data.borrow().value,
                        NodeValue::Item(_) | NodeValue::TaskItem(_)
                    ) {
                        blocks.push((
                            truncate_snippet(&plain_text(item)),
                            breadcrumb(&heading_stack),
                        ));
                    }
                }
            }
            NodeValue::FootnoteDefinition(_) => {
                // Rendered as trailing <li>s inside <section class="footnotes">,
                // which the browser indexes after everything else.
                footnotes.push((
                    truncate_snippet(&plain_text(node)),
                    breadcrumb(&heading_stack),
                ));
            }
            // Thematic breaks and raw HTML blocks are not commentable.
            _ => {}
        }
    }

    blocks.extend(footnotes);
    blocks
        .into_iter()
        .enumerate()
        .map(|(index, (snippet, breadcrumb))| Block {
            index: index + 1,
            snippet,
            breadcrumb,
        })
        .collect()
}

fn breadcrumb(heading_stack: &[(u8, String)]) -> String {
    heading_stack
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join(" › ")
}

fn plain_text<'a>(node: &'a AstNode<'a>) -> String {
    let mut out = String::new();
    collect_plain_text(node, &mut out);
    out.trim().to_string()
}

fn collect_plain_text<'a>(node: &'a AstNode<'a>, out: &mut String) {
    match &node.data.borrow().value {
        NodeValue::Text(text) => out.push_str(text),
        NodeValue::Code(code) => out.push_str(&code.literal),
        NodeValue::SoftBreak | NodeValue::LineBreak => out.push(' '),
        _ => {
            for child in node.children() {
                // Separate nested blocks (paragraphs, nested list items) with
                // a space so they don't run together in the snippet.
                if child.data.borrow().value.block() && !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                collect_plain_text(child, out);
            }
        }
    }
}

fn truncate_snippet(text: &str) -> String {
    let mut snippet = text.to_string();
    if snippet.len() <= SNIPPET_MAX_BYTES {
        return snippet;
    }
    let mut boundary = SNIPPET_MAX_BYTES;
    while !snippet.is_char_boundary(boundary) {
        boundary -= 1;
    }
    snippet.truncate(boundary);
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippets(markdown: &str) -> Vec<String> {
        markdown_blocks(markdown)
            .into_iter()
            .map(|block| block.snippet)
            .collect()
    }

    #[test]
    fn counts_headings_one_through_five_and_skips_h6() {
        let blocks = markdown_blocks("# One\n\n## Two\n\n###### Six\n\n##### Five\n");

        assert_eq!(
            blocks
                .iter()
                .map(|block| (block.index, block.snippet.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, "One"), (2, "Two"), (3, "Five")]
        );
    }

    #[test]
    fn paragraphs_blockquotes_and_code_blocks_are_single_blocks() {
        let blocks =
            snippets("para one\n\n> quoted line one\n> and two\n\n```rust\nfn main() {}\n```\n");

        assert_eq!(
            blocks,
            vec!["para one", "quoted line one and two", "fn main() {}"]
        );
    }

    #[test]
    fn mermaid_fences_count_as_code_blocks() {
        assert_eq!(
            snippets("```mermaid\ngraph TD\nA---B\n```\n"),
            vec!["graph TD\nA---B"]
        );
    }

    #[test]
    fn each_top_level_list_item_is_a_block_with_nested_content_collapsed() {
        let blocks = snippets(
            "- first\n- second\n  - nested one\n  - nested two\n\n1. ordered\n\n- [x] shipped\n- [ ] pending\n",
        );

        assert_eq!(
            blocks,
            vec![
                "first",
                "second nested one nested two",
                "ordered",
                "shipped",
                "pending"
            ]
        );
    }

    #[test]
    fn tables_are_single_blocks_and_thematic_breaks_are_skipped() {
        let blocks = snippets("before\n\n| a | b |\n| - | - |\n| c | d |\n\n---\n\nafter\n");

        assert_eq!(blocks, vec!["before", "a b c d", "after"]);
    }

    #[test]
    fn tables_get_heading_breadcrumb_and_document_order_index() {
        let blocks = markdown_blocks("# Plan\n\nintro\n\n| a |\n| - |\n| b |\n\nafter\n");

        assert_eq!(
            blocks.iter().map(|b| b.index).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert_eq!(blocks[2].snippet, "a b");
        assert_eq!(blocks[2].breadcrumb, "Plan");
    }

    #[test]
    fn frontmatter_is_block_one() {
        let blocks = markdown_blocks("---\ntitle: Demo\n---\n# Hello\n");

        assert_eq!(blocks[0].index, 1);
        assert_eq!(blocks[0].snippet, "title: Demo");
        assert_eq!(blocks[0].breadcrumb, "");
        assert_eq!(blocks[1].snippet, "Hello");
    }

    #[test]
    fn footnote_definitions_are_appended_after_other_blocks() {
        let blocks = snippets("Text.[^note]\n\n[^note]: supporting detail\n\nlater para\n");

        assert_eq!(blocks, vec!["Text.", "later para", "supporting detail"]);
    }

    #[test]
    fn breadcrumbs_follow_the_heading_stack() {
        let blocks = markdown_blocks(
            "intro\n\n# Rollout plan\n\nstaged\n\n## Phases\n\nby region\n\n# Risks\n\nnone\n",
        );

        let crumbs: Vec<(&str, &str)> = blocks
            .iter()
            .map(|block| (block.snippet.as_str(), block.breadcrumb.as_str()))
            .collect();
        assert_eq!(
            crumbs,
            vec![
                ("intro", ""),
                ("Rollout plan", "Rollout plan"),
                ("staged", "Rollout plan"),
                ("Phases", "Rollout plan › Phases"),
                ("by region", "Rollout plan › Phases"),
                ("Risks", "Risks"),
                ("none", "Risks"),
            ]
        );
    }

    #[test]
    fn snippets_truncate_on_a_char_boundary() {
        let long = "é".repeat(400);
        let blocks = markdown_blocks(&long);

        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].snippet.len() <= SNIPPET_MAX_BYTES);
        assert!(blocks[0].snippet.chars().all(|c| c == 'é'));
    }

    #[test]
    fn inline_code_and_soft_breaks_flatten_into_plain_text() {
        assert_eq!(
            snippets("Call `foo()`\nthen stop.\n"),
            vec!["Call foo() then stop."]
        );
    }
}
