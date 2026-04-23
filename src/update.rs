use std::cmp::Ordering;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, LOCATION};
use reqwest::redirect::Policy;
use semver::Version;

use crate::{DiscussError, Result};

const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(test)]
const UPDATE_CHECK_REFERENCE: &str = concat!("update", "::check");

pub fn check() -> Result<String> {
    let current = parse_version(env!("CARGO_PKG_VERSION"), "the current package version")?;
    let latest_url = latest_release_url();
    let client = Client::builder()
        .connect_timeout(UPDATE_CHECK_TIMEOUT)
        .redirect(Policy::none())
        .build()
        .map_err(|source| update_error(format!("could not build the HTTP client: {source}")))?;
    let response = client.get(&latest_url).send().map_err(|source| {
        update_error(format!(
            "could not reach {latest_url} within {} seconds: {source}",
            UPDATE_CHECK_TIMEOUT.as_secs()
        ))
    })?;
    if response.status().is_server_error() {
        return Err(update_error(format!(
            "GitHub returned HTTP {} for {latest_url}",
            response.status().as_u16()
        )));
    }
    let latest = latest_version_from_response(response.headers(), &latest_url)?;

    Ok(status_line(&current, &latest))
}

fn latest_release_url() -> String {
    format!("{}/releases/latest", env!("CARGO_PKG_REPOSITORY"))
}

fn latest_version_from_response(headers: &HeaderMap, latest_url: &str) -> Result<Version> {
    let location = headers.get(LOCATION).ok_or_else(|| {
        update_error(format!(
            "GitHub did not return a Location header for {latest_url}"
        ))
    })?;
    let location = location.to_str().map_err(|_| {
        update_error(format!(
            "GitHub returned a non-UTF-8 Location header for {latest_url}"
        ))
    })?;

    parse_latest_version_from_location(location)
}

fn parse_latest_version_from_location(location: &str) -> Result<Version> {
    let raw_tag = location
        .trim_end_matches('/')
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .ok_or_else(|| {
            update_error(format!(
                "could not determine the latest release tag from redirect location {location:?}"
            ))
        })?;
    let version = raw_tag.strip_prefix('v').unwrap_or(raw_tag);

    parse_version(
        version,
        &format!("the latest release tag in redirect location {location:?}"),
    )
}

fn parse_version(raw: &str, context: &str) -> Result<Version> {
    Version::parse(raw).map_err(|source| {
        update_error(format!(
            "could not parse {context} ({raw}) as a semantic version: {source}"
        ))
    })
}

fn status_line(current: &Version, latest: &Version) -> String {
    let summary = match compare_versions(current, latest) {
        Ordering::Less => "a newer version is available — run `discuss update -y`",
        Ordering::Equal => "you're up to date",
        Ordering::Greater => "this build is newer than the latest published release",
    };

    format!("current: {current}  latest: {latest}  ({summary})")
}

fn update_error(message: String) -> DiscussError {
    DiscussError::UpdateCheckError { message }
}

fn compare_versions(current: &Version, latest: &Version) -> Ordering {
    current.cmp(latest)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::{Path, PathBuf};

    use reqwest::header::HeaderValue;

    #[test]
    fn parses_latest_tag_from_relative_redirect_location() {
        let version = parse_latest_version_from_location("/owner/repo/releases/tag/v1.2.3")
            .expect("relative GitHub release redirect should parse");

        assert_eq!(version, Version::parse("1.2.3").expect("valid version"));
    }

    #[test]
    fn compares_upgrade_downgrade_and_equal_versions() {
        let current = Version::parse("0.1.0").expect("valid version");

        assert_eq!(
            compare_versions(&current, &Version::parse("0.2.0").expect("valid version")),
            Ordering::Less
        );
        assert_eq!(
            compare_versions(&current, &Version::parse("0.1.0").expect("valid version")),
            Ordering::Equal
        );
        assert_eq!(
            compare_versions(&current, &Version::parse("0.0.9").expect("valid version")),
            Ordering::Greater
        );
    }

    #[test]
    fn missing_location_header_returns_actionable_error() {
        let error = latest_version_from_response(&HeaderMap::new(), &latest_release_url())
            .expect_err("missing Location header should fail");

        let message = error.to_string();
        assert!(message.contains("update check failed"));
        assert!(message.contains("Location header"));
        assert!(message.contains("discuss update --check"));
    }

    #[test]
    fn update_check_is_only_referenced_from_the_update_subcommand() {
        let src_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let references = update_check_references(&src_dir);

        assert_eq!(references, vec![src_dir.join("lib.rs")]);
    }

    #[test]
    fn parses_latest_tag_from_location_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            LOCATION,
            HeaderValue::from_static("/owner/repo/releases/tag/v1.2.3"),
        );

        let version = latest_version_from_response(&headers, &latest_release_url())
            .expect("Location header should parse");

        assert_eq!(version, Version::parse("1.2.3").expect("valid version"));
    }

    fn update_check_references(path: &Path) -> Vec<PathBuf> {
        let mut references = Vec::new();
        collect_update_check_references(path, &mut references);
        references
    }

    fn collect_update_check_references(path: &Path, references: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(path).expect("read src dir");

        for entry in entries {
            let entry = entry.expect("read dir entry");
            let path = entry.path();

            if path.is_dir() {
                collect_update_check_references(&path, references);
                continue;
            }

            if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
                continue;
            }

            let source = fs::read_to_string(&path).expect("read source file");
            if source.contains(UPDATE_CHECK_REFERENCE) {
                references.push(path);
            }
        }

        references.sort();
    }
}
