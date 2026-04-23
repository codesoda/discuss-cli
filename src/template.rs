use crate::assets;

const TEMPLATE: &str = include_str!("../discuss.html");
const DOC_CONTENT_OPEN: &str = "<section id=\"doc-content\">";
const DOC_CONTENT_CLOSE: &str = "</section>";
const INITIAL_STATE_INSERT_BEFORE: &str = "<script>\n(function() {";
const INITIAL_STATE_SCRIPT_OPEN: &str = "<script id=\"discuss-initial-state\">";
const INITIAL_STATE_SCRIPT_CLOSE: &str = "</script>";
const MERMAID_SHIM_SCRIPT_OPEN: &str = "<script id=\"discuss-mermaid-shim\">";
const MERMAID_SHIM_SCRIPT_CLOSE: &str = "</script>";

pub fn render_page(rendered_markdown: &str, initial_state_json: &str) -> String {
    let page = inject_doc_content(TEMPLATE, rendered_markdown);
    let page = inject_initial_state(&page, initial_state_json);
    inject_mermaid_shim(&page)
}

fn inject_doc_content(template: &str, rendered_markdown: &str) -> String {
    let section_start =
        find_doc_content_open(template).expect("bundled template must contain #doc-content");
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

fn find_doc_content_open(html: &str) -> Option<usize> {
    let search_start = html
        .find("<body")
        .and_then(|body_start| html[body_start..].find('>').map(|end| body_start + end + 1))
        .unwrap_or(0);

    html[search_start..]
        .find(DOC_CONTENT_OPEN)
        .map(|index| search_start + index)
}

fn inject_initial_state(page: &str, initial_state_json: &str) -> String {
    let initial_state_script = format!(
        "{INITIAL_STATE_SCRIPT_OPEN}\nwindow.__DISCUSS_INITIAL_STATE__ = {};\n{INITIAL_STATE_SCRIPT_CLOSE}\n\n",
        js_safe_json(initial_state_json)
    );

    inject_before_main_script(page, &initial_state_script)
}

fn inject_mermaid_shim(page: &str) -> String {
    let mermaid_shim_script = format!(
        "{MERMAID_SHIM_SCRIPT_OPEN}\n{}\n{MERMAID_SHIM_SCRIPT_CLOSE}\n\n",
        assets::mermaid_shim_js()
    );

    inject_before_main_script(page, &mermaid_shim_script)
}

fn inject_before_main_script(page: &str, insertion: &str) -> String {
    let insert_at = page
        .find(INITIAL_STATE_INSERT_BEFORE)
        .or_else(|| page.find("</body>"))
        .expect("bundled template must contain a script block or closing body");

    let mut rendered = String::with_capacity(page.len() + insertion.len());
    rendered.push_str(&page[..insert_at]);
    rendered.push_str(insertion);
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
            find_doc_content_open(html).expect("doc-content start") + DOC_CONTENT_OPEN.len();
        let content_end = html[content_start..]
            .find(DOC_CONTENT_CLOSE)
            .expect("doc-content end")
            + content_start;

        &html[content_start..content_end]
    }

    fn without_injected_script(html: &str, script_open: &str, script_close: &str) -> String {
        let script_start = html.find(script_open).expect("injected script start");
        let script_end = html[script_start..]
            .find(script_close)
            .expect("injected script end")
            + script_start
            + script_close.len();
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

    fn without_injected_scripts(html: &str) -> String {
        let html =
            without_injected_script(html, INITIAL_STATE_SCRIPT_OPEN, INITIAL_STATE_SCRIPT_CLOSE);
        without_injected_script(&html, MERMAID_SHIM_SCRIPT_OPEN, MERMAID_SHIM_SCRIPT_CLOSE)
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

        assert_eq!(without_injected_scripts(&page), expected_without_state);
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

    #[test]
    fn injects_mermaid_shim_before_main_script() {
        let page = render_page(
            "<pre><code class=\"language-mermaid\">flowchart TD</code></pre>",
            "{}",
        );

        let shim_script_start = page
            .find(MERMAID_SHIM_SCRIPT_OPEN)
            .expect("mermaid shim script should be present");
        let main_script_start = page
            .find(INITIAL_STATE_INSERT_BEFORE)
            .expect("main script should be present");

        assert!(shim_script_start < main_script_start);
        assert!(page.contains("pre > code.language-mermaid"));
        assert!(page.contains("/assets/mermaid.min.js"));
    }

    #[test]
    fn bundled_template_hydrates_state_from_seed_or_api() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#);

        let seed_check = page
            .find("if (stateSeed)")
            .expect("template should prefer server-rendered state seed");
        let api_fetch = page
            .find("fetch('/api/state'")
            .expect("template should fall back to GET /api/state");

        assert!(seed_check < api_fetch);
        assert!(page.contains("function normalizeState(raw)"));
        assert!(page.contains("raw.threads"));
        assert!(page.contains("raw.replies"));
        assert!(page.contains("draft.updatedAt"));
        assert!(!page.contains("localStorage"));
        assert!(!page.contains("STORAGE_KEY"));
    }

    #[test]
    fn bundled_template_sends_thread_mutations_to_rest_api() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#);

        assert!(page.contains("await apiJson('/api/threads'"));
        assert!(page.contains("await apiJson(threadApiPath(threadId, '/replies')"));
        assert!(page.contains("await apiJson(threadApiPath(threadId, '/resolve')"));
        assert!(page.contains("await apiJson(threadApiPath(threadId, '/unresolve')"));
        assert!(page.contains("await apiJson(threadApiPath(threadId), { method: 'DELETE' })"));
        assert!(!page.contains("delete-comment"));
        assert!(!page.contains("s.followups[tid].splice"));
    }

    #[test]
    fn bundled_template_sends_draft_mutations_to_rest_api() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#);

        assert!(page.contains("function persistNewThreadDraft"));
        assert!(page.contains("function queueDraftRequest"));
        assert!(page.contains("apiJson('/api/drafts/new-thread'"));
        assert!(page.contains("method: 'DELETE', body: { anchorStart, anchorEnd }"));
        assert!(page.contains("function persistFollowupDraft"));
        assert!(page.contains("apiJson('/api/drafts/followup'"));
        assert!(page.contains("method: 'DELETE', body: { threadId }"));
        assert!(!page.contains("saveState(s);"));
    }

    #[test]
    fn bundled_template_surfaces_rest_mutation_failures_inline() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#);

        assert!(page.contains(".mutation-error"));
        assert!(page.contains("function showMutationError"));
        assert!(page.contains("button.textContent = 'Retry'"));
        assert!(page.contains("showMutationError(followup, \"couldn't save"));
        assert!(page.contains("showMutationError(followup, \"couldn't resolve"));
        assert!(page.contains("showMutationError(restored, \"couldn't delete"));
        assert!(page.contains("showMutationError(newThreadEditor, \"couldn't save"));
        assert!(page.contains("function showDraftMutationError"));
        assert!(page.contains("\"couldn't save draft"));
        assert!(page.contains("\"couldn't clear draft"));
        assert!(!page.contains("alert("));
    }

    #[test]
    fn bundled_template_subscribes_to_sse_and_applies_incremental_events() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#);

        assert!(page.contains("new EventSource('/api/events')"));
        assert!(page.contains("'thread.created'"));
        assert!(page.contains("'thread.deleted'"));
        assert!(page.contains("'thread.resolved'"));
        assert!(page.contains("'thread.unresolved'"));
        assert!(page.contains("'reply.added'"));
        assert!(page.contains("'take.added'"));
        assert!(page.contains("'draft.updated'"));
        assert!(page.contains("'draft.cleared'"));
        assert!(page.contains("function applyServerEvent(kind, payload)"));
        assert!(page.contains("function scheduleEventReconnect()"));
        assert!(page.contains("refreshAllThreadsFromState()"));
        assert!(page.contains("refreshThreadFromState(reply.threadId)"));
    }

    #[test]
    fn bundled_template_sends_browser_heartbeat() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#);

        assert!(page.contains("const HEARTBEAT_INTERVAL_MS = 30000"));
        assert!(page.contains("fetch('/api/heartbeat'"));
        assert!(page.contains("setInterval(sendHeartbeat, HEARTBEAT_INTERVAL_MS)"));
        assert!(page.contains("clearInterval(heartbeatTimer)"));

        let event_stream = page
            .find("startEventStream();")
            .expect("template should start SSE");
        let heartbeat = page
            .find("startHeartbeat();")
            .expect("template should start heartbeat");

        assert!(event_stream < heartbeat);
    }

    #[test]
    fn bundled_template_interleaves_takes_and_replies_chronologically() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#);

        assert!(page.contains("return sortCommentsByCreatedAt(items);"));
        assert!(page.contains("function sortCommentsByCreatedAt(items)"));
        assert!(page.contains("function commentTimestamp(item)"));
        assert!(page.contains(r#".user-comment[data-kind="take"]"#));
        assert!(page.contains("const metaPrefix = it.kind === 'take' ? 'Agent take"));
    }

    #[test]
    fn bundled_template_marks_thread_contributor_state() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#);

        assert!(page.contains(".thread-marker.kind-mixed"));
        assert!(page.contains("function markerKindForThread(state, threadId, prep)"));
        assert!(page.contains("if (hasTake && hasReply) return 'mixed';"));
        assert!(page.contains("if (hasTake) return 'pending';"));
        assert!(page.contains("function latestContributorForThread(state, threadId, prep)"));
        assert!(page.contains("latest: ${latestContributorForThread(state, tid, prep)}"));
    }

    #[test]
    fn bundled_template_finishes_session_through_done_api() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#);

        assert!(page.contains("Done \u{2014} send to chat"));
        assert!(page.contains("You can close this tab."));
        assert!(page.contains("await apiJson('/api/done')"));
        assert!(page.contains("function markReviewComplete()"));
        assert!(page.contains("reviewComplete = true;"));
        assert!(page.contains("if (reviewComplete) return;"));
        assert!(page.contains("document.body.classList.add('review-complete')"));
        assert!(page.contains("showMutationError(doneControl, \"couldn't finish"));
        assert!(!page.contains("navigator.clipboard"));
        assert!(!page.contains("window.prompt"));
        assert!(!page.contains("function buildCopyText"));
    }
}
