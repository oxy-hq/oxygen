//! The request bodies of the trust-policy routes, parsed and checked
//! (API-tokens design §3.4). Pure: nothing here reads the database or GitHub,
//! so every rule is unit-tested.
//!
//! What a body may ask for:
//!
//! - `repository` is `owner/repo`, in the characters GitHub allows. It is
//!   resolved to numeric ids when the policy is registered; a body may carry
//!   `repository_id` and `repository_owner_id` for when that cannot be done.
//! - `workflow_path` is `.github/workflows/<file>.yml`, or the full
//!   `owner/repo/.github/workflows/<file>.yml` of a reusable workflow.
//! - `environment` and `ref_pattern` are optional text; blank is none.
//! - `grants` is **always explicit**: there is no default reach for a policy.
//!   Each grant is in the account's own org, a workspace ceiling is never
//!   above the account's `org_role`, and `owner` is never valid.

use serde::Deserialize;
use uuid::Uuid;

use super::access::{GrantInput, Invalid, invalid, present};
use super::account_access::{AccountRole, account_grants};
use super::personal::GrantSpec;

pub const WORKFLOW_PATH_MAX_CHARS: usize = 255;
pub const ENVIRONMENT_MAX_CHARS: usize = 255;
pub const REF_PATTERN_MAX_CHARS: usize = 255;

const KIND_WORKSPACE: &str = "workspace";
const KIND_APP_PUBLISH: &str = "app_publish";
const WORKFLOWS_DIR: &str = ".github/workflows/";

/// GitHub's numeric ids for a repository and its owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepoIds {
    pub repository_id: i64,
    pub repository_owner_id: i64,
}

/// One thing a matching run is granted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyGrant {
    Workspace(GrantSpec),
    /// Publish this app of the account's org.
    AppPublish {
        org_id: Uuid,
        app_id: Uuid,
    },
}

/// `POST …/trust-policies`.
#[derive(Clone, Debug, Deserialize)]
pub struct CreatePolicyBody {
    pub repository: String,
    pub workflow_path: String,
    pub environment: Option<String>,
    pub ref_pattern: Option<String>,
    pub allow_self_hosted: Option<bool>,
    pub grants: Option<Vec<GrantInput>>,
    pub repository_id: Option<i64>,
    pub repository_owner_id: Option<i64>,
}

/// A create, checked for shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyWant {
    pub owner: String,
    pub repo: String,
    pub workflow_path: String,
    pub environment: Option<String>,
    pub ref_pattern: Option<String>,
    pub allow_self_hosted: bool,
    pub grants: Vec<PolicyGrant>,
    /// The ids the body carried — used only when they cannot be resolved.
    pub explicit_ids: Option<RepoIds>,
}

impl PolicyWant {
    /// `owner/repo`, as typed.
    pub fn repository(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

impl CreatePolicyBody {
    pub fn want(&self, org_id: Uuid, role: AccountRole) -> Result<PolicyWant, Invalid> {
        let (owner, repo) = parse_repository(&self.repository)?;
        Ok(PolicyWant {
            owner,
            repo,
            workflow_path: clean_workflow_path(&self.workflow_path)?,
            environment: clean_environment(self.environment.as_deref())?,
            ref_pattern: clean_ref_pattern(self.ref_pattern.as_deref())?,
            allow_self_hosted: self.allow_self_hosted.unwrap_or(false),
            grants: policy_grants(org_id, role, self.grants.as_deref().unwrap_or(&[]))?,
            explicit_ids: explicit_ids(self.repository_id, self.repository_owner_id)?,
        })
    }
}

/// `PATCH …/trust-policies/{id}`. A field left out is left as it is;
/// `environment` and `ref_pattern` sent as `null` are cleared.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PatchPolicyBody {
    pub workflow_path: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub environment: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub ref_pattern: Option<Option<String>>,
    pub allow_self_hosted: Option<bool>,
    pub grants: Option<Vec<GrantInput>>,
    pub disabled: Option<bool>,
    /// Never editable: a policy for another repository is another policy.
    pub repository: Option<String>,
    pub repository_id: Option<i64>,
    pub repository_owner_id: Option<i64>,
}

/// An edit, checked for shape.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PolicyEdit {
    pub workflow_path: Option<String>,
    pub environment: Option<Option<String>>,
    pub ref_pattern: Option<Option<String>>,
    pub allow_self_hosted: Option<bool>,
    /// `Some` replaces the whole set.
    pub grants: Option<Vec<PolicyGrant>>,
    pub disabled: Option<bool>,
}

