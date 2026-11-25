//! Server-side API metadata for 1-based commentable markdown blocks.

use serde::Serialize;

use crate::render::render_markdown;

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

/// Returns metadata from the same AST plan that stamps the rendered markdown.
pub fn markdown_blocks(markdown: &str) -> Vec<Block> {
    render_markdown(markdown)
        .blocks
        .into_iter()
        .map(|block| Block {
            index: block.index,
            snippet: block.snippet,
            breadcrumb: block.breadcrumb,
        })
        .collect()
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
    fn tables_keep_a_whole_table_block_and_add_row_blocks() {
        let blocks = snippets("before\n\n| a | b |\n| - | - |\n| c | d |\n\n---\n\nafter\n");

        assert_eq!(blocks, vec!["before", "a b c d", "a b", "c d", "after"]);
    }

    #[test]
    fn table_rows_get_identifying_breadcrumbs_and_document_order_indices() {
        let blocks = markdown_blocks("# Plan\n\nintro\n\n| a |\n| - |\n| b |\n| c |\n\nafter\n");

        assert_eq!(
            blocks.iter().map(|b| b.index).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(blocks[2].snippet, "a b c");
        assert_eq!(blocks[2].breadcrumb, "Plan");
        assert_eq!(blocks[3].snippet, "a");
        assert_eq!(blocks[3].breadcrumb, "Plan › Table header");
        assert_eq!(blocks[4].snippet, "b");
        assert_eq!(blocks[4].breadcrumb, "Plan › Table row 1");
        assert_eq!(blocks[5].snippet, "c");
        assert_eq!(blocks[5].breadcrumb, "Plan › Table row 2");
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
        assert!(blocks[0].snippet.len() <= 300);
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
