use comrak::{Options, markdown_to_html};

pub fn render(markdown: &str) -> String {
    if let Some((frontmatter, body)) = split_frontmatter(markdown) {
        let mut out = render_frontmatter_block(frontmatter);
        out.push_str(&markdown_to_html(body, &render_options()));
        out
    } else {
        markdown_to_html(markdown, &render_options())
    }
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

fn render_frontmatter_block(yaml: &str) -> String {
    format!(
        "<details class=\"front-matter\"><summary>Front Matter</summary><pre><code class=\"language-yaml\">{}</code></pre></details>\n",
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
                "<h1>One</h1>",
                "<h2>Two</h2>",
                "<h3>Three</h3>",
                "<h4>Four</h4>",
                "<h5>Five</h5>",
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
                "<li>unordered</li>",
                "<ol>",
                "<li>ordered</li>",
                "<blockquote>",
                "<p>quoted</p>",
                "<pre><code class=\"language-rust\">fn main() {}",
            ],
        );
        assert!(!html.contains("<div"));
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
            "<pre><code class=\"language-mermaid\">graph TD\nA---B\n</code></pre>\n"
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
                "</code></pre></details>",
                "<h1>Hello</h1>",
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
                "<h1>Heading</h1>",
                "<p>para</p>",
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
                "<h1>Title</h1>",
            ],
        );
    }

    #[test]
    fn render_is_deterministic() {
        let markdown = "# Title\n\n- [x] item\n\n| a | b |\n| - | - |\n| c | d |\n";

        assert_eq!(render(markdown), render(markdown));
    }
}
