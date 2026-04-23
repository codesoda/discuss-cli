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
        assert!(mermaid_js().len() < 700 * 1024);
    }
}
