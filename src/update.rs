use std::cmp::Ordering;
use std::env;
use std::fs;
use std::io::{self, Cursor, IsTerminal, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, LOCATION};
use reqwest::redirect::Policy;
use semver::Version;
use sha2::{Digest, Sha256};
use tar::Archive;
use tempfile::{NamedTempFile, TempPath};

use crate::{DiscussError, Result};

const UPDATE_TIMEOUT: Duration = Duration::from_secs(3);
const CHECKSUMS_FILE_NAME: &str = "checksums-sha256.txt";
const BINARY_NAME: &str = env!("CARGO_PKG_NAME");
#[cfg(test)]
const UPDATE_CHECK_REFERENCE: &str = concat!("update", "::check");

#[derive(Debug, Clone, PartialEq, Eq)]
struct LatestRelease {
    tag: String,
    version: Version,
}

pub fn check() -> Result<String> {
    let current = current_version()?;
    let latest = latest_release()?;

    Ok(status_line(&current, &latest.version))
}

pub fn install(yes: bool) -> Result<String> {
    let stdin_is_tty = io::stdin().is_terminal();
    if !yes && !stdin_is_tty {
        return Err(update_install_error(
            "stdin is not a TTY - rerun with `discuss update -y` to confirm the install non-interactively"
                .to_string(),
        ));
    }

    let current = current_version()?;
    let latest = latest_release()?;

    match compare_versions(&current, &latest.version) {
        Ordering::Equal | Ordering::Greater => return Ok(status_line(&current, &latest.version)),
        Ordering::Less => {}
    }

    let approved = confirm_install(yes, stdin_is_tty, &current, &latest.version, |prompt| {
        prompt_on_stderr(prompt)
    })?;
    if !approved {
        return Ok(format!(
            "current: {current}  latest: {}  (update cancelled)",
            latest.version
        ));
    }

    let target = current_target_triple()?;
    let asset_name = release_asset_name(&latest.tag, target);
    let archive_url = release_asset_url(&latest.tag, &asset_name);
    let checksums_url = checksums_url(&latest.tag);
    let client = download_client()?;
    let archive_bytes = download_bytes(&client, &archive_url)?;
    let checksums = download_text(&client, &checksums_url)?;

    verify_archive_checksum(&archive_bytes, &checksums, &asset_name)?;
    let replacement = extract_binary(&archive_bytes, &asset_name)?;
    self_replace::self_replace(&replacement).map_err(|source| {
        update_install_error(format!(
            "could not replace the running binary with {asset_name}: {source} - rerun `discuss update -y` or reinstall with `./install.sh`"
        ))
    })?;

    Ok(format!(
        "updated discuss from {current} to {}",
        latest.version
    ))
}

fn current_version() -> Result<Version> {
    parse_version(env!("CARGO_PKG_VERSION"), "the current package version")
}

fn latest_release() -> Result<LatestRelease> {
    let latest_url = latest_release_url();
    let client = redirectless_client()?;
    let response = client.get(&latest_url).send().map_err(|source| {
        update_check_error(format!(
            "could not reach {latest_url} within {} seconds: {source}",
            UPDATE_TIMEOUT.as_secs()
        ))
    })?;
    if response.status().is_server_error() {
        return Err(update_check_error(format!(
            "GitHub returned HTTP {} for {latest_url}",
            response.status().as_u16()
        )));
    }

    latest_release_from_headers(response.headers(), &latest_url)
}

fn latest_release_url() -> String {
    format!("{}/releases/latest", env!("CARGO_PKG_REPOSITORY"))
}

fn latest_release_from_headers(headers: &HeaderMap, latest_url: &str) -> Result<LatestRelease> {
    let location = headers.get(LOCATION).ok_or_else(|| {
        update_check_error(format!(
            "GitHub did not return a Location header for {latest_url}"
        ))
    })?;
    let location = location.to_str().map_err(|_| {
        update_check_error(format!(
            "GitHub returned a non-UTF-8 Location header for {latest_url}"
        ))
    })?;

    parse_latest_release_from_location(location)
}

