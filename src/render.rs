//! Pure markdown rendering and its shared commentable-block plan.

use std::collections::HashMap;
use std::fmt::{self, Write};

use comrak::html::{ChildRendering, Context, format_document_with_formatter, format_node_default};
use comrak::nodes::{AstNode, NodeValue};
use comrak::options::Plugins;
use comrak::{Arena, Options, parse_document};

const SNIPPET_MAX_BYTES: usize = 300;
const ANCHOR_MARKER_PREFIX: &str = "<!--discuss-anchor-";

pub fn render(markdown: &str) -> String {
    render_markdown(markdown).html
}

fn render_options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    options
}

fn split_frontmatter(input: &str) -> Option<(&str, &str)> {
    let first_newline = input.find('\n')?;
    if input[..first_newline].trim_end() != "---" {
        return None;
    }
    let after_open = &input[first_newline + 1..];

    let mut offset = 0usize;
    for line in after_open.split_inclusive('\n') {
        if line.trim_end() == "---" {
            let frontmatter = &after_open[..offset];
            let body = &after_open[offset + line.len()..];
            return Some((frontmatter, body));
        }
        offset += line.len();
    }
    None
}

pub(crate) struct RenderedBlock {
    pub(crate) index: usize,
    pub(crate) snippet: String,
    pub(crate) breadcrumb: String,
}

pub(crate) struct RenderedMarkdown {
    pub(crate) html: String,
    pub(crate) blocks: Vec<RenderedBlock>,
}

#[derive(Clone, Copy)]
enum AnchorTarget {
    Element(&'static str),
    PreWrapper,
    TableWrapper,
}

struct PlannedBlock {
    node_id: Option<usize>,
    target: AnchorTarget,
    snippet: String,
    breadcrumb: String,
}

#[derive(Clone, Copy)]
struct RenderAnchor {
    index: usize,
    target: AnchorTarget,
}

struct RenderContext {
    anchors: HashMap<usize, RenderAnchor>,
}

/// Parses markdown once, plans its outermost commentable blocks, and renders
/// stamps from that same plan.
pub(crate) fn render_markdown(markdown: &str) -> RenderedMarkdown {
    let mut planned = Vec::new();
    let mut footnotes = Vec::new();
    let mut heading_stack: Vec<(u8, String)> = Vec::new();

    let (frontmatter, body) = if let Some((frontmatter, body)) = split_frontmatter(markdown) {
        planned.push(PlannedBlock {
            node_id: None,
            target: AnchorTarget::PreWrapper,
            snippet: truncate_snippet(frontmatter.trim()),
            breadcrumb: String::new(),
        });
        (Some(frontmatter), body)
    } else {
        (None, markdown)
    };

    let options = render_options();
    let arena = Arena::new();
    let root = parse_document(&arena, body, &options);

    for node in root.children() {
        let node_key = node_id(node);
        match &node.data.borrow().value {
            NodeValue::Heading(heading) => {
                // h6 is deliberately neither commentable nor part of the
                // breadcrumb hierarchy.
                if heading.level > 5 {
                    continue;
                }
                let text = plain_text(node);
                heading_stack.retain(|(level, _)| *level < heading.level);
                heading_stack.push((heading.level, text.clone()));
                planned.push(PlannedBlock {
                    node_id: Some(node_key),
                    target: AnchorTarget::Element(match heading.level {
                        1 => "<h1",
                        2 => "<h2",
                        3 => "<h3",
                        4 => "<h4",
                        5 => "<h5",
                        _ => unreachable!(),
                    }),
                    snippet: truncate_snippet(&text),
                    breadcrumb: breadcrumb(&heading_stack),
                });
            }
            NodeValue::Paragraph => planned.push(planned_node(
                node,
                node_key,
                AnchorTarget::Element("<p"),
                &heading_stack,
            )),
            NodeValue::BlockQuote => planned.push(planned_node(
                node,
                node_key,
                AnchorTarget::Element("<blockquote"),
                &heading_stack,
            )),
            NodeValue::Table(_) => {
                // Keep the table-level anchor first so existing table threads
                // continue to resolve, then add one precise anchor per row.
                planned.push(planned_node(
                    node,
                    node_key,
                    AnchorTarget::TableWrapper,
                    &heading_stack,
                ));
                for (row_offset, row) in node.children().enumerate() {
                    let is_header = matches!(row.data.borrow().value, NodeValue::TableRow(true));
                    let row_label = if is_header {
                        "Table header".to_string()
                    } else {
                        format!("Table row {row_offset}")
                    };
                    let mut row_breadcrumb = heading_stack.clone();
                    row_breadcrumb.push((6, row_label));
                    planned.push(planned_node(
                        row,
                        node_id(row),
                        AnchorTarget::Element("<tr"),
                        &row_breadcrumb,
                    ));
                }
            }
            NodeValue::CodeBlock(code_block) => planned.push(PlannedBlock {
                node_id: Some(node_key),
                target: AnchorTarget::PreWrapper,
                snippet: truncate_snippet(code_block.literal.trim_end()),
                breadcrumb: breadcrumb(&heading_stack),
            }),
            NodeValue::List(_) => {
                // Only direct children of a root list are outermost anchors.
                for item in node.children() {
                    if matches!(
                        item.data.borrow().value,
                        NodeValue::Item(_) | NodeValue::TaskItem(_)
                    ) {
                        planned.push(planned_node(
                            item,
                            node_id(item),
                            AnchorTarget::Element("<li"),
                            &heading_stack,
                        ));
                    }
                }
            }
            NodeValue::FootnoteDefinition(_) => {
                // Comrak renders referenced definitions as trailing list items.
                // Keep them after every ordinary body block.
                footnotes.push(planned_node(
                    node,
                    node_key,
                    AnchorTarget::Element("<li"),
                    &heading_stack,
                ));
            }
            // Thematic breaks and raw HTML blocks are not commentable.
            _ => {}
        }
    }
    planned.extend(footnotes);

    let mut anchors = HashMap::new();
    let blocks = planned
        .iter()
        .enumerate()
        .map(|(offset, block)| {
            let index = offset + 1;
            if let Some(node_id) = block.node_id {
                anchors.insert(
                    node_id,
                    RenderAnchor {
                        index,
                        target: block.target,
                    },
                );
            }
            RenderedBlock {
                index,
                snippet: block.snippet.clone(),
                breadcrumb: block.breadcrumb.clone(),
            }
        })
        .collect();

    let mut body_html = String::new();
    format_document_with_formatter(
        root,
        &options,
        &mut body_html,
        &Plugins::default(),
        stamped_formatter,
        RenderContext { anchors },
    )
    .expect("writing rendered markdown to a String cannot fail");
    inject_element_stamps(&mut body_html, &planned);

    let mut html = frontmatter
        .map(|yaml| render_frontmatter_block(yaml, 1))
        .unwrap_or_default();
    html.push_str(&body_html);
    RenderedMarkdown { html, blocks }
}

fn planned_node<'a>(
    node: &'a AstNode<'a>,
    node_id: usize,
    target: AnchorTarget,
    heading_stack: &[(u8, String)],
) -> PlannedBlock {
    PlannedBlock {
        node_id: Some(node_id),
        target,
        snippet: truncate_snippet(&plain_text(node)),
        breadcrumb: breadcrumb(heading_stack),
    }
}

