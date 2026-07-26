//! Guards the bundled stylesheet against hardcoded colours.
//!
//! Dark mode works by re-defining the `:root` palette under
//! `html[data-theme="dark"]`. A rule that writes a literal colour instead of a
//! `var(--token)` opts out of that mechanism: it keeps its light-mode value
//! after the theme flips, which is how `#doc-content h3` ended up at 1.4:1 and
//! `blockquote` at 1.3:1 against the dark background.
//!
//! This test pins the set of declarations that still hardcode a colour. Adding a
//! new one fails the build with instructions; the fix is normally a token, and
//! the escape hatch is documented in the failure message.

const TEMPLATE: &str = include_str!("../discuss.html");

/// Properties where a hardcoded colour causes a theme bug. `box-shadow` is
/// deliberately absent: its colours are almost always `rgba(0, 0, 0, α)`, which
/// reads acceptably on either background, so including it would bury real
/// findings under a dozen benign entries.
const THEMED_PROPERTIES: &[&str] = &[
    "color",
    "background",
    "background-color",
    "border",
    "border-color",
    "border-top",
    "border-right",
    "border-bottom",
    "border-left",
    "border-top-color",
    "border-right-color",
    "border-bottom-color",
    "border-left-color",
    "outline",
    "outline-color",
];

/// Named colours the stylesheet actually uses. Extend if a new one appears.
const NAMED_COLOURS: &[&str] = &["white", "black"];

/// Declarations that hardcode a colour and are *correct* doing so, because the
/// colour works on both backgrounds or the rule has a dark-scoped counterpart.
///
/// Broadly: white/light text on a saturated fill (accent, green, red, grey),
/// the thread markers, and the handful of composite surfaces that carry their
/// own `html[data-theme="dark"]` override (`.done-banner`, `#thread-tooltip`,
/// `.verdict-modal`, `.heading-minimap::before`, `.hm-bar`).
const ALLOWED: &[&str] = &[
    "#doc-content [data-anchor-idx].has-thread | background:rgba(231, 178, 122, 0.07)",
    "#finish-review | color:white",
    "#finish-review.ok | background:#2e7d32",
    "#finish-review.ok | border-color:#2e7d32",
    "#selection-popup:hover | color:white",
    "#thread-tooltip .tt-kind | background:rgba(255,255,255,0.18)",
    "#thread-tooltip .tt-quote | color:#fcdca9",
    "#thread-tooltip | color:white",
    ".done-banner .verdict-feedback | color:#245f31",
    ".done-banner | background:#e9f7ec",
    ".done-banner | border-bottom:1px solid #b8dfc0",
    ".done-banner | color:#1f6b2f",
    ".file-item .file-count | color:#fff",
    ".file-sidebar .file-sidebar-title | color:var(--muted, #888)",
    ".followup button.primary | color:white",
    ".heading-minimap::before | background:rgba(255, 255, 255, 0.32)",
    ".hm-bar | background:rgba(0,0,0,0.22)",
    ".new-thread-editor button.primary | color:white",
    ".thread-marker | border:2px solid white",
    ".thread-marker | color:white",
    ".thread-marker.has-draft::before | background:#0d9488",
    ".thread-marker.has-draft::before | border:1.5px solid white",
    ".thread-marker.is-resolved | background:#2e7d32 !important",
    ".thread-marker.is-resolved | color:white !important",
    ".thread-marker.kind-draft | background:#0d9488",
    ".thread-marker.kind-draft | border-color:white",
    ".thread-marker.kind-draft | color:white",
    ".thread-marker.kind-pending | color:#5a4500",
    ".verdict-modal | background:rgba(255, 255, 255, 0.32)",
    ".verdict-option-button.negative, .verdict-submit.negative | background:#c62828",
    ".verdict-option-button.negative, .verdict-submit.negative | border-color:#c62828",
    ".verdict-option-button.negative, .verdict-submit.negative | color:white",
    ".verdict-option-button.neutral, .verdict-submit.neutral | background:#666",
    ".verdict-option-button.neutral, .verdict-submit.neutral | border-color:#666",
    ".verdict-option-button.neutral, .verdict-submit.neutral | color:white",
    ".verdict-option-button.positive, .verdict-submit.positive | background:#2e7d32",
    ".verdict-option-button.positive, .verdict-submit.positive | border-color:#2e7d32",
    ".verdict-option-button.positive, .verdict-submit.positive | color:white",
    "header .toggle.on | color:white",
];