fn parse_latest_release_from_location(location: &str) -> Result<LatestRelease> {
    let tag = location
        .trim_end_matches('/')
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .ok_or_else(|| {
            update_check_error(format!(
                "could not determine the latest release tag from redirect location {location:?}"
            ))
        })?
        .to_string();
    let version = tag.strip_prefix('v').unwrap_or(&tag).to_string();

    Ok(LatestRelease {
        tag,
        version: parse_version(
            &version,
            &format!("the latest release tag in redirect location {location:?}"),
        )?,
    })
}

fn parse_version(raw: &str, context: &str) -> Result<Version> {
    Version::parse(raw).map_err(|source| {
        update_check_error(format!(
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

fn compare_versions(current: &Version, latest: &Version) -> Ordering {
    current.cmp(latest)
}

fn redirectless_client() -> Result<Client> {
    Client::builder()
        .connect_timeout(UPDATE_TIMEOUT)
        .redirect(Policy::none())
        .build()
        .map_err(|source| update_check_error(format!("could not build the HTTP client: {source}")))
}

fn download_client() -> Result<Client> {
    Client::builder()
        .connect_timeout(UPDATE_TIMEOUT)
        .build()
        .map_err(|source| {
            update_install_error(format!("could not build the HTTP client: {source}"))
        })
}

fn current_target_triple() -> Result<&'static str> {
    target_triple_for(env::consts::OS, env::consts::ARCH)
}

fn target_triple_for(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        _ => Err(update_install_error(format!(
            "unsupported platform {os}/{arch} - supported targets are aarch64-apple-darwin, x86_64-apple-darwin, and x86_64-unknown-linux-gnu"
        ))),
    }
}

fn release_asset_name(tag: &str, target: &str) -> String {
    format!("{BINARY_NAME}-{tag}-{target}.tar.gz")
}

fn release_asset_url(tag: &str, asset_name: &str) -> String {
    format!(
        "{}/releases/download/{tag}/{asset_name}",
        env!("CARGO_PKG_REPOSITORY")
    )
}

fn checksums_url(tag: &str) -> String {
    format!(
        "{}/releases/download/{tag}/{CHECKSUMS_FILE_NAME}",
        env!("CARGO_PKG_REPOSITORY")
    )
}

fn confirm_install<F>(
    yes: bool,
    stdin_is_tty: bool,
    current: &Version,
    latest: &Version,
    mut prompt: F,
) -> Result<bool>
where
    F: FnMut(&str) -> Result<String>,
{
    if yes {
        return Ok(true);
    }
    if !stdin_is_tty {
        return Err(update_install_error(
            "stdin is not a TTY - rerun with `discuss update -y` to confirm the install non-interactively"
                .to_string(),
        ));
    }

    let response = prompt(&format!("Update from {current} to {latest}? [y/N]"))?;
    let response = response.trim();

    Ok(response.eq_ignore_ascii_case("y") || response.eq_ignore_ascii_case("yes"))
}

fn prompt_on_stderr(prompt: &str) -> Result<String> {
    let mut stderr = io::stderr().lock();
    write!(stderr, "{prompt}").map_err(|source| {
        update_install_error(format!(
            "could not write the update prompt: {source} - rerun `discuss update -y` or try again in a terminal"
        ))
    })?;
    stderr.flush().map_err(|source| {
        update_install_error(format!(
            "could not flush the update prompt: {source} - rerun `discuss update -y` or try again in a terminal"
        ))
    })?;

    let mut response = String::new();
    io::stdin().read_line(&mut response).map_err(|source| {
        update_install_error(format!(
            "could not read an interactive response: {source} - rerun `discuss update -y` or try again in a terminal"
        ))
    })?;

    Ok(response)
}

fn download_bytes(client: &Client, url: &str) -> Result<Vec<u8>> {
    let response = client.get(url).send().map_err(|source| {
        update_install_error(format!(
            "could not reach {url} within {} seconds: {source} - check your network connection and rerun `discuss update -y`",
            UPDATE_TIMEOUT.as_secs()
        ))
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(update_install_error(format!(
            "GitHub returned HTTP {} for {url} - confirm the release assets are published and rerun `discuss update --check`",
            status.as_u16()
        )));
    }

    response
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(|source| {
            update_install_error(format!(
                "could not read {url}: {source} - check the download and rerun `discuss update -y`"
            ))
        })
}

fn download_text(client: &Client, url: &str) -> Result<String> {
    let response = client.get(url).send().map_err(|source| {
        update_install_error(format!(
            "could not reach {url} within {} seconds: {source} - check your network connection and rerun `discuss update -y`",
            UPDATE_TIMEOUT.as_secs()
        ))
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(update_install_error(format!(
            "GitHub returned HTTP {} for {url} - confirm the release assets are published and rerun `discuss update --check`",
            status.as_u16()
        )));
    }

    response.text().map_err(|source| {
        update_install_error(format!(
            "could not read {url}: {source} - check the download and rerun `discuss update -y`"
        ))
    })
}

fn verify_archive_checksum(archive_bytes: &[u8], checksums: &str, asset_name: &str) -> Result<()> {
    let expected = checksum_for_asset(checksums, asset_name)?;
    let actual = format!("{:x}", Sha256::digest(archive_bytes));

    if actual.eq_ignore_ascii_case(&expected) {
        return Ok(());
    }

    Err(update_install_error(format!(
        "sha256 mismatch for {asset_name} - expected {expected}, got {actual}; rerun `discuss update --check` and try again"
    )))
}

fn checksum_for_asset(checksums: &str, asset_name: &str) -> Result<String> {
    for line in checksums.lines() {
        let mut parts = line.split_whitespace();
        let Some(checksum) = parts.next() else {
            continue;
        };
        let Some(file_name) = parts.next() else {
            continue;
        };
        if file_name.trim_start_matches('*') == asset_name {
            return Ok(checksum.to_string());
        }
    }

    Err(update_install_error(format!(
        "{CHECKSUMS_FILE_NAME} did not contain an entry for {asset_name} - rerun `discuss update --check` to confirm the published assets"
    )))
}

fn extract_binary(archive_bytes: &[u8], asset_name: &str) -> Result<TempPath> {
    let decoder = GzDecoder::new(Cursor::new(archive_bytes));
    let mut archive = Archive::new(decoder);
    let entries = archive.entries().map_err(|source| {
        update_install_error(format!(
            "could not read {asset_name}: {source} - the published tarball may be corrupt"
        ))
    })?;

    for entry in entries {
        let mut entry = entry.map_err(|source| {
            update_install_error(format!(
                "could not read {asset_name}: {source} - the published tarball may be corrupt"
            ))
        })?;
        let path = entry.path().map_err(|source| {
            update_install_error(format!(
                "could not inspect {asset_name}: {source} - the published tarball may be corrupt"
            ))
        })?;
        if !binary_entry_matches(&path) {
            continue;
        }

        let replacement = write_replacement_binary(&mut entry, asset_name)?;
        return Ok(replacement);
    }

    Err(update_install_error(format!(
        "archive {asset_name} did not contain a {BINARY_NAME} binary - rerun `discuss update --check` to confirm the published assets"
    )))
}

fn binary_entry_matches(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == BINARY_NAME)
}

fn write_replacement_binary<R>(reader: &mut R, asset_name: &str) -> Result<TempPath>
where
    R: io::Read,
{
    let mut replacement = NamedTempFile::new().map_err(|source| {
        update_install_error(format!(
            "could not create a temporary file for {asset_name}: {source} - clear disk space and rerun `discuss update -y`"
        ))
    })?;
    io::copy(reader, replacement.as_file_mut()).map_err(|source| {
        update_install_error(format!(
            "could not extract the replacement binary from {asset_name}: {source} - rerun `discuss update -y`"
        ))
    })?;
    replacement.as_file_mut().flush().map_err(|source| {
        update_install_error(format!(
            "could not flush the replacement binary for {asset_name}: {source} - rerun `discuss update -y`"
        ))
    })?;
    #[cfg(unix)]
    {
        fs::set_permissions(replacement.path(), fs::Permissions::from_mode(0o755)).map_err(
            |source| {
                update_install_error(format!(
                    "could not mark the replacement binary as executable: {source} - rerun `discuss update -y`"
                ))
            },
        )?;
    }

    Ok(replacement.into_temp_path())
}

fn update_check_error(message: String) -> DiscussError {
    DiscussError::UpdateCheckError { message }
}

fn update_install_error(message: String) -> DiscussError {
    DiscussError::UpdateError { message }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::{Path, PathBuf};

    use reqwest::header::HeaderValue;

    #[test]
    fn parses_latest_tag_from_relative_redirect_location() {
        let release = parse_latest_release_from_location("/owner/repo/releases/tag/v1.2.3")
            .expect("relative GitHub release redirect should parse");

        assert_eq!(release.tag, "v1.2.3");
        assert_eq!(
            release.version,
            Version::parse("1.2.3").expect("valid version")
        );
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
        let error = latest_release_from_headers(&HeaderMap::new(), &latest_release_url())
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

        let release = latest_release_from_headers(&headers, &latest_release_url())
            .expect("Location header should parse");

        assert_eq!(release.tag, "v1.2.3");
        assert_eq!(
            release.version,
            Version::parse("1.2.3").expect("valid version")
        );
    }

    #[test]
    fn target_triple_mapping_covers_supported_targets() {
        assert_eq!(
            target_triple_for("macos", "aarch64").expect("apple silicon target"),
            "aarch64-apple-darwin"
        );
        assert_eq!(
            target_triple_for("macos", "x86_64").expect("intel mac target"),
            "x86_64-apple-darwin"
        );
        assert_eq!(
            target_triple_for("linux", "x86_64").expect("linux target"),
            "x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn unsupported_target_returns_actionable_error() {
        let error = target_triple_for("linux", "aarch64").expect_err("unsupported target");

        assert!(
            error
                .to_string()
                .contains("unsupported platform linux/aarch64")
        );
    }

    #[test]
    fn checksum_verification_accepts_matching_archive_bytes() {
        let archive_bytes = b"archive-bytes";
        let digest = format!("{:x}", Sha256::digest(archive_bytes));
        let checksums = format!("{digest}  discuss-v1.2.3-aarch64-apple-darwin.tar.gz\n");

        verify_archive_checksum(
            archive_bytes,
            &checksums,
            "discuss-v1.2.3-aarch64-apple-darwin.tar.gz",
        )
        .expect("matching checksum should pass");
    }

    #[test]
    fn checksum_verification_rejects_mismatched_archive_bytes() {
        let error = verify_archive_checksum(
            b"archive-bytes",
            "deadbeef  discuss-v1.2.3-aarch64-apple-darwin.tar.gz\n",
            "discuss-v1.2.3-aarch64-apple-darwin.tar.gz",
        )
        .expect_err("mismatched checksum should fail");

        let message = error.to_string();
        assert!(message.contains("sha256 mismatch"));
        assert!(message.contains("deadbeef"));
    }

    #[test]
    fn yes_flag_skips_the_prompt() {
        let current = Version::parse("0.1.0").expect("valid version");
        let latest = Version::parse("0.2.0").expect("valid version");
        let mut called = false;

        let approved = confirm_install(true, false, &current, &latest, |_| {
            called = true;
            Ok(String::new())
        })
        .expect("yes flag should bypass prompt");

        assert!(approved);
        assert!(!called);
    }

    #[test]
    fn non_tty_update_requires_yes() {
        let current = Version::parse("0.1.0").expect("valid version");
        let latest = Version::parse("0.2.0").expect("valid version");

        let error = confirm_install(false, false, &current, &latest, |_| Ok(String::new()))
            .expect_err("non-tty update should require yes");

        assert!(error.to_string().contains("discuss update -y"));
    }

    #[test]
    fn tty_prompt_accepts_yes_responses() {
        let current = Version::parse("0.1.0").expect("valid version");
        let latest = Version::parse("0.2.0").expect("valid version");
        let mut prompt = String::new();

        let approved = confirm_install(false, true, &current, &latest, |question| {
            prompt = question.to_string();
            Ok("yes\n".to_string())
        })
        .expect("interactive prompt should succeed");

        assert!(approved);
        assert_eq!(prompt, "Update from 0.1.0 to 0.2.0? [y/N]");
    }

    #[test]
    fn tty_prompt_rejects_default_response() {
        let current = Version::parse("0.1.0").expect("valid version");
        let latest = Version::parse("0.2.0").expect("valid version");

        let approved = confirm_install(false, true, &current, &latest, |_| Ok("\n".to_string()))
            .expect("interactive prompt should succeed");

        assert!(!approved);
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
