pub const MERMAID_JS: &str = include_str!("../assets/mermaid.min.js");
pub const MERMAID_SHIM_JS: &str = include_str!("../assets/mermaid-shim.js");
pub const PREACT_JS: &str = include_str!("../assets/preact.umd.js");
pub const PREACT_HOOKS_JS: &str = include_str!("../assets/preact-hooks.umd.js");
pub const HTM_JS: &str = include_str!("../assets/htm.umd.js");
pub const DISCUSS_V2_HTML: &str = include_str!("../assets/discuss-v2.html");

pub fn mermaid_js() -> &'static str {
    MERMAID_JS
}

pub fn mermaid_shim_js() -> &'static str {
    MERMAID_SHIM_JS
}

pub fn preact_js() -> &'static str {
    PREACT_JS
}

pub fn preact_hooks_js() -> &'static str {
    PREACT_HOOKS_JS
}

pub fn htm_js() -> &'static str {
    HTM_JS
}

pub fn discuss_v2_html() -> &'static str {
    DISCUSS_V2_HTML
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shim_references_mermaid_selector_and_bundled_asset_path() {
        assert!(mermaid_shim_js().contains("pre > code.language-mermaid"));
        assert!(mermaid_shim_js().contains("/assets/mermaid.min.js"));
    }

    #[test]
    fn shim_loads_mermaid_only_after_finding_blocks() {
        let shim = mermaid_shim_js();
        let empty_check = shim
            .find("if (!blocks.length) return;")
            .expect("empty check");
        let script_create = shim
            .find("document.createElement('script')")
            .expect("script creation");

        assert!(empty_check < script_create);
    }

    #[test]
    fn mermaid_asset_is_bundled_and_within_size_budget() {
        assert!(mermaid_js().contains("mermaidAPI"));
        assert!(mermaid_js().len() < 700 * 1024);
    }

    #[test]
    fn preact_umd_is_bundled_and_within_size_budget() {
        let body = preact_js();
        assert!(body.contains("preact"), "expected preact UMD marker");
        assert!(body.len() < 32 * 1024);
    }

    #[test]
    fn htm_umd_is_bundled_and_within_size_budget() {
        let body = htm_js();
        assert!(body.contains("htm") || body.contains("module.exports"));
        assert!(body.len() < 8 * 1024);
    }

    #[test]
    fn preact_hooks_umd_is_bundled_and_within_size_budget() {
        let body = preact_hooks_js();
        assert!(body.contains("module") || body.contains("exports"));
        assert!(body.len() < 16 * 1024);
    }

    #[test]
    fn discuss_v2_template_loads_preact_htm_and_seeds_initial_state() {
        let template = discuss_v2_html();
        assert!(template.contains("/assets/preact.umd.js"));
        assert!(template.contains("/assets/preact-hooks.umd.js"));
        assert!(template.contains("/assets/htm.umd.js"));
        assert!(template.contains(r#"id="discuss-initial-state""#));
        assert!(template.contains(r#"id="discuss-rendered-files""#));
        assert!(template.contains("window.__DISCUSS_INITIAL_STATE__ = {};"));
        assert!(template.contains("window.__DISCUSS_RENDERED_FILES__ = {};"));
        assert!(template.contains(r#"id="app""#));
    }

    #[test]
    fn discuss_v2_template_assigns_anchor_indices_and_renders_threads_panel() {
        let template = discuss_v2_html();
        assert!(template.contains("setAttribute('data-anchor-idx'"));
        assert!(template.contains("v2-threads-panel"));
        assert!(template.contains("function ThreadsPanel"));
        assert!(template.contains("function scrollToAnchor"));
    }

    #[test]
    fn discuss_v2_template_dispatches_sse_events_through_state_setters() {
        let template = discuss_v2_html();
        assert!(template.contains("'thread.created'"));
        assert!(template.contains("'thread.deleted'"));
        assert!(template.contains("'thread.resolved'"));
        assert!(template.contains("'thread.unresolved'"));
        assert!(template.contains("'reply.added'"));
        assert!(template.contains("'take.added'"));
        assert!(template.contains("function dispatchSseEvent"));
        assert!(template.contains("setThreads(prev"));
    }

    #[test]
    fn discuss_v2_template_sends_browser_heartbeat() {
        let template = discuss_v2_html();
        assert!(template.contains("fetch('/api/heartbeat'"));
        assert!(template.contains("HEARTBEAT_INTERVAL_MS"));
    }

    #[test]
    fn discuss_v2_template_wires_selection_popup_and_new_thread_editor() {
        let template = discuss_v2_html();
        assert!(template.contains("function SelectionPopup"));
        assert!(template.contains("function NewThreadEditor"));
        assert!(template.contains("function currentSelectionAnchors"));
        assert!(template.contains("function anchorElementFor"));
        assert!(template.contains("addEventListener('selectionchange'"));
        assert!(template.contains("v2-selection-popup"));
        assert!(template.contains("v2-new-thread-editor"));
    }

    #[test]
    fn discuss_v2_template_posts_threads_with_optimistic_rollback() {
        let template = discuss_v2_html();
        assert!(template.contains("apiJson('/api/threads'"));
        assert!(template.contains("function submitNewThread"));
        assert!(template.contains("__optimistic: true"));
        assert!(template.contains("setThreads(prev => prev.filter(t => t.id !== tempId))"));
    }

    #[test]
    fn discuss_v2_template_supports_thread_replies_resolve_unresolve_delete() {
        let template = discuss_v2_html();
        assert!(template.contains("async function postReply"));
        assert!(template.contains("async function resolveThread"));
        assert!(template.contains("async function unresolveThread"));
        assert!(template.contains("async function deleteThread"));
        assert!(template.contains("/replies"));
        assert!(template.contains("/resolve"));
        assert!(template.contains("/unresolve"));
        assert!(template.contains("method: 'DELETE'"));
        assert!(template.contains("function ThreadComment"));
        assert!(template.contains("v2-thread-timeline"));
        assert!(template.contains("v2-thread-actions"));
    }

    #[test]
    fn discuss_v2_template_hides_file_tree_when_single_file_and_shows_thread_badges() {
        let template = discuss_v2_html();
        assert!(template.contains("data-single-file"));
        assert!(template.contains("data-section-file-id"));
        assert!(template.contains("threadCounts"));
        assert!(template.contains("v2-tree-badge"));
        assert!(template.contains("function handleSelectFile"));
        assert!(template.contains("scrollIntoView"));
    }

    #[test]
    fn discuss_v2_template_renders_diff_files_per_hunk_with_language_class() {
        let template = discuss_v2_html();
        assert!(template.contains("function parseDiffFile"));
        assert!(template.contains("function languageFromPath"));
        assert!(template.contains("DIFF_LANGUAGE_BY_EXT"));
        assert!(template.contains("v2-diff-hunk"));
        assert!(template.contains("v2-diff-header"));
        assert!(template.contains("data-hunk-idx"));
        assert!(template.contains("language-diff-${lang}"));
        assert!(template.contains("v2-diff-add"));
        assert!(template.contains("v2-diff-del"));
        assert!(template.contains("Binary files"));
    }

    #[test]
    fn discuss_v2_template_lazy_highlights_diff_hunks_via_intersection_observer() {
        let template = discuss_v2_html();
        assert!(template.contains("function observeForLazyHighlight"));
        assert!(template.contains("IntersectionObserver"));
        assert!(template.contains("dataset.prismHighlighted"));
        assert!(template.contains("rootMargin: '200px 0px'"));
    }

    #[test]
    fn discuss_v2_template_supports_file_tree_filter_and_open_only_toggle() {
        let template = discuss_v2_html();
        assert!(template.contains("v2-tree-controls"));
        assert!(template.contains("placeholder=\"filter files…\""));
        assert!(template.contains("only with open threads"));
        assert!(template.contains("setOpenOnly"));
        assert!(template.contains("file.path.toLowerCase().includes(trimmed)"));
    }

    #[test]
    fn discuss_v2_template_supports_three_mode_theme_toggle_with_storage() {
        let template = discuss_v2_html();
        assert!(template.contains("THEME_STORAGE_KEY"));
        assert!(template.contains("'discuss-theme'"));
        assert!(template.contains("function ThemeToggle"));
        assert!(template.contains("function applyThemeMode"));
        assert!(template.contains("function nextThemeMode"));
        assert!(template.contains("v2-theme-toggle"));
        assert!(template.contains("html[data-theme=\"dark\"]"));
        assert!(template.contains("root.dataset.themeMode"));
    }

    #[test]
    fn discuss_v2_template_lazy_loads_mermaid_for_markdown_blocks() {
        let template = discuss_v2_html();
        assert!(template.contains("function loadMermaid"));
        assert!(template.contains("function renderMermaidUnder"));
        assert!(template.contains("/assets/mermaid.min.js"));
        assert!(template.contains("pre > code.language-mermaid"));
        assert!(template.contains("data-mermaid-rendered"));
        assert!(template.contains("renderMermaidUnder(articleRef.current)"));
    }

    #[test]
    fn discuss_v2_template_loads_prism_with_diff_plugin_and_highlights_files() {
        let template = discuss_v2_html();
        assert!(template.contains("https://unpkg.com/prismjs@1.30.0/themes/prism.min.css"));
        assert!(
            template.contains("https://unpkg.com/prismjs@1.30.0/themes/prism-tomorrow.min.css")
        );
        assert!(template.contains("prism-diff-highlight.min.css"));
        assert!(template.contains("https://unpkg.com/prismjs@1.30.0/components/prism-core.min.js"));
        assert!(template.contains("prism-autoloader.min.js"));
        assert!(template.contains("prism-diff-highlight.min.js"));
        assert!(template.contains("function highlightWithPrism"));
        assert!(template.contains("Prism.highlightAllUnder"));
        assert!(template.contains("diff-highlight"));
    }

    #[test]
    fn discuss_v2_template_wires_done_flow_with_banner_and_review_lock() {
        let template = discuss_v2_html();
        assert!(template.contains("async function submitDone"));
        assert!(template.contains("apiJson('/api/done')"));
        assert!(template.contains("setReviewComplete"));
        assert!(template.contains("v2-done-bar"));
        assert!(template.contains("v2-done-banner"));
        assert!(template.contains("review-complete"));
        assert!(template.contains("function stopHeartbeat"));
        assert!(template.contains("function stopEventStream"));
    }

    #[test]
    fn discuss_v2_template_keys_new_thread_drafts_by_file_id_and_anchor_range() {
        let template = discuss_v2_html();
        assert!(template.contains("function newThreadDraftKey(fileId, anchorStart, anchorEnd)"));
        assert!(template.contains("function parseLegacyOrFileScopedKey"));
        assert!(template.contains("body: { fileId, anchorStart, anchorEnd, text }"));
        assert!(template.contains("body: { fileId, anchorStart, anchorEnd }"));
        assert!(template.contains("payload.fileId, payload.anchorStart, payload.anchorEnd"));
    }

    #[test]
    fn discuss_v2_template_auto_saves_drafts_through_rest_with_sse_sync() {
        let template = discuss_v2_html();
        assert!(template.contains("function useDraftAutoSave"));
        assert!(template.contains("function newThreadDraftKey"));
        assert!(template.contains("function normalizeDrafts"));
        assert!(template.contains("function saveNewThreadDraft"));
        assert!(template.contains("function clearNewThreadDraft"));
        assert!(template.contains("function saveFollowupDraft"));
        assert!(template.contains("function clearFollowupDraft"));
        assert!(template.contains("/api/drafts/new-thread"));
        assert!(template.contains("/api/drafts/followup"));
        assert!(template.contains("'draft.updated'"));
        assert!(template.contains("'draft.cleared'"));
        assert!(template.contains("onDraftUpdated"));
        assert!(template.contains("onDraftCleared"));
        assert!(template.contains("v2-draft-status"));
    }
}
