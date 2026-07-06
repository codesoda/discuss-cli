pub const MERMAID_JS: &str = include_str!("../assets/mermaid.min.js");
pub const MERMAID_SHIM_JS: &str = include_str!("../assets/mermaid-shim.js");

pub fn mermaid_js() -> &'static str {
    MERMAID_JS
}

pub fn mermaid_shim_js() -> &'static str {
    MERMAID_SHIM_JS
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
        assert!(mermaid_js().len() < 4 * 1024 * 1024);
    }

    #[test]
    fn shim_uses_modern_mermaid_api_and_loose_security() {
        let shim = mermaid_shim_js();
        assert!(shim.contains("startOnLoad: false"));
        assert!(shim.contains("securityLevel: 'loose'"));
        assert!(shim.contains(".render(id, source)"));
        assert!(shim.contains("output.svg"));
        assert!(shim.contains("'rendered'"));
    }

    #[test]
    fn shim_marks_blocks_for_prism_skip_before_loading_mermaid() {
        let shim = mermaid_shim_js();
        let mark_pos = shim
            .find("'mermaid-block', 'no-line-numbers'")
            .expect("shim should mark mermaid pre blocks");
        let script_pos = shim
            .find("document.createElement('script')")
            .expect("shim should load mermaid asset");
        assert!(mark_pos < script_pos);
    }
}