impl PatchPolicyBody {
    pub fn edit(&self, org_id: Uuid, role: AccountRole) -> Result<PolicyEdit, Invalid> {
        let renames_repository = self.repository.is_some()
            || self.repository_id.is_some()
            || self.repository_owner_id.is_some();
        if renames_repository {
            return invalid("a trust policy's repository cannot be changed: register another");
        }
        Ok(PolicyEdit {
            workflow_path: self
                .workflow_path
                .as_deref()
                .map(clean_workflow_path)
                .transpose()?,
            environment: match &self.environment {
                None => None,
                Some(text) => Some(clean_environment(text.as_deref())?),
            },
            ref_pattern: match &self.ref_pattern {
                None => None,
                Some(text) => Some(clean_ref_pattern(text.as_deref())?),
            },
            allow_self_hosted: self.allow_self_hosted,
            grants: self
                .grants
                .as_deref()
                .map(|inputs| policy_grants(org_id, role, inputs))
                .transpose()?,
            disabled: self.disabled,
        })
    }
}

fn is_owner_name(name: &str) -> bool {
    let chars_ok = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    (1..=39).contains(&name.len()) && chars_ok && !name.starts_with('-') && !name.ends_with('-')
}

fn is_repo_name(name: &str) -> bool {
    let chars_ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (1..=100).contains(&name.len()) && chars_ok && name != "." && name != ".."
}

/// `owner/repo`, each in the characters GitHub allows. Strict on purpose: the
/// two names become path segments of a GitHub API URL, and nothing that could
/// change which URL that is may pass.
pub fn parse_repository(raw: &str) -> Result<(String, String), Invalid> {
    let bad = || invalid("'repository' must be 'owner/repo'");
    let Some((owner, repo)) = raw.trim().split_once('/') else {
        return bad();
    };
    if !is_owner_name(owner) || !is_repo_name(repo) {
        return bad();
    }
    Ok((owner.to_string(), repo.to_string()))
}

fn is_workflow_file(file: &str) -> bool {
    let named = file.len() > ".yml".len() && (file.ends_with(".yml") || file.ends_with(".yaml"));
    let chars_ok = file
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'));
    named && chars_ok && !file.contains("..") && !file.starts_with('/') && !file.contains("//")
}

/// `.github/workflows/<file>` of the policy's own repository, or the full
/// `owner/repo/.github/workflows/<file>` of a reusable workflow elsewhere.
pub fn clean_workflow_path(raw: &str) -> Result<String, Invalid> {
    let path = raw.trim();
    let bad = || {
        invalid(
            "'workflow_path' must be '.github/workflows/<file>.yml', or \
             'owner/repo/.github/workflows/<file>.yml' for a reusable workflow",
        )
    };
    if path.chars().count() > WORKFLOW_PATH_MAX_CHARS {
        return bad();
    }
    if let Some(file) = path.strip_prefix(WORKFLOWS_DIR) {
        return if is_workflow_file(file) {
            Ok(path.to_string())
        } else {
            bad()
        };
    }
    let mut parts = path.splitn(3, '/');
    let (Some(owner), Some(repo), Some(rest)) = (parts.next(), parts.next(), parts.next()) else {
        return bad();
    };
    let file_ok = rest
        .strip_prefix(WORKFLOWS_DIR)
        .is_some_and(is_workflow_file);
    if is_owner_name(owner) && is_repo_name(repo) && file_ok {
        Ok(path.to_string())
    } else {
        bad()
    }
}

fn printable(text: &str) -> bool {
    !text.chars().any(|c| c.is_control())
}

