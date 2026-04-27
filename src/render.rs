use comrak::{Options, markdown_to_html};

pub fn render(markdown: &str) -> String {
    markdown_to_html(markdown, &render_options())
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
    fn render_is_deterministic() {
        let markdown = "# Title\n\n- [x] item\n\n| a | b |\n| - | - |\n| c | d |\n";

        assert_eq!(render(markdown), render(markdown));
    }
}
