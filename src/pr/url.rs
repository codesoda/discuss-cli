use std::fmt;
use std::str::FromStr;

use url::Url;

use crate::{DiscussError, Result};

/// A strictly validated public GitHub pull-request URL.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GithubPrUrl {
    canonical_url: String,
    owner: String,
    repo: String,
    number: u64,
}

impl GithubPrUrl {
    /// Parses a full `https://github.com/owner/repo/pull/number` URL.
    pub fn parse(input: &str) -> Result<Self> {
        input.parse()
    }

    /// Returns the canonical URL, with the pull-request number normalized.
    pub fn canonical_url(&self) -> &str {
        &self.canonical_url
    }

    /// Returns the repository owner.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Returns the repository name.
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// Returns the positive pull-request number.
    pub fn number(&self) -> u64 {
        self.number
    }
}

impl FromStr for GithubPrUrl {
    type Err = DiscussError;

    fn from_str(input: &str) -> Result<Self> {
        const PREFIX: &str = "https://github.com/";
        if !input.starts_with(PREFIX) {
            return Err(invalid_url(
                input,
                "the scheme and host must be exactly https://github.com",
            ));
        }
        if input.contains('%') {
            return Err(invalid_url(
                input,
                "percent-encoded path material is not allowed",
            ));
        }
        if input.contains(['?', '#']) {
            return Err(invalid_url(
                input,
                "query strings and fragments are not allowed",
            ));
        }
        if input.contains('\\') {
            return Err(invalid_url(input, "backslashes are not allowed"));
        }

        let parsed = Url::parse(input).map_err(|error| {
            invalid_url(input, &format!("the URL could not be parsed: {error}"))
        })?;
        if parsed.scheme() != "https"
            || parsed.host_str() != Some("github.com")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.port().is_some()
        {
            return Err(invalid_url(
                input,
                "credentials, explicit ports, and non-GitHub origins are not allowed",
            ));
        }

        // Checking the raw authority catches an explicit default port, which
        // `url::Url` intentionally normalizes away.
        let authority = input[PREFIX.len() - "github.com/".len()..]
            .split('/')
            .next()
            .unwrap_or_default();
        if authority != "github.com" {
            return Err(invalid_url(
                input,
                "the host must not include credentials or a port",
            ));
        }

        let segments: Vec<_> = parsed
            .path_segments()
            .ok_or_else(|| invalid_url(input, "the pull-request path is missing"))?
            .collect();
        if segments.len() != 4 || segments[2] != "pull" {
            return Err(invalid_url(
                input,
                "expected exactly /{owner}/{repo}/pull/{positive-number} with no trailing slash",
            ));
        }
        let [owner, repo, _, number_text] = segments.as_slice() else {
            unreachable!("segment count was checked")
        };
        if !valid_owner(owner) {
            return Err(invalid_url(
                input,
                "owner must be 1-39 ASCII letters, digits, or hyphens and cannot start or end with a hyphen",
            ));
        }
        if !valid_repo(repo) {
            return Err(invalid_url(
                input,
                "repository must contain only ASCII letters, digits, '.', '_', or '-'",
            ));
        }
        if number_text.is_empty() || !number_text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid_url(
                input,
                "pull-request number must be positive decimal digits",
            ));
        }
        let number = number_text
            .parse::<u64>()
            .map_err(|_| invalid_url(input, "pull-request number is too large"))?;
        if number == 0 {
            return Err(invalid_url(
                input,
                "pull-request number must be greater than zero",
            ));
        }

        let canonical_url = format!("https://github.com/{owner}/{repo}/pull/{number}");
        Ok(Self {
            canonical_url,
            owner: (*owner).to_string(),
            repo: (*repo).to_string(),
            number,
        })
    }
}

impl fmt::Display for GithubPrUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.canonical_url)
    }
}

fn valid_owner(owner: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= 39
        && !owner.starts_with('-')
        && !owner.ends_with('-')
        && owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_repo(repo: &str) -> bool {
    !repo.is_empty()
        && repo != "."
        && repo != ".."
        && repo
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn invalid_url(input: &str, reason: &str) -> DiscussError {
    DiscussError::ConfigError {
        message: format!(
            "invalid GitHub pull-request URL {input:?}: {reason}; expected https://github.com/OWNER/REPO/pull/NUMBER"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_canonicalizes_strict_pull_request_url() {
        let url = GithubPrUrl::parse("https://github.com/codesoda/discuss-cli/pull/0051")
            .expect("valid PR URL");
        assert_eq!(url.owner(), "codesoda");
        assert_eq!(url.repo(), "discuss-cli");
        assert_eq!(url.number(), 51);
        assert_eq!(
            url.canonical_url(),
            "https://github.com/codesoda/discuss-cli/pull/51"
        );
        assert_eq!(url.to_string(), url.canonical_url());
    }

    #[test]
    fn rejects_noncanonical_or_unsafe_urls() {
        for input in [
            "http://github.com/a/b/pull/1",
            "https://gitlab.com/a/b/pull/1",
            "https://user@github.com/a/b/pull/1",
            "https://github.com:443/a/b/pull/1",
            "https://github.com/a/b/pull/1?x=1",
            "https://github.com/a/b/pull/1#files",
            "https://github.com/a%2fb/c/pull/1",
            "https://github.com/a/b/pull/1/",
            "https://github.com/a/b/pull/1/files",
            "https://github.com/a/b/pull/0",
            "https://github.com/a/b/pull/-1",
            "https://github.com/-a/b/pull/1",
            "https://github.com/a-/b/pull/1",
            "https://github.com/a/b@c/pull/1",
            "HTTPS://github.com/a/b/pull/1",
        ] {
            let error = GithubPrUrl::parse(input).expect_err(input);
            assert!(matches!(error, DiscussError::ConfigError { .. }), "{input}");
            assert!(error.to_string().contains("expected https://github.com"));
        }
    }
}
