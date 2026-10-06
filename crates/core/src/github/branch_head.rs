//! The commit a branch points at on GitHub, in one request.
//!
//! `GET /repos/{owner}/{repo}/commits/heads/{branch}` with the
//! `application/vnd.github.sha` media type answers with the 40-character SHA
//! as the whole body — no JSON, no commit payload. It is asked on a timer for
//! every remote-backed workspace, so the cheapest form of the question is the
//! one worth having.

use std::time::Duration;

use reqwest::StatusCode;

use super::client::GitHubClient;
use super::tarball::{is_rate_limited, transport_error};

/// One small request; anything slower than this is GitHub being unwell.
const BRANCH_HEAD_TIMEOUT: Duration = Duration::from_secs(15);

/// Why GitHub did not say where the branch is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BranchHeadError {
    /// 404 or 422: no such branch, no such repository, or a repository this
    /// token cannot see — GitHub does not distinguish them.
    #[error("GitHub has no branch {branch} in {owner}/{repo}, or this token cannot see it")]
    NotFound {
        owner: String,
        repo: String,
        branch: String,
    },
    /// 401, or a 403 that is not a rate limit: the token itself was refused.
    #[error("GitHub refused the token for {owner}/{repo} (HTTP {status})")]
    Denied {
        owner: String,
        repo: String,
        status: u16,
    },
    /// 429, or a 403 carrying a rate-limit header. Separate from
    /// [`Self::Unavailable`] because the right response differs: stop asking
    /// for a while, for every repository, not just this one.
    #[error("GitHub rate-limited the request (HTTP {status})")]
    RateLimited { status: u16 },
    /// A 5xx, a request that never completed, or a body that is not a SHA.
    #[error("GitHub is unavailable: {0}")]
    Unavailable(String),
}

impl GitHubClient {
    /// The SHA `branch` points at in `owner/repo`, lowercased.
    pub async fn branch_head(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<String, BranchHeadError> {
        let url = format!(
            "{}/repos/{owner}/{repo}/commits/heads/{}",
            self.base_url,
            encode_ref_path(branch)
        );
        let response = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.github.sha")
            .timeout(BRANCH_HEAD_TIMEOUT)
            .send()
            .await
            .map_err(|e| BranchHeadError::Unavailable(transport_error(e)))?;

        let status = response.status();
        if status.is_success() {
            let body = response
                .text()
                .await
                .map_err(|e| BranchHeadError::Unavailable(transport_error(e)))?;
            return parse_sha(&body);
        }
        if is_rate_limited(&response) {
            return Err(BranchHeadError::RateLimited {
                status: status.as_u16(),
            });
        }
        Err(match status {
            StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => BranchHeadError::NotFound {
                owner: owner.to_string(),
                repo: repo.to_string(),
                branch: branch.to_string(),
            },
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => BranchHeadError::Denied {
                owner: owner.to_string(),
                repo: repo.to_string(),
                status: status.as_u16(),
            },
            _ => BranchHeadError::Unavailable(format!("HTTP {status} from {url}")),
        })
    }
}

/// A branch name as URL path segments. Git allows `#`, `?`, `%` and spaces in
/// a ref name, any of which would end or corrupt the path if written as is —
/// and a branch that always builds a wrong URL is a 404 on every check, for
/// good. Each segment is percent-encoded; the `/` between them is kept.
fn encode_ref_path(branch: &str) -> String {
    branch
        .split('/')
        .map(|segment| urlencoding::encode(segment).into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// A success body that is not a commit id is not an answer: a proxy's HTML
/// page compared against the promoted SHA would read as "the branch moved".
fn parse_sha(body: &str) -> Result<String, BranchHeadError> {
    let sha = body.trim();
    if sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(sha.to_ascii_lowercase());
    }
    let shown: String = sha.chars().take(60).collect();
    Err(BranchHeadError::Unavailable(format!(
        "expected a commit SHA, got {shown:?}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_name_is_encoded_one_segment_at_a_time() {
        assert_eq!(encode_ref_path("main"), "main");
        assert_eq!(encode_ref_path("release/2026.10"), "release/2026.10");
        assert_eq!(encode_ref_path("fix/#12 50%?"), "fix/%2312%2050%25%3F");
        assert_eq!(encode_ref_path("naïve"), "na%C3%AFve");
    }

    #[test]
    fn a_sha_body_is_the_answer_and_anything_else_is_not() {
        let sha = "0123456789ABCDEF0123456789abcdef01234567";
        assert_eq!(
            parse_sha(&format!("{sha}\n")).unwrap(),
            sha.to_ascii_lowercase()
        );
        for not_a_sha in ["", "main", "<html>Bad Gateway</html>", &sha[..39]] {
            assert!(
                matches!(parse_sha(not_a_sha), Err(BranchHeadError::Unavailable(_))),
                "{not_a_sha:?}"
            );
        }
    }
}
