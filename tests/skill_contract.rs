//! Guards the dynamic-port contract in bundled agent integrations.

use std::fs;
use std::path::{Path, PathBuf};

fn bundled_integration_paths() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let skill_dir = root.join("skills/discuss");
    let manifest_path = skill_dir.join("manifest.txt");
    let manifest = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", manifest_path.display()));

    manifest
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|entry| {
            let relative = Path::new(entry);
            assert!(
                !relative.is_absolute()
                    && !relative
                        .components()
                        .any(|component| matches!(component, std::path::Component::ParentDir)),
                "invalid bundled integration path in manifest: {entry:?}"
            );
            let path = skill_dir.join(relative);
            assert!(
                path.is_file(),
                "manifest entry is not a file: {}",
                path.display()
            );
            path
        })
        .collect()
}

fn allows_compatibility_port(line: &str) -> bool {
    line.contains("--port 7777") && (line.contains("explicit") || line.contains("Explicit"))
}

#[test]
fn bundled_integrations_consume_reported_endpoint_map() {
    let paths = bundled_integration_paths();
    assert!(
        !paths.is_empty(),
        "integration manifest should not be empty"
    );

    let mut combined = String::new();
    let mut violations = Vec::new();
    let mapped_paths = ["/api/state", "/api/events", "/api/threads", "/api/done"];

    for path in paths {
        let content = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        combined.push_str(&content);
        combined.push('\n');

        for (index, line) in content.lines().enumerate() {
            let location = format!("{}:{}", path.display(), index + 1);

            if line.contains("127.0.0.1:7777") {
                violations.push(format!("{location}: hardcoded default endpoint"));
            }
            if line.contains("/api/state.files") {
                violations.push(format!(
                    "{location}: refers to state through a reconstructed path instead of endpoints.state"
                ));
            }
            if line.contains("7777") && line.contains("7782") {
                violations.push(format!("{location}: stale port-range probing"));
            }
            if line.contains("7777") && !allows_compatibility_port(line) {
                violations.push(format!(
                    "{location}: 7777 is allowed only in explicit --port compatibility guidance"
                ));
            }

            for variable in ["$URL", "${URL}", "$API_BASE", "${API_BASE}"] {
                for mapped_path in mapped_paths {
                    if line.contains(&format!("{variable}{mapped_path}")) {
                        violations.push(format!(
                            "{location}: reconstructs mapped endpoint {variable}{mapped_path}"
                        ));
                    }
                }
            }

            let lower = line.to_ascii_lowercase();
            if lower.contains("pick a free port")
                || lower.contains("scan for a free port")
                || lower.contains("checking which port")
            {
                violations.push(format!("{location}: stale scan-before-bind guidance"));
            }
        }
    }

    for required in [
        "payload.endpoints",
        "endpoints.state",
        "endpoints.events",
        "endpoints.createThread",
        "endpoints.addTakeTemplate",
        "endpoints.done",
        "apiBaseUrl",
        "proxyUrl",
        "OS",
    ] {
        assert!(
            combined.contains(required),
            "bundled integration guidance should mention {required:?}"
        );
    }
    assert!(
        combined.contains("poller.sh \"$STATE_URL\""),
        "poller should receive the exact reported state endpoint"
    );
    assert!(
        combined.contains("curl -s -o \"$TMPFILE\" -w \"%{http_code}\" \"$STATE_URL\""),
        "poller should curl the exact state endpoint without appending a path"
    );

    assert!(
        violations.is_empty(),
        "bundled integrations violate the endpoint contract:\n{}",
        violations.join("\n")
    );
}