fn style_block(html: &str) -> &str {
    let open = html
        .find("<style>")
        .expect("template must have a <style> block")
        + "<style>".len();
    let close = html[open..]
        .find("</style>")
        .expect("template <style> must close")
        + open;
    &html[open..close]
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start..].find("*/") {
            Some(end) => rest = &rest[start + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn normalise(selector: &str) -> String {
    selector.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Walks the stylesheet and yields `(selector, declaration)` for every
/// declaration, tracking a selector stack so nested blocks (`@keyframes`) get
/// attributed to their innermost selector rather than the at-rule.
fn declarations(css: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut buf = String::new();

    let flush = |buf: &mut String, stack: &[String], found: &mut Vec<(String, String)>| {
        let decl = buf.trim();
        if !decl.is_empty() {
            let selector = stack.last().cloned().unwrap_or_default();
            found.push((selector, decl.to_string()));
        }
        buf.clear();
    };

    for ch in css.chars() {
        match ch {
            '{' => {
                stack.push(normalise(&buf));
                buf.clear();
            }
            '}' => {
                flush(&mut buf, &stack, &mut found);
                stack.pop();
            }
            ';' if !stack.is_empty() => flush(&mut buf, &stack, &mut found),
            _ => buf.push(ch),
        }
    }
    found
}

/// True for the two palette blocks, where literal colours are the whole point.
fn is_palette(selector: &str) -> bool {
    selector == ":root" || selector == r#"html[data-theme="dark"]"#
}

/// True for any rule explicitly scoped to the dark theme — such a rule cannot
/// leak a light-mode colour into dark mode, so literals in it are fine.
fn is_dark_scoped(selector: &str) -> bool {
    selector.contains(r#"data-theme="dark""#)
}

fn has_literal_colour(value: &str) -> bool {
    // Hex: '#' followed by a hex digit.
    if value
        .as_bytes()
        .windows(2)
        .any(|w| w[0] == b'#' && (w[1] as char).is_ascii_hexdigit())
    {
        return true;
    }
    if value.contains("rgb") {
        return true;
    }
    NAMED_COLOURS.iter().any(|name| {
        value
            .split(|c: char| !c.is_ascii_alphabetic())
            .any(|word| word.eq_ignore_ascii_case(name))
    })
}

fn hardcoded_colour_declarations() -> Vec<String> {
    let css = strip_comments(style_block(TEMPLATE));
    let mut found: Vec<String> = declarations(&css)
        .into_iter()
        .filter(|(selector, _)| !is_palette(selector) && !is_dark_scoped(selector))
        .filter_map(|(selector, decl)| {
            let (property, value) = decl.split_once(':')?;
            let property = property.trim().to_ascii_lowercase();
            if property.starts_with("--") || !THEMED_PROPERTIES.contains(&property.as_str()) {
                return None;
            }
            if !has_literal_colour(value) {
                return None;
            }
            Some(format!("{selector} | {property}:{}", value.trim()))
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

#[test]
fn stylesheet_does_not_hardcode_new_colours() {
    let found = hardcoded_colour_declarations();

    let unexpected: Vec<&String> = found
        .iter()
        .filter(|entry| !ALLOWED.contains(&entry.as_str()))
        .collect();

    assert!(
        unexpected.is_empty(),
        "discuss.html declares {} colour(s) that bypass the theme palette:\n\n{}\n\n\
         A literal colour here keeps its light-mode value after the theme flips, \
         which is how h3 and blockquote ended up unreadable in dark mode.\n\n\
         Fix it by using a palette token (see :root and html[data-theme=\"dark\"]), \
         adding a new token to BOTH palettes if none fits.\n\n\
         If the colour really is correct on both backgrounds -- white text on a \
         saturated fill, or a rule that has its own html[data-theme=\"dark\"] \
         override -- add the exact line above to ALLOWED in tests/theme.rs.",
        unexpected.len(),
        unexpected
            .iter()
            .map(|entry| format!("  {entry}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[test]
fn allowlist_has_no_stale_entries() {
    let found = hardcoded_colour_declarations();

    let stale: Vec<&&str> = ALLOWED
        .iter()
        .filter(|allowed| !found.iter().any(|entry| entry == *allowed))
        .collect();

    assert!(
        stale.is_empty(),
        "ALLOWED in tests/theme.rs lists {} entr(y/ies) no longer in discuss.html:\n\n{}\n\n\
         These were tokenised or removed -- delete them from ALLOWED so the list \
         keeps reflecting the stylesheet.",
        stale.len(),
        stale
            .iter()
            .map(|entry| format!("  {entry}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[test]
fn both_palettes_define_the_same_tokens() {
    let css = strip_comments(style_block(TEMPLATE));

    let tokens = |wanted: &str| -> Vec<String> {
        let mut names: Vec<String> = declarations(&css)
            .into_iter()
            .filter(|(selector, _)| selector == wanted)
            .filter_map(|(_, decl)| {
                let (property, _) = decl.split_once(':')?;
                let property = property.trim();
                property.starts_with("--").then(|| property.to_string())
            })
            .collect();
        names.sort();
        names.dedup();
        names
    };

    let light = tokens(":root");
    let dark = tokens(r#"html[data-theme="dark"]"#);
    assert!(!light.is_empty(), "light palette should define tokens");

    // Layout tokens (sizes, the font stack) live only in :root by design; the
    // dark palette overrides colours. So require dark ⊆ light, and that every
    // dark token is a real light token rather than a typo'd new name.
    let unknown: Vec<&String> = dark.iter().filter(|name| !light.contains(name)).collect();
    assert!(
        unknown.is_empty(),
        "html[data-theme=\"dark\"] defines token(s) that :root does not:\n\n{}\n\n\
         A token only in the dark palette is unreachable in light mode -- most \
         likely a typo, or a token that needs a light value too.",
        unknown
            .iter()
            .map(|name| format!("  {name}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}
