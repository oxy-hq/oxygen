//! `owner/repo` → GitHub's numeric ids, when a trust policy is registered
//! (API-tokens design §3.4).
//!
//! A policy matches on the numeric `repository_id` and `repository_owner_id`,
//! never on names — a name can be re-registered by someone else. So the ids
//! are read from GitHub once, here, rather than typed by hand:
//!
//! 1. through the org's GitHub App installation, when it has one — which also
//!    sees a private repository;
//! 2. otherwise through the public API, unauthenticated.
//!
//! **Nothing a caller sends chooses where this request goes.** The host is a
//! constant, the path is two segments that were checked against GitHub's own
//! name characters before they got here (`parse_repository`), redirects are
//! not followed, and the request times out. A repository that cannot be read
//! is simply unresolved — the caller may then supply the ids itself.

use std::sync::OnceLock;
use std::time::Duration;

use entity::git_namespaces;
use entity::prelude::GitNamespaces;
use oxy_auth::token::trust_policy_access::RepoIds;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use uuid::Uuid;

/// The one host this module talks to.
const GITHUB_API: &str = "https://api.github.com";
const TIMEOUT: Duration = Duration::from_secs(5);
const USER_AGENT: &str = "oxy-trust-policy";

/// A repository as GitHub names it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub ids: RepoIds,
    /// `owner/repo`, in GitHub's own casing.
    pub full_name: String,
}

#[derive(Deserialize)]
struct RepoBody {
    id: i64,
    full_name: String,
    owner: OwnerBody,
}

#[derive(Deserialize)]
struct OwnerBody {
    id: i64,
}

impl From<RepoBody> for Resolved {
    fn from(body: RepoBody) -> Self {
        Self {
            ids: RepoIds {
                repository_id: body.id,
                repository_owner_id: body.owner.id,
            },
            full_name: body.full_name,
        }
    }
}

/// The URL for a repository. `owner` and `repo` are already confined to
/// GitHub's name characters, so neither can add a segment, a query or a host.
fn repo_url(owner: &str, repo: &str) -> String {
    format!("{GITHUB_API}/repos/{owner}/{repo}")
}

async fn fetch(owner: &str, repo: &str, token: Option<&str>) -> Option<Resolved> {
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(USER_AGENT)
        .build()
        .ok()?;
    let mut request = client
        .get(repo_url(owner, repo))
        .header("Accept", "application/vnd.github+json");
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(e) => {
            tracing::warn!(error = %e, "trust policy: could not reach GitHub to resolve a repository");
            return None;
        }
    };
    if !response.status().is_success() {
        tracing::info!(
            status = %response.status(),
            authenticated = token.is_some(),
            "trust policy: GitHub did not resolve the repository"
        );
        return None;
    }
    response.json::<RepoBody>().await.ok().map(Into::into)
}

/// A token for each of the org's GitHub App installations. A namespace whose
/// token cannot be minted is skipped: the public API is still to try.
async fn installation_tokens<C: ConnectionTrait>(db: &C, org_id: Uuid) -> Vec<String> {
    let namespaces = match GitNamespaces::find()
        .filter(git_namespaces::Column::OrgId.eq(org_id))
        .filter(git_namespaces::Column::Provider.eq("github"))
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "trust policy: could not read the org's git namespaces");
            return Vec::new();
        }
    };
    let mut tokens = Vec::new();
    for namespace in namespaces {
        match oxy::github::github_token_for_namespace(&namespace).await {
            Ok(token) => tokens.push(token),
            Err(e) => tracing::info!(
                error = %e,
                namespace = %namespace.id,
                "trust policy: no installation token for a git namespace"
            ),
        }
    }
    tokens
}

async fn resolve_on_github<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
    owner: &str,
    repo: &str,
) -> Option<Resolved> {
    for token in installation_tokens(db, org_id).await {
        if let Some(found) = fetch(owner, repo, Some(&token)).await {
            return Some(found);
        }
    }
    fetch(owner, repo, None).await
}

type Stub = Box<dyn Fn(&str, &str) -> Option<Resolved> + Send + Sync>;
static STUB: OnceLock<Stub> = OnceLock::new();

/// Answer every lookup in this process from `stub` instead of GitHub. For
/// integration tests, which must neither depend on the network nor resolve a
/// fixture name to whatever real repository happens to hold it.
#[doc(hidden)]
pub fn stub_resolver_for_tests(
    stub: impl Fn(&str, &str) -> Option<Resolved> + Send + Sync + 'static,
) -> bool {
    STUB.set(Box::new(stub)).is_ok()
}

/// The repository's ids, or `None` when GitHub would not say.
pub(super) async fn resolve<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
    owner: &str,
    repo: &str,
) -> Option<Resolved> {
    if let Some(stub) = STUB.get() {
        return stub(owner, repo);
    }
    resolve_on_github(db, org_id, owner, repo).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_only_ever_goes_to_githubs_api() {
        let url = repo_url("acme", "app");
        assert_eq!(url, "https://api.github.com/repos/acme/app");
        let parsed = reqwest::Url::parse(&url).expect("a URL");
        assert_eq!(parsed.host_str(), Some("api.github.com"));
        assert_eq!(parsed.scheme(), "https");
    }

    #[test]
    fn no_name_the_parser_admits_can_change_the_host_or_the_path() {
        use oxy_auth::token::trust_policy_access::parse_repository;
        for hostile in [
            "acme/app/../../user",
            "acme/app?access_token=x",
            "acme/app#x",
            "evil.example/x/y",
            "acme@evil.example/app",
            "acme/app%2F..%2F..",
            "acme\\@evil.example/app",
            "../repos",
        ] {
            assert!(
                parse_repository(hostile).is_err(),
                "{hostile:?} must never reach a URL"
            );
        }
        // What it does admit lands on exactly two path segments under /repos.
        let (owner, repo) = parse_repository("Acme-Corp/my.repo_name").expect("valid");
        let parsed = reqwest::Url::parse(&repo_url(&owner, &repo)).expect("a URL");
        assert_eq!(parsed.host_str(), Some("api.github.com"));
        assert_eq!(parsed.path(), "/repos/Acme-Corp/my.repo_name");
        assert_eq!(parsed.query(), None);
    }

    #[test]
    fn githubs_answer_maps_to_both_ids() {
        let body: RepoBody = serde_json::from_value(serde_json::json!({
            "id": 987,
            "full_name": "Acme/App",
            "owner": { "id": 42, "login": "Acme" },
            "private": false,
        }))
        .expect("a repository body");
        assert_eq!(
            Resolved::from(body),
            Resolved {
                ids: RepoIds {
                    repository_id: 987,
                    repository_owner_id: 42,
                },
                full_name: "Acme/App".into(),
            }
        );
    }
}
