//! Guards bundled skills and integrations against hardcoded endpoint construction.
//!
//! Agents must consume endpoints from the session.started payload instead of
//! scanning ports or reconstructing URLs from a hardcoded port. This test fails
//! if bundled skill files still instruct agents to scan or rebuild — a pattern
//! that is race-prone in concurrent-session scenarios.
//!
//! Documentation may mention "7777" only when explaining explicit-port
//! compatibility (e.g., "`--port 7777` for predictability").

use std::fs;
use std::path::PathBuf;

/// Files to scan for endpoint construction violations.
fn skill_files() -> Vec<PathBuf> {
    vec![
        PathBuf::from("skills/discuss/SKILL.md"),
        PathBuf::from("skills/discuss/poller.sh"),
    ]
}

/// True if the line represents a violation: hardcoded-port endpoint construction
/// or port-scanning guidance in the bundled skill files. This function needs context
/// to be called with surrounding lines so it can detect explicit-port contexts.
/// See `should_flag_as_violation` which wraps this with context awareness.
///
/// Patterns that trigger violations:
/// 1. "Pick a free port by checking which of 7777" — explicit port-scanning guidance
/// 2. Instructing agents to scan a port range and build URLs from it
/// 3. Hardcoded port in bash without being part of an explicit-port example block
///
/// Patterns that DON'T trigger (allowed):
/// - Explaining `--port 7777` as a compatibility option
/// - JSON examples with `<port>` placeholder
/// - References to `$URL`, `$ENDPOINT_URL`, etc.
fn has_hardcoded_endpoint_construction_raw(line: &str) -> bool {
    let line_lower = line.to_lowercase();

    // **VIOLATION 1:** Explicit port-scanning guidance
    // "Pick a free port by checking which of 7777–7782 isn't already bound"
    if (line_lower.contains("pick a free port") || line_lower.contains("checking which"))
        && (line_lower.contains("7777") || line_lower.contains("7782"))
    {
        return true;
    }

    // **VIOLATION 2:** Direct hardcoded port reference in bash ("http://127.0.0.1:7777")
    // Look for a literal hardcoded address like `http://127.0.0.1:7777/api/`
    // But allow `http://127.0.0.1:<port>/...` (with angle brackets for placeholder)
    if line.contains("http://127.0.0.1:") && !line.contains("<port>") && !line.contains("$") {
        // This looks like a hardcoded port example (e.g., http://127.0.0.1:7777)
        // Check if it's followed by a digit without angle brackets
        if let Some(pos) = line.find("http://127.0.0.1:") {
            let after = &line[pos + "http://127.0.0.1:".len()..];
            let next_char = after.chars().next();
            if next_char.is_some_and(|c| c.is_ascii_digit()) {
                return true;
            }
        }
    }

    false
}

/// Context-aware wrapper that checks if a line should be flagged as a violation.
/// Looks at preceding lines to see if this is in an explicit-port context.
fn should_flag_as_violation(line: &str, line_num: usize, all_lines: &[&str]) -> bool {
    if !has_hardcoded_endpoint_construction_raw(line) {
        return false;
    }

    // Allow if in an explicit-port context: check preceding lines for "--port" mention
    let start = line_num.saturating_sub(20);
    for prev_line in all_lines.iter().take(line_num).skip(start) {
        if prev_line.contains("--port") {
            // We're in the section that explains explicit --port usage
            return false;
        }
    }

    true
}

#[test]
fn bundled_skills_do_not_hardcode_endpoint_construction() {
    let skill_paths = skill_files();
    let mut violations = Vec::new();

    for path in skill_paths {
        if !path.exists() {
            panic!("Skill file missing: {}", path.display());
        }

        let content = fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("Failed to read {}", path.display()));

        let lines: Vec<&str> = content.lines().collect();

        for (line_num, line) in lines.iter().enumerate() {
            if should_flag_as_violation(line, line_num, &lines) {
                violations.push(format!(
                    "{}:{}: {}",
                    path.display(),
                    line_num + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Bundled skill files construct endpoints from hardcoded ports or port scanning. \
         Agents must consume endpoints from session.started.payload instead:\n\n{}\n\n\
         Fix by:\n\
         1. Remove port-scanning guidance (e.g., 'scan which of 7777-7782')\n\
         2. Remove hand-built URL construction (e.g., `\"$URL/api/state\"`)\n\
         3. Replace with 'parse endpoints from session.started.payload'\n\
         4. Substitute {{threadId}} into addTakeTemplate for each request\n\
         \n\
         Exception: documentation may mention '7777' when explaining \
         explicit `--port` compatibility.",
        violations
            .iter()
            .map(|v| format!("  {v}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn guard_detects_hardcoded_port_pattern_and_allows_explicit_port() {
    // NEGATIVE CONTROL: this test proves the guard works both ways.

    // These patterns SHOULD be flagged (violations) when in isolation:
    let violations = vec![
        "Pick a free port by checking which of 7777–7782 isn't already bound",
        "checking which of 7777–7782 isn't already bound",
    ];

    for pattern in violations {
        assert!(
            has_hardcoded_endpoint_construction_raw(pattern),
            "Guard should detect hardcoded port pattern: {}",
            pattern
        );
    }

    // These patterns SHOULD NOT be flagged (allowed):
    let allowed = vec![
        "Pass `--port 7777` for a predictable address.",
        "Use --port 7777 when you need a fixed port.",
        r#"json sample: {"url": "http://127.0.0.1:<port>/api/state"}"#,
        "Wait for session.started and parse url from payload.",
        r#"bash <skill-dir>/poller.sh "$ENDPOINT_URL""#,
        r#"curl -s "$STATE_ENDPOINT" | jq"#,
        r#"curl -s "$URL/api/state""#,
        r#"bash <skill-dir>/poller.sh "http://127.0.0.1:<port>""#,
    ];

    for pattern in allowed {
        assert!(
            !has_hardcoded_endpoint_construction_raw(pattern),
            "Guard should NOT flag endpoint-map-aware pattern: {}",
            pattern
        );
    }
}
