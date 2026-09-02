use crate::assets;

const TEMPLATE: &str = include_str!("../discuss.html");
const DOC_CONTENT_OPEN: &str = "<section id=\"doc-content\">";
const DOC_CONTENT_CLOSE: &str = "</section>";
const INITIAL_STATE_INSERT_BEFORE: &str = "<script>\n(function() {";
const INITIAL_STATE_SCRIPT_OPEN: &str = "<script id=\"discuss-initial-state\">";
const INITIAL_STATE_SCRIPT_CLOSE: &str = "</script>";
const MERMAID_SHIM_SCRIPT_OPEN: &str = "<script id=\"discuss-mermaid-shim\">";
const MERMAID_SHIM_SCRIPT_CLOSE: &str = "</script>";
const RENDERED_FILES_SCRIPT_OPEN: &str = "<script id=\"discuss-rendered-files\">";
const RENDERED_FILES_SCRIPT_CLOSE: &str = "</script>";

pub fn render_page(
    rendered_markdown: &str,
    initial_state_json: &str,
    rendered_files_json: &str,
) -> String {
    let page = inject_doc_content(TEMPLATE, rendered_markdown);
    let page = inject_initial_state(&page, initial_state_json);
    let page = inject_rendered_files(&page, rendered_files_json);
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

fn inject_rendered_files(page: &str, rendered_files_json: &str) -> String {
    let rendered_files_script = format!(
        "{RENDERED_FILES_SCRIPT_OPEN}\nwindow.__DISCUSS_RENDERED_FILES__ = {};\n{RENDERED_FILES_SCRIPT_CLOSE}\n\n",
        js_safe_json(rendered_files_json)
    );

    inject_before_main_script(page, &rendered_files_script)
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
        let html = without_injected_script(
            &html,
            RENDERED_FILES_SCRIPT_OPEN,
            RENDERED_FILES_SCRIPT_CLOSE,
        );
        without_injected_script(&html, MERMAID_SHIM_SCRIPT_OPEN, MERMAID_SHIM_SCRIPT_CLOSE)
    }

    #[test]
    fn injects_rendered_markdown_inside_doc_content() {
        let page = render_page("<h1>Injected</h1>\n<p>Body</p>\n", "{}", "[]");

        assert_eq!(
            doc_content_inner(&page),
            "\n<h1>Injected</h1>\n<p>Body</p>\n"
        );
    }

    #[test]
    fn seeds_initial_state_json_before_main_script() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#, "[]");

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
        let page = render_page(rendered_markdown, "{}", "[]");

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
    fn seeds_rendered_files_json_before_main_script() {
        let page = render_page("<p>Doc</p>", "{}", r#"[{"id":"f-1","html":"<h1>hi</h1>"}]"#);

        let files_script_start = page
            .find(RENDERED_FILES_SCRIPT_OPEN)
            .expect("rendered-files script should be present");
        let main_script_start = page
            .find(INITIAL_STATE_INSERT_BEFORE)
            .expect("main script should be present");

        assert!(files_script_start < main_script_start);
        assert!(page.contains(
            r#"window.__DISCUSS_RENDERED_FILES__ = [{"id":"f-1","html":"\u003ch1>hi\u003c/h1>"}];"#
        ));
        assert_eq!(page.matches(RENDERED_FILES_SCRIPT_OPEN).count(), 1);
    }

    #[test]
    fn initial_state_json_is_safe_inside_script_tag() {
        let page = render_page("<p>Doc</p>", r#"{"text":"</script><p>break</p>"}"#, "[]");

        assert!(page.contains(r#"{"text":"\u003c/script>\u003cp>break\u003c/p>"}"#));
        assert_eq!(page.matches(INITIAL_STATE_SCRIPT_OPEN).count(), 1);
    }

    #[test]
    fn bundled_template_wires_prism_for_syntax_highlighting() {
        let page = render_page("<p>Doc</p>", "{}", "[]");

        assert!(page.contains("https://unpkg.com/prismjs@1.30.0/themes/prism.min.css"));
        assert!(page.contains("https://unpkg.com/prismjs@1.30.0/themes/prism-tomorrow.min.css"));
        assert!(page.contains(
            "https://unpkg.com/prismjs@1.30.0/plugins/diff-highlight/prism-diff-highlight.min.css"
        ));
        assert!(page.contains(
            "https://unpkg.com/prismjs@1.30.0/plugins/line-numbers/prism-line-numbers.min.css"
        ));
        assert!(page.contains("https://unpkg.com/prismjs@1.30.0/components/prism-core.min.js"));
        assert!(page.contains(
            "https://unpkg.com/prismjs@1.30.0/plugins/autoloader/prism-autoloader.min.js"
        ));
        assert!(page.contains("Prism.plugins.autoloader.languages_path"));
        assert!(page.contains("window.Prism.manual = true"));
        assert!(page.contains("function highlightCodeBlocks()"));
        assert!(page.contains("Prism.highlightElement(code)"));
        assert!(page.contains("pre.classList.add('line-numbers')"));
    }

    #[test]
    fn bundled_template_skips_prism_for_mermaid_blocks() {
        let page = render_page("<p>Doc</p>", "{}", "[]");

        assert!(page.contains("function isMermaidPre(pre)"));
        assert!(page.contains("language-mermaid"));
        assert!(page.contains("if (isMermaidPre(pre)) return;"));
        assert!(page.contains(".mermaid-block"));
        assert!(page.contains(".mermaid-error"));
    }

    #[test]
    fn bundled_template_includes_theme_toggle_with_system_default() {
        let page = render_page("<p>Doc</p>", "{}", "[]");

        assert!(page.contains(r#"id="theme-toggle""#));
        assert!(page.contains("theme-icon-system"));
        assert!(page.contains("theme-icon-light"));
        assert!(page.contains("theme-icon-dark"));
        assert!(page.contains("function applyThemeMode(mode)"));
        assert!(page.contains("function initThemeToggle()"));
        assert!(page.contains("'discuss-theme'"));
        assert!(page.contains("'(prefers-color-scheme: dark)'"));
        assert!(page.contains(r#"html[data-theme="dark"]"#));
        assert!(page.contains("link[data-prism-theme]"));

        let bootstrap_script = page
            .find("id=\"discuss-theme-bootstrap\"")
            .expect("pre-paint theme bootstrap script should be present");
        let body_open = page.find("<body>").expect("body open");
        assert!(
            bootstrap_script < body_open,
            "theme bootstrap must run before body paint",
        );
    }

    #[test]
    fn injects_mermaid_shim_before_main_script() {
        let page = render_page(
            "<pre><code class=\"language-mermaid\">flowchart TD</code></pre>",
            "{}",
            "[]",
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
    fn bundled_template_wires_html_prototype_inspection() {
        let page = render_page(
            r#"<div class="html-review"><iframe class="prototype-frame" sandbox="allow-scripts allow-same-origin"></iframe></div>"#,
            "{}",
            "[]",
        );

        assert!(page.contains(r#"id="inspect-toggle""#));
        assert!(page.contains("discuss:set-inspect"));
        assert!(page.contains("discuss:element-selected"));
        assert!(page.contains("discuss:resolve-anchors"));
        assert!(page.contains("function prototypeOrigin(frame)"));
        assert!(page.contains("event.source !== frame.contentWindow || event.origin !== origin"));
        assert!(page.contains("discuss:route-changed"));
        assert!(page.contains("discuss:external-navigation"));
        assert!(page.contains("function openHtmlThreadEditor(selection)"));
        assert!(page.contains("${escapeHtml(rangeLabel)}"));
        assert!(page.contains("elementAnchor"));
        assert!(page.contains("sandbox=\"allow-scripts allow-same-origin\""));
    }

    #[test]
    fn bundled_template_hydrates_state_from_seed_or_api() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#, "[]");

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
        // localStorage may only persist UI preferences (theme, ⌘-Enter-to-send,
        // sidebar collapse), never document/thread state. The old
        // state-in-localStorage pattern used STORAGE_KEY = 'discuss-state' —
        // that must stay removed.
        for (offset, _) in page.match_indices("localStorage") {
            let window_end = (offset + 80).min(page.len());
            let context = &page[offset..window_end];
            assert!(
                context.contains("discuss-theme")
                    || context.contains("THEME_STORAGE_KEY")
                    || context.contains("CMD_ENTER_KEY")
                    || context.contains("FILES_COLLAPSED_KEY"),
                "localStorage may only persist UI preferences; saw: {context}",
            );
        }
        assert!(!page.contains("STORAGE_KEY = 'discuss-state'"));
    }

    #[test]
    fn bundled_template_does_not_resegment_markdown_anchors() {
        let page = render_page("<p data-anchor-idx=\"1\">Doc</p>", "{}", "[]");

        assert!(!page.contains("COMMENTABLE_SELECTOR"));
        assert!(!page.contains("assignAnchorIndices"));
        assert!(!page.contains("setAttribute('data-anchor-idx'"));
        assert!(page.contains("<p data-anchor-idx=\"1\">Doc</p>"));
    }

    #[test]
    fn bundled_template_has_accessible_collapsible_file_sidebar() {
        let page = render_page("<p>Doc</p>", "{}", "[]");

        // Toggle markup + accessibility wiring.
        assert!(page.contains("file-sidebar-toggle"));
        assert!(page.contains("toggle.setAttribute('aria-controls', 'file-sidebar')"));
        assert!(
            page.contains("toggle.setAttribute('aria-expanded', collapsed ? 'false' : 'true')")
        );
        assert!(page.contains("'Expand file list'"));
        assert!(page.contains("'Collapse file list'"));
        // Sighted mouse users get the same hover affordance as AT users.
        assert!(page.contains("toggle.title = label"));
        // aria-label overrides button content for assistive tech, so the whole
        // accessible name is composed: path, plus the file kind (the icon is
        // aria-hidden and the .file-kind tag is hidden in the collapsed rail),
        // plus the open-thread count whenever the badges refresh.
        assert!(page.contains("item.dataset.a11yBase"));
        assert!(page.contains("item.setAttribute('aria-label', item.dataset.a11yBase)"));
        assert!(page.contains("file.kind !== 'markdown' ? `${path}, ${file.kind}` : path"));
        assert!(page.contains("${open} open thread"));

        // Collapsed rail width sits in the required 48-52px range via one var.
        assert!(page.contains("body.multi-file.files-collapsed { --files-w: 50px; }"));

        // Per-kind icons for markdown, diff, image, and HTML files.
        assert!(page.contains("const FILE_KIND_ICONS"));
        for kind in ["markdown", "diff", "image", "html"] {
            assert!(
                page.contains(&format!("{kind}: '<")),
                "expected an icon for kind {kind}"
            );
        }
        assert!(page.contains("file-icon file-icon-"));
        assert!(page.contains("svg.setAttribute('aria-hidden', 'true')"));

        // Compact open-thread badge survives collapse.
        assert!(page.contains("body.files-collapsed .file-item .file-count"));

        // Open thread cards are positioned in pixels and a class toggle fires
        // no resize event, so the toggle must reposition them explicitly.
        // (Markers, minimap, and image pins re-anchor from CSS on their own.)
        let toggle_setup = page
            .find("toggle.className = 'file-sidebar-toggle'")
            .expect("sidebar toggle construction");
        let toggle_handler = page[toggle_setup..]
            .find("toggle.addEventListener('click'")
            .expect("sidebar toggle click handler")
            + toggle_setup;
        let reposition = page[toggle_handler..]
            .find("scheduleReposition();")
            .expect("toggle handler must reschedule repositioning");
        assert!(
            reposition < 1000,
            "scheduleReposition should be inside the toggle handler"
        );
    }

    #[test]
    fn file_sidebar_collapse_pref_defaults_to_expanded_and_persists_ui_pref_only() {
        let page = render_page("<p>Doc</p>", "{}", "[]");

        assert!(page.contains("const FILES_COLLAPSED_KEY = 'discuss-files-collapsed'"));
        // Absent/anything-but-'1' reads as expanded: the first-run default.
        assert!(page.contains("return stored === '1';"));
        assert!(page.contains("localStorage.getItem(FILES_COLLAPSED_KEY)"));
        assert!(page.contains("localStorage.setItem(FILES_COLLAPSED_KEY, collapsed ? '1' : '0')"));
        // Only the UI pref is stored - never review state.
        assert!(!page.contains("localStorage.setItem('discuss-state'"));
    }

    #[test]
    fn bundled_template_links_thread_summary_to_thread_cards() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#, "[]");

        assert!(page.contains("class=\"toggle thread-summary-toggle\""));
        assert!(page.contains("function threadSummaryEntries(state)"));
        assert!(page.contains("function jumpToThread(threadId, fileId)"));
        assert!(page.contains("jumpToThread(threadId, fileId)"));
        assert!(page.contains("openThread(thread)"));
    }

    #[test]
    fn bundled_template_supports_image_pin_threads() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#, "[]");

        assert!(page.contains("function normalizeImageAnchor(raw)"));
        assert!(page.contains("function openImagePinEditor(imageAnchor)"));
        assert!(page.contains("imageAnchor: { ...imageAnchor }"));
        assert!(page.contains("function renderImagePins(state)"));
        assert!(page.contains("className = 'image-pin-marker'"));
        assert!(page.contains("`📍 Pin ${t.anchorStart}`"));
    }

    #[test]
    fn bundled_template_sends_thread_mutations_to_rest_api() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#, "[]");

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
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#, "[]");

        assert!(page.contains("function persistNewThreadDraft"));
        assert!(page.contains("function queueDraftRequest"));
        assert!(page.contains("apiJson('/api/drafts/new-thread'"));
        assert!(page.contains("method: 'DELETE', body: { fileId, anchorStart, anchorEnd }"));
        assert!(page.contains("function persistFollowupDraft"));
        assert!(page.contains("apiJson('/api/drafts/followup'"));
        assert!(page.contains("method: 'DELETE', body: { threadId }"));
        assert!(!page.contains("saveState(s);"));
    }

    #[test]
    fn bundled_template_surfaces_rest_mutation_failures_inline() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#, "[]");

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
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#, "[]");

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
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#, "[]");

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
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#, "[]");

        assert!(page.contains("return sortCommentsByCreatedAt(items);"));
        assert!(page.contains("function sortCommentsByCreatedAt(items)"));
        assert!(page.contains("function commentTimestamp(item)"));
        assert!(page.contains(r#".user-comment[data-kind="take"]"#));
        assert!(page.contains("const metaPrefix = it.kind === 'take' ? 'Agent take"));
    }

    #[test]
    fn bundled_template_marks_thread_contributor_state() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#, "[]");

        assert!(page.contains(".thread-marker.kind-mixed"));
        assert!(page.contains("function markerKindForThread(state, threadId, prep)"));
        assert!(page.contains("if (hasTake && hasReply) return 'mixed';"));
        assert!(page.contains("if (hasTake) return 'pending';"));
        assert!(page.contains("function latestContributorForThread(state, threadId, prep)"));
        assert!(page.contains("latest: ${latestContributorForThread(state, tid, prep)}"));
    }

    #[test]
    fn bundled_template_finishes_session_through_done_api() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#, "[]");

        assert!(page.contains(">Finish review</button>"));
        assert!(page.contains("You can close this tab."));
        assert!(page.contains("await apiJson('/api/done')"));
        assert!(page.contains("function markReviewComplete()"));
        assert!(page.contains("reviewComplete = true;"));
        assert!(page.contains("if (reviewComplete) return;"));
        assert!(page.contains("document.body.classList.add('review-complete')"));
        assert!(page.contains("showMutationError(doneControl, \"couldn't finish"));
        assert!(!page.contains("window.prompt"));
        assert!(!page.contains("function buildCopyText"));
    }

    #[test]
    fn bundled_template_shows_version_history_and_update_copy() {
        let page = render_page("<p>Doc</p>", r#"{"threads":[]}"#, "[]");

        assert!(page.contains("<h1>Discuss</h1>"));
        assert!(!page.contains("Contextual Discussion"));
        assert!(page.contains("id=\"header-version\""));
        assert!(page.contains("fetch('/api/version'"));
        assert!(page.contains("const UPDATE_COMMAND = 'discuss update -y';"));
        assert!(page.contains("What’s new since v${info.current}"));
        assert!(page.contains("(info.releases || []).forEach"));
        assert!(page.contains("copyTextToClipboard(UPDATE_COMMAND)"));
    }
}
