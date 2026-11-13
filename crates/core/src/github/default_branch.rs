//! A repository's default branch on GitHub, in one request:
//! `GET /repos/{owner}/{repo}`, reading `default_branch`. The periodic compile
//! check needs it recorded on the workspace row before it can ask where that
//! branch is; this is how a remote-backed workspace with none gets one.

use std::time::Duration;

use reqwest::StatusCode;

use super::branch_head::BranchHeadError;
use super::client::GitHubClient;
use super::tarball::{is_rate_limited, transport_error};

/// One small request; anything slower than this is GitHub being unwell.
const DEFAULT_BRANCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Why GitHub did not name the default branch.
///
/// Split from [`BranchHeadError::Unavailable`] because a caller asking about
/// many repositories must tell the two apart: a request that never completed
/// says GitHub is unwell, so stop asking anyone; a response that came back
/// without a default branch (an unmapped status such as 451 or a 5xx for this
/// repository, a body with no `default_branch`) is about **this** repository,
/// and asking the next one is fine.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DefaultBranchError {
    /// Not found, denied, rate-limited, or a request that never completed
    /// (`Unavailable` here means transport only).
    #[error(transparent)]
    Request(#[from] BranchHeadError),
    /// GitHub answered for this repository, but not with a default branch.
    #[error("GitHub did not name a default branch: {0}")]
    NoAnswer(String),
}

impl GitHubClient {
    /// The default branch of `owner/repo`, asked with at most `timeout` (and never
    /// more than 15 s). A not-found names the branch `"(default)"`.
    pub async fn repository_default_branch(
        &self,
        owner: &str,
        repo: &str,
        timeout: Duration,
    ) -> Result<String, DefaultBranchError> {
        let url = format!("{}/repos/{owner}/{repo}", self.base_url);
        let response = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .timeout(timeout.min(DEFAULT_BRANCH_TIMEOUT))
            .send()
            .await
            .map_err(|e| BranchHeadError::Unavailable(transport_error(e)))?;

        let status = response.status();
        if status.is_success() {
            let body = response
                .text()
                .await
                .map_err(|e| BranchHeadError::Unavailable(transport_error(e)))?;
            return parse_default_branch(&body);
        }
        if is_rate_limited(&response) {
            return Err(BranchHeadError::RateLimited {
                status: status.as_u16(),
            }
            .into());
        }
        Err(match status {
            StatusCode::NOT_FOUND => BranchHeadError::NotFound {
                owner: owner.to_string(),
                repo: repo.to_string(),
                branch: "(default)".to_string(),
            }
            .into(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => BranchHeadError::Denied {
                owner: owner.to_string(),
                repo: repo.to_string(),
                status: status.as_u16(),
            }
            .into(),
            _ => DefaultBranchError::NoAnswer(format!("HTTP {status} from {url}")),
        })
    }
}

/// `default_branch` out of a repository body. A body without one (a proxy's
/// HTML page, an empty repository's odd answer) is not an answer.
fn parse_default_branch(body: &str) -> Result<String, DefaultBranchError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| DefaultBranchError::NoAnswer(format!("not a repository body: {e}")))?;
    match value.get("default_branch").and_then(|b| b.as_str()) {
        Some(branch) if !branch.trim().is_empty() => Ok(branch.trim().to_string()),
        _ => Err(DefaultBranchError::NoAnswer(
            "the repository body names no default_branch".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_branch_is_read_from_the_repository_body() {
        let body = r#"{"full_name":"acme/repo","default_branch":"trunk"}"#;
        assert_eq!(parse_default_branch(body).unwrap(), "trunk");
        for not_an_answer in [
            "",
            "<html>Bad Gateway</html>",
            r#"{"default_branch":""}"#,
            "{}",
        ] {
            assert!(
                matches!(
                    parse_default_branch(not_an_answer),
                    Err(DefaultBranchError::NoAnswer(_))
                ),
                "{not_an_answer:?}"
            );
        }
    }
}