fn node_id(node: &AstNode<'_>) -> usize {
    node as *const AstNode<'_> as usize
}

fn stamped_formatter<'a>(
    context: &mut Context<RenderContext>,
    node: &'a AstNode<'a>,
    entering: bool,
) -> Result<ChildRendering, fmt::Error> {
    let anchor = context.user.anchors.get(&node_id(node)).copied();

    if entering && let Some(anchor) = anchor {
        match anchor.target {
            AnchorTarget::PreWrapper => {
                write!(
                    context,
                    "<div class=\"pre-wrap\" data-anchor-idx=\"{}\">",
                    anchor.index
                )?;
            }
            AnchorTarget::TableWrapper => {
                write!(
                    context,
                    "<div class=\"table-wrap\" data-anchor-idx=\"{}\">",
                    anchor.index
                )?;
            }
            AnchorTarget::Element(_) => {}
        }
    }

    let children = format_node_default(context, node, entering)?;

    if entering
        && let Some(RenderAnchor {
            index,
            target: AnchorTarget::Element(_),
        }) = anchor
    {
        write!(context, "{ANCHOR_MARKER_PREFIX}{index}-->")?;
    }

    if !entering
        && matches!(
            anchor.map(|anchor| anchor.target),
            Some(AnchorTarget::PreWrapper | AnchorTarget::TableWrapper)
        )
    {
        context.write_str("</div>\n")?;
    }

    Ok(children)
}

