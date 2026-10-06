//! Download a repository at one commit as a gzipped tarball.
//!
//! `GET /repos/{owner}/{repo}/tarball/{sha}` answers with a redirect to the
//! archive host, which the HTTP client follows. The redirect is to another
//! origin, so the client drops the `Authorization` header on the way — the
//! redirect URL carries its own short-lived credential.

use std::time::Duration;

use reqwest::{Response, StatusCode};

use super::client::GitHubClient;

/// How long one archive request may take end to end, body included.
///
/// A commit of an Oxy workspace is kilobytes and arrives in well under a
/// second; this is the ceiling for a repository carrying large data files,
/// not an expectation.
const TARBALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Why GitHub did not hand over the archive.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TarballError {
    /// 404 (or 422, which GitHub answers for a malformed ref). GitHub also
    /// answers 404 for a private repository the token cannot see, so the two
    /// cannot be told apart from here.
    #[error(
        "GitHub has no commit {sha} in {owner}/{repo}, or this token cannot see the repository"
    )]
    NotFound {
        owner: String,
        repo: String,
        sha: String,
    },
    /// 401, or a 403 that is not a rate limit: the token itself was refused.
    #[error("GitHub refused the token for {owner}/{repo} (HTTP {status})")]
    Denied {
        owner: String,
        repo: String,
        status: u16,
    },
    /// A rate limit, a 5xx, or a request that never completed. Nothing is
    /// wrong with the commit; the same request is worth sending again.
    #[error("GitHub is unavailable: {0}")]
    Unavailable(String),
}

impl GitHubClient {
    /// Start downloading `owner/repo` at `sha`. On success the response body
    /// is the archive, unread: the caller streams it under its own size limit.
    pub async fn commit_tarball(
        &self,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<Response, TarballError> {
        let url = format!("{}/repos/{owner}/{repo}/tarball/{sha}", self.base_url);
        let response = self
            .client
            .get(&url)
            .timeout(TARBALL_TIMEOUT)
            .send()
            .await
            .map_err(|e| TarballError::Unavailable(transport_error(e)))?;

        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let rate_limited = is_rate_limited(&response);
        Err(match status {
            StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => TarballError::NotFound {
                owner: owner.to_string(),
                repo: repo.to_string(),
                sha: sha.to_string(),
            },
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN if !rate_limited => {
                TarballError::Denied {
                    owner: owner.to_string(),
                    repo: repo.to_string(),
                    status: status.as_u16(),
                }
            }
            _ => TarballError::Unavailable(format!("HTTP {status} from {url}")),
        })
    }
}

/// A transport failure as text, **without the URL it failed on**. After the
/// redirect that URL is the archive host's, and for a private repository it
/// carries a short-lived download token in its query string. This text goes
/// into a task's failure message and a log line, so the token must not.
pub fn transport_error(error: reqwest::Error) -> String {
    error.without_url().to_string()
}

/// GitHub reports both of its rate limits as 403 or 429. What marks one is a
/// `retry-after` header (secondary limit) or an exhausted
/// `x-ratelimit-remaining` (primary limit).
pub(super) fn is_rate_limited(response: &Response) -> bool {
    let headers = response.headers();
    response.status() == StatusCode::TOO_MANY_REQUESTS
        || headers.contains_key("retry-after")
        || headers
            .get("x-ratelimit-remaining")
            .is_some_and(|v| v.as_bytes() == b"0")
}
