const TEMPLATE: &str = include_str!("../discuss.html");
const DOC_CONTENT_OPEN: &str = "<section id=\"doc-content\">";
const DOC_CONTENT_CLOSE: &str = "</section>";
const INITIAL_STATE_INSERT_BEFORE: &str = "<script>\n(function() {";
const INITIAL_STATE_SCRIPT_OPEN: &str = "<script id=\"discuss-initial-state\">";
const INITIAL_STATE_SCRIPT_CLOSE: &str = "</script>";

pub fn render_page(rendered_markdown: &str, initial_state_json: &str) -> String {
    let page = inject_doc_content(TEMPLATE, rendered_markdown);
    inject_initial_state(&page, initial_state_json)
}

fn inject_doc_content(template: &str, rendered_markdown: &str) -> String {
    let section_start = template
        .find(DOC_CONTENT_OPEN)
        .expect("bundled template must contain #doc-content");
    let content_start = section_start + DOC_CONTENT_OPEN.len();
    let section_end = template[content_start..]
        .find(DOC_CONTENT_CLOSE)
        .map(|index| content_start + index)
        .expect("bundled template #doc-content must close");

    let mut page = String::with_capacity(
        template.len() - (section_end - content_start) + rendered_markdown.len() + 2,
    );
    page.push_str(&template[..content_start]);
    page.push('\n');
    page.push_str(rendered_markdown);
    if !rendered_markdown.ends_with('\n') {
        page.push('\n');
    }
    page.push_str(&template[section_end..]);
    page
}

fn inject_initial_state(page: &str, initial_state_json: &str) -> String {
    let insert_at = page
        .find(INITIAL_STATE_INSERT_BEFORE)
        .or_else(|| page.find("</body>"))
        .expect("bundled template must contain a script block or closing body");
    let initial_state_script = format!(
        "{INITIAL_STATE_SCRIPT_OPEN}\nwindow.__DISCUSS_INITIAL_STATE__ = {};\n{INITIAL_STATE_SCRIPT_CLOSE}\n\n",
        js_safe_json(initial_state_json)
    );

    let mut rendered = String::with_capacity(page.len() + initial_state_script.len());
    rendered.push_str(&page[..insert_at]);
    rendered.push_str(&initial_state_script);
    rendered.push_str(&page[insert_at..]);
    rendered
}

fn js_safe_json(json: &str) -> String {
    json.replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_content_inner(html: &str) -> &str {
        let content_start =
            html.find(DOC_CONTENT_OPEN).expect("doc-content start") + DOC_CONTENT_OPEN.len();
        let content_end = html[content_start..]
            .find(DOC_CONTENT_CLOSE)
            .expect("doc-content end")
            + content_start;

        &html[content_start..content_end]
    }

    fn without_initial_state_script(html: &str) -> String {
        let script_start = html
            .find(INITIAL_STATE_SCRIPT_OPEN)
            .expect("initial state script start");
        let script_end = html[script_start..]
            .find(INITIAL_STATE_SCRIPT_CLOSE)
            .expect("initial state script end")
            + script_start
            + INITIAL_STATE_SCRIPT_CLOSE.len();
        let trailing_newlines = html[script_end..]
            .chars()
            .take_while(|character| *character == '\n')
            .map(char::len_utf8)
            .sum::<usize>();

        let mut stripped = String::new();
        stripped.push_str(&html[..script_start]);
        stripped.push_str(&html[script_end + trailing_newlines..]);
        stripped
    }

    #[test]
    fn injects_rendered_markdown_inside_doc_content() {
        let page = render_page("<h1>Injected</h1>\n<p>Body</p>\n", "{}");

        assert_eq!(
            doc_content_inner(&page),
            "\n<h1>Injected</h1>\n<p>Body</p>\n"
        );
    }

    #[test]
    fn seeds_initial_state_json_before_main_script() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#);

        let state_script_start = page
            .find(INITIAL_STATE_SCRIPT_OPEN)
            .expect("state script should be present");
        let main_script_start = page
            .find(INITIAL_STATE_INSERT_BEFORE)
            .expect("main script should be present");

        assert!(state_script_start < main_script_start);
        assert!(page.contains(r#"window.__DISCUSS_INITIAL_STATE__ = {"threads":[]};"#));
    }

    #[test]
    fn preserves_template_markup_outside_injection_points() {
        let rendered_markdown = "<h1>Injected</h1>\n";
        let expected_without_state = inject_doc_content(TEMPLATE, rendered_markdown);
        let page = render_page(rendered_markdown, "{}");

        assert_eq!(without_initial_state_script(&page), expected_without_state);
    }

    #[test]
    fn doc_content_injection_handles_empty_sections() {
        let template = r#"<main><section id="doc-content"></section><aside>keep</aside></main>"#;

        let page = inject_doc_content(template, "<p>Inserted</p>");

        assert_eq!(
            page,
            r#"<main><section id="doc-content">
<p>Inserted</p>
</section><aside>keep</aside></main>"#
        );
    }

    #[test]
    fn initial_state_json_is_safe_inside_script_tag() {
        let page = render_page("<p>Doc</p>", r#"{"text":"</script><p>break</p>"}"#);

        assert!(page.contains(r#"{"text":"\u003c/script>\u003cp>break\u003c/p>"}"#));
        assert_eq!(page.matches(INITIAL_STATE_SCRIPT_OPEN).count(), 1);
    }
}