fn inject_element_stamps(html: &mut String, planned: &[PlannedBlock]) {
    for (offset, block) in planned.iter().enumerate() {
        let AnchorTarget::Element(tag) = block.target else {
            continue;
        };
        let index = offset + 1;
        let marker = format!("{ANCHOR_MARKER_PREFIX}{index}-->");
        let marker_start = html
            .find(&marker)
            .expect("planned anchor marker should be rendered");
        let marker_end = marker_start + marker.len();
        let tag_start = html[..marker_start]
            .rfind(tag)
            .expect("planned anchor element should be rendered");
        html.replace_range(marker_start..marker_end, "");
        html.insert_str(
            tag_start + tag.len(),
            &format!(" data-anchor-idx=\"{index}\""),
        );
    }
}

fn render_frontmatter_block(yaml: &str, index: usize) -> String {
    format!(
        "<details class=\"front-matter\"><summary>Front Matter</summary><div class=\"pre-wrap\" data-anchor-idx=\"{index}\"><pre><code class=\"language-yaml\">{}</code></pre></div></details>\n",
        escape_html(yaml)
    )
}

fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
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

    fn assert_contains_all(html: &str, expected_parts: &[&str]) {
        for expected_part in expected_parts {
            assert!(
                html.contains(expected_part),
                "expected rendered HTML to contain {expected_part:?}\n{html}"
            );
        }
    }

    #[test]
    fn renders_heading_levels_one_through_five() {
        let html = render(
            r#"
# One
## Two
### Three
#### Four
##### Five
"#,
        );

        assert_contains_all(
            &html,
            &[
                "<h1 data-anchor-idx=\"1\">One</h1>",
                "<h2 data-anchor-idx=\"2\">Two</h2>",
                "<h3 data-anchor-idx=\"3\">Three</h3>",
                "<h4 data-anchor-idx=\"4\">Four</h4>",
                "<h5 data-anchor-idx=\"5\">Five</h5>",
            ],
        );
    }

    #[test]
    fn renders_lists_blockquotes_and_fenced_code_as_semantic_elements() {
        let html = render(
            r#"
- unordered
- list

1. ordered
2. list

> quoted

```rust
fn main() {}
```
"#,
        );

        assert_contains_all(
            &html,
            &[
                "<ul>",
                "<li data-anchor-idx=\"1\">unordered</li>",
                "<ol>",
                "<li data-anchor-idx=\"3\">ordered</li>",
                "<blockquote data-anchor-idx=\"5\">",
                "<p>quoted</p>",
                "<pre><code class=\"language-rust\">fn main() {}",
            ],
        );
        assert_eq!(html.matches("<div class=\"pre-wrap\"").count(), 1);
    }

    #[test]
    fn renders_gfm_tables_task_lists_strikethrough_autolinks_and_footnotes() {
        let html = render(
            r#"
| status | owner |
| --- | --- |
| done | team |

- [x] shipped
- [ ] pending

~~removed~~

Visit www.example.com.

Footnote here.[^note]

[^note]: supporting detail
"#,
        );

        assert_contains_all(
            &html,
            &[
                "<table>",
                "<th>status</th>",
                "<td>team</td>",
                "<input type=\"checkbox\" checked=\"\" disabled=\"\" /> shipped",
                "<input type=\"checkbox\" disabled=\"\" /> pending",
                "<del>removed</del>",
                "<a href=\"http://www.example.com\">www.example.com</a>",
                "footnote-ref",
                "supporting detail",
            ],
        );
    }

    #[test]
    fn leaves_mermaid_code_blocks_for_client_side_hydration() {
        let html = render(
            r#"
```mermaid
graph TD
A---B
```
"#,
        );

        assert_eq!(
            html,
            "<div class=\"pre-wrap\" data-anchor-idx=\"1\">\n<pre><code class=\"language-mermaid\">graph TD\nA---B\n</code></pre>\n</div>\n"
        );
    }

    #[test]
    fn renders_yaml_frontmatter_as_collapsible_details_block() {
        let html = render("---\ntitle: Demo\nauthor: Chris\n---\n# Hello\n");

        assert_contains_all(
            &html,
            &[
                "<details class=\"front-matter\">",
                "<summary>Front Matter</summary>",
                "<pre><code class=\"language-yaml\">",
                "title: Demo\nauthor: Chris\n",
                "</code></pre></div></details>",
                "<h1 data-anchor-idx=\"2\">Hello</h1>",
            ],
        );
    }

    #[test]
    fn frontmatter_block_escapes_html_special_chars() {
        let html = render("---\ntitle: \"1 < 2 & 3 > 0\"\n---\nbody\n");

        assert!(html.contains("1 &lt; 2 &amp; 3 &gt; 0"));
        assert!(!html.contains("1 < 2 & 3 > 0"));
    }

    #[test]
    fn no_frontmatter_when_missing_closing_delimiter() {
        let html = render("---\nfoo: bar\n\nbody text\n");

        assert!(!html.contains("front-matter"));
        assert!(!html.contains("<summary>Front Matter</summary>"));
    }

    #[test]
    fn no_frontmatter_when_dashes_not_at_top() {
        let html = render("\n---\nfoo: bar\n---\n");

        assert!(!html.contains("front-matter"));
        assert!(!html.contains("<summary>Front Matter</summary>"));
    }

    #[test]
    fn body_renders_normally_after_frontmatter() {
        let html = render("---\ntitle: x\n---\n# Heading\n\npara\n");

        assert_contains_all(
            &html,
            &[
                "<details class=\"front-matter\">",
                "<h1 data-anchor-idx=\"2\">Heading</h1>",
                "<p data-anchor-idx=\"3\">para</p>",
            ],
        );
    }

    #[test]
    fn empty_frontmatter_still_renders_block() {
        let html = render("---\n---\n# Title\n");

        assert_contains_all(
            &html,
            &[
                "<details class=\"front-matter\">",
                "<pre><code class=\"language-yaml\"></code></pre>",
                "<h1 data-anchor-idx=\"2\">Title</h1>",
            ],
        );
    }

    #[test]
    fn render_is_deterministic() {
        let markdown = "# Title\n\n- [x] item\n\n| a | b |\n| - | - |\n| c | d |\n";

        assert_eq!(render(markdown), render(markdown));
    }

    fn stamped_indices(html: &str) -> Vec<usize> {
        html.split("data-anchor-idx=\"")
            .skip(1)
            .map(|suffix| {
                suffix
                    .split_once('"')
                    .expect("anchor stamp should have a closing quote")
                    .0
                    .parse()
                    .expect("anchor stamp should be numeric")
            })
            .collect()
    }

    #[test]
    fn rendered_stamps_and_block_indices_share_one_outermost_plan() {
        let markdown = r#"---
title: Demo
---
# Heading
###### Not commentable

Paragraph.

> Quote
>
> - nested item
> - nested item with code:
>
>   ```text
>   nested
>   ```

- top item
  - nested item
- [x] task item

| a | b |
| - | - |
| c | d |

```rust
fn main() {}
```

Reference.[^note]

Later.

[^note]: Footnote detail.
"#;

        let rendered = render_markdown(markdown);
        let expected: Vec<usize> = (1..=rendered.blocks.len()).collect();

        assert_eq!(
            rendered
                .blocks
                .iter()
                .map(|block| block.snippet.as_str())
                .collect::<Vec<_>>(),
            vec![
                "title: Demo",
                "Heading",
                "Paragraph.",
                "Quote nested item nested item with code:",
                "top item nested item",
                "task item",
                "a b c d",
                "a b",
                "c d",
                "fn main() {}",
                "Reference.",
                "Later.",
                "Footnote detail.",
            ]
        );
        assert_eq!(stamped_indices(&rendered.html), expected);
        assert!(rendered.html.contains(
            "<div class=\"pre-wrap\" data-anchor-idx=\"1\"><pre><code class=\"language-yaml\">"
        ));
        assert!(
            rendered
                .html
                .contains("<h1 data-anchor-idx=\"2\">Heading</h1>")
        );
        assert!(rendered.html.contains("<blockquote data-anchor-idx=\"4\">"));
        assert!(rendered.html.contains("<li data-anchor-idx=\"5\">top item"));
        assert!(
            rendered
                .html
                .contains("<div class=\"table-wrap\" data-anchor-idx=\"7\">")
        );
        assert!(rendered.html.contains("<tr data-anchor-idx=\"8\">"));
        assert!(rendered.html.contains("<tr data-anchor-idx=\"9\">"));
        assert!(
            rendered
                .html
                .contains("<li data-anchor-idx=\"13\" id=\"fn-note\">")
        );
        assert_eq!(
            rendered.html.matches("<div class=\"table-wrap\"").count(),
            1
        );
        assert_eq!(rendered.html.matches("class=\"pre-wrap\"").count(), 2);
        assert!(rendered.html.contains("<section class=\"footnotes"));
        assert!(!rendered.html.contains("###### Not commentable"));
        assert_eq!(
            rendered.html.matches("data-anchor-idx=").count(),
            rendered.blocks.len()
        );
    }
}