/// Blank is none.
pub fn clean_environment(raw: Option<&str>) -> Result<Option<String>, Invalid> {
    let Some(text) = raw.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    if text.chars().count() > ENVIRONMENT_MAX_CHARS || !printable(text) {
        return invalid(format!(
            "'environment' must be at most {ENVIRONMENT_MAX_CHARS} printable characters"
        ));
    }
    Ok(Some(text.to_string()))
}

/// Blank is none. A pattern is a glob on the full ref, so it starts `refs/`.
pub fn clean_ref_pattern(raw: Option<&str>) -> Result<Option<String>, Invalid> {
    let Some(text) = raw.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    let shaped = text.starts_with("refs/")
        && text.chars().count() <= REF_PATTERN_MAX_CHARS
        && !text.chars().any(|c| c.is_whitespace() || c.is_control());
    if !shaped {
        return invalid(
            "'ref_pattern' must be a full ref such as 'refs/heads/main', where '*' matches \
             any run of characters",
        );
    }
    Ok(Some(text.to_string()))
}

/// Both ids, or neither.
fn explicit_ids(repo: Option<i64>, owner: Option<i64>) -> Result<Option<RepoIds>, Invalid> {
    match (repo, owner) {
        (None, None) => Ok(None),
        (Some(repository_id), Some(repository_owner_id))
            if repository_id > 0 && repository_owner_id > 0 =>
        {
            Ok(Some(RepoIds {
                repository_id,
                repository_owner_id,
            }))
        }
        _ => invalid(
            "'repository_id' and 'repository_owner_id' are GitHub's numeric ids, given together",
        ),
    }
}

fn app_publish_grant(org_id: Uuid, input: &GrantInput) -> Result<PolicyGrant, Invalid> {
    let Some(app_id) = input.app_id else {
        return invalid("an app_publish grant names its app in 'app_id'");
    };
    if input.org_id.is_some_and(|named| named != org_id) {
        return invalid("a service account's grants are in its own organization");
    }
    if input.workspace_id.is_some() || input.role_ceiling.is_some() {
        return invalid("an app_publish grant takes neither 'workspace_id' nor 'role_ceiling'");
    }
    Ok(PolicyGrant::AppPublish { org_id, app_id })
}

/// The grants of a policy on an account of `org_id` standing at `role`.
///
/// Never empty, and never defaulted: a policy with no stated reach is refused
/// rather than given the whole org.
pub fn policy_grants(
    org_id: Uuid,
    role: AccountRole,
    inputs: &[GrantInput],
) -> Result<Vec<PolicyGrant>, Invalid> {
    if inputs.is_empty() {
        return invalid("'grants' is required: say what a matching run may reach");
    }
    let mut workspace: Vec<GrantInput> = Vec::new();
    let mut apps: Vec<PolicyGrant> = Vec::new();
    for input in inputs {
        match input.kind.as_deref().unwrap_or(KIND_WORKSPACE) {
            KIND_WORKSPACE => {
                if input.role_ceiling.is_none() {
                    return invalid(
                        "a workspace grant on a trust policy states its 'role_ceiling'",
                    );
                }
                if input.app_id.is_some() {
                    return invalid("a workspace grant takes no 'app_id'");
                }
                workspace.push(input.clone());
            }
            KIND_APP_PUBLISH => {
                let grant = app_publish_grant(org_id, input)?;
                if !apps.contains(&grant) {
                    apps.push(grant);
                }
            }
            other => {
                return invalid(format!(
                    "grant 'kind' must be workspace or app_publish, not '{other}'"
                ));
            }
        }
    }
    let mut out: Vec<PolicyGrant> = Vec::new();
    if !workspace.is_empty() {
        // The account's own rules: its org only, never `owner`, never above
        // its `org_role`.
        let specs = account_grants(org_id, role, &workspace)?;
        out.extend(specs.into_iter().map(PolicyGrant::Workspace));
    }
    out.extend(apps);
    Ok(out)
}

#[cfg(test)]
#[path = "trust_policy_access_tests.rs"]
mod tests;
