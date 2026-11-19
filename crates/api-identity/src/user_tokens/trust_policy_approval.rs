//! A **trust policy** approved in the browser for `oxyc init-ci` (the shape is
//! `oxy_auth::token::trust_policy_mint`).
//!
//! `init-ci` writes a workflow that publishes an app with no stored secret,
//! and the deployment honours it only through a trust policy on a service
//! account. Creating either is session-only, so `init-ci` asks for both through
//! the approval `oxyc` mints its tokens with: `/cli-auth`, then the PKCE
//! exchange or a device-code poll (`cli_login.rs`, `device_login.rs`).
//!
//! ## Written at the approval, reported at the exchange
//!
//! A token's approval only records what was approved, and the exchange mints
//! it: the secret must reach the CLI and nothing else. A trust policy carries
//! no secret, so **the approval itself writes the rows**, under the approver's
//! session — what clicking through the API access page does — and the code (or
//! the device-code request) stores which rows it wrote. The exchange and the
//! poll only report them: `{ "registration": … }`, the account, the policy,
//! the app, and which of them the approval created. Never a credential.
//!
//! So an approval nobody collects is not lost: the rows are there, and a later
//! `init-ci` that may read them finds them and asks nobody. That is what lets
//! an agent ask, hand the person the link, and go (`init-ci --no-wait`).
//!
//! ## Who may approve it, and what is written
//!
//! The door of the session-only routes that create both, for the org the
//! approval names: `OrgAdminFor<ServiceAccounts>` after the org is resolved as
//! `org_middleware` resolves one (`service_account::managed_org`). The writes
//! are the routes' own functions: `accounts::create_row` and
//! `trust_policies::plan` + `write`, so the audit rows, the org's environment
//! requirement and the GitHub lookup are the web flow's. Every check that can
//! refuse the policy runs before the account is created.
//!
//! The app is named by id or by slug — an asker who is not an org admin may
//! not list the org's apps — and resolved in the org, as the approver.
//!
//! The repository's ids are always GitHub's answer, never the asker's: a run
//! is matched on them, and the page shows the approver only the name. A
//! repository GitHub will not show the deployment is refused
//! (`repository_unresolved`); an admin registers that one on the account's
//! Trusted access page, where they type the ids themselves.
//!
//! An account of that name is reused, at whatever role it has (publishing an
//! app needs none above member); a new one is created as a member, and only
//! when the approval says to. A disabled account is refused. A policy that
//! already trusts the same run to publish the same app is reused, not doubled.

use entity::apps;
use entity::prelude::Apps;
use entity::service_accounts;
use oxy_app_core::audit::RequestActor;
use oxy_auth::token::account_access::{AccountRole, CreateAccountBody};
use oxy_auth::token::service_account as store;
use oxy_auth::token::trust_policy_mint::{self as mint, PolicyRequest};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::error::TokenError;
use super::service_account::{managed_org, usable_as_is};
use crate::org_api_access::accounts;
use crate::org_api_access::dto::{ServiceAccountDto, TrustPolicyDto};
use crate::org_api_access::trust_policies::{self, PlannedPolicy};

fn parse(body: &Value) -> Result<PolicyRequest, TokenError> {
    mint::parse(body).map_err(|e| TokenError::InvalidTrustPolicy(e.0))
}

/// What a new account's description says on the API access page.
const DESCRIPTION: &str = "Publishes from GitHub Actions. Created by `oxyc init-ci`.";

/// The org's app the request names, by id or by slug. An app of another org,
/// or none, is 404 `app_not_found` naming it as asked: the approval is not a
/// way to learn which apps exist.
async fn app_named(
    db: &DatabaseConnection,
    org_id: Uuid,
    raw: &str,
) -> Result<apps::Model, TokenError> {
    let found = match Uuid::parse_str(raw) {
        Ok(id) => Apps::find_by_id(id).one(db).await?,
        Err(_) => {
            Apps::find()
                .filter(apps::Column::OrgId.eq(org_id))
                .filter(apps::Column::Slug.eq(raw))
                .one(db)
                .await?
        }
    };
    found
        .filter(|app| app.org_id == org_id)
        .ok_or_else(|| TokenError::AppNotFound(raw.to_string()))
}

/// A request that has passed every check a session can make.
struct Checked {
    org: entity::organizations::Model,
    /// The app the policy may publish, resolved in the org.
    app: apps::Model,
    account: Option<service_accounts::Model>,
    planned: PlannedPolicy,
    /// A policy of the account that already trusts this run to publish this app.
    existing: Option<TrustPolicyDto>,
}

/// Whether `policy` trusts the run `planned` describes to publish `app_id`.
fn same_trust(policy: &TrustPolicyDto, planned: &PlannedPolicy, app_id: Uuid) -> bool {
    policy.disabled_at.is_none()
        && policy.repository.eq_ignore_ascii_case(planned.repository())
        && policy.workflow_path == planned.workflow_path()
        && policy.environment.as_deref().map(str::to_ascii_lowercase)
            == planned.environment().map(str::to_ascii_lowercase)
        && policy
            .grants
            .iter()
            .any(|g| g.kind == "app_publish" && g.app_id == Some(app_id))
}

/// The policy the request asks for, planned as the create route plans one —
/// with no ids from the asker, so GitHub's answer is the only one.
async fn plan(
    db: &DatabaseConnection,
    org_id: Uuid,
    role: AccountRole,
    request: &PolicyRequest,
    app_id: Uuid,
) -> Result<PlannedPolicy, TokenError> {
    trust_policies::plan(db, org_id, role, &request.policy_body(app_id))
        .await
        .map_err(|e| match e {
            TokenError::Invalid(why) => TokenError::InvalidTrustPolicy(why),
            TokenError::RepositoryUnresolved => {
                TokenError::RepositoryUnconfirmed(request.repository.clone())
            }
            other => other,
        })
}

async fn check(
    db: &DatabaseConnection,
    actor: &RequestActor,
    request: &PolicyRequest,
) -> Result<Checked, TokenError> {
    let org = managed_org(db, actor, &request.org).await?;
    let app = app_named(db, org.id, &request.app).await?;
    let account = store::find_by_name(db, org.id, &request.account).await?;
    let role = match &account {
        Some(account) => {
            usable_as_is(db, account).await?;
            store::role_of(account).ok_or_else(|| {
                TokenError::Internal("service account has an unknown org_role".into())
            })?
        }
        None if request.create_account => AccountRole::Member,
        None => return Err(TokenError::ServiceAccountNotFound(request.account.clone())),
    };
    let planned = plan(db, org.id, role, request, app.id).await?;
    let existing = match &account {
        Some(account) => trust_policies::list(db, account)
            .await?
            .into_iter()
            .find(|p| same_trust(p, &planned, app.id)),
        None => None,
    };
    Ok(Checked {
        org,
        app,
        account,
        planned,
        existing,
    })
}

/// What a code or a device-code request stores once the approval has written
/// the rows: which rows, so the collection can report them.
const REGISTERED: &str = "registered";

/// A `mint` of `kind: "trust_policy"`, approved under the session: checked,
/// and **carried out** — the account created when asked and missing, the
/// policy written unless one already trusts this run — as the approver.
/// Returns what the code or request stores: the rows it wrote, by id.
pub(super) async fn approve(
    db: &DatabaseConnection,
    actor: &RequestActor,
    body: &Value,
) -> Result<Value, TokenError> {
    let request = parse(body)?;
    let checked = check(db, actor, &request).await?;
    let app_id = checked.app.id;
    let (account, created_account) = account_for(db, actor, &request, &checked).await?;
    let (policy_id, created_policy) = match checked.existing {
        Some(policy) => (policy.id, false),
        None => {
            // An account that lost a create race may stand at another role
            // than the one the policy was checked for: check it again for
            // the account it is.
            let planned = if created_account || checked.account.is_some() {
                checked.planned
            } else {
                let role = store::role_of(&account).ok_or_else(|| {
                    TokenError::Internal("service account has an unknown org_role".into())
                })?;
                plan(db, account.org_id, role, &request, app_id).await?
            };
            let written = trust_policies::write(db, actor, &account, planned).await?;
            (written.id, true)
        }
    };
    Ok(json!({
        "kind": mint::KIND,
        REGISTERED: {
            "org": account.org_id,
            "account": account.user_id,
            "policy": policy_id,
            "app": app_id,
            "created_account": created_account,
            "created_policy": created_policy,
        },
    }))
}

/// What an approved registration answers the CLI with: no credential.
#[derive(Debug, Serialize)]
pub struct Registered {
    pub service_account: ServiceAccountDto,
    pub trust_policy: TrustPolicyDto,
    /// The app the policy publishes — what an asker who named it by slug
    /// checks the policy's grant against.
    pub app: PreviewApp,
    pub created_account: bool,
    pub created_policy: bool,
}

/// The account the approval names: the one there, or a new member. A create
/// that loses a race to another of the same name takes the winner.
async fn account_for(
    db: &DatabaseConnection,
    actor: &RequestActor,
    request: &PolicyRequest,
    checked: &Checked,
) -> Result<(service_accounts::Model, bool), TokenError> {
    if let Some(account) = &checked.account {
        return Ok((account.clone(), false));
    }
    let body = CreateAccountBody {
        name: request.account.clone(),
        description: Some(DESCRIPTION.into()),
        org_role: Some(AccountRole::Member.as_str().into()),
    };
    match accounts::create_row(db, actor, checked.org.id, body).await {
        Ok(account) => Ok((account, true)),
        Err(TokenError::NameTaken) => {
            let account = store::find_by_name(db, checked.org.id, &request.account)
                .await?
                .ok_or_else(|| TokenError::Internal("a taken name names no account".into()))?;
            usable_as_is(db, &account).await?;
            Ok((account, false))
        }
        Err(e) => Err(e),
    }
}

/// The rows an approval stored, by id.
#[derive(Deserialize)]
struct StoredRegistration {
    org: Uuid,
    account: Uuid,
    policy: Uuid,
    app: Uuid,
    created_account: bool,
    created_policy: bool,
}

/// Report what an approval wrote: the rows as they are now. Nothing is
/// written here — the approval did that, under the approver's session — so
/// nothing is checked again either; a row gone since reads as a code that
/// can no longer be honoured.
pub(super) async fn redeem(
    db: &DatabaseConnection,
    approved: &Value,
) -> Result<Registered, TokenError> {
    let stored: StoredRegistration = approved
        .get(REGISTERED)
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or_else(|| TokenError::Internal("a trust policy's approval stored no rows".into()))?;
    let account = store::find(db, stored.org, stored.account)
        .await?
        .ok_or(TokenError::NotFound)?;
    let trust_policy = trust_policies::list(db, &account)
        .await?
        .into_iter()
        .find(|p| p.id == stored.policy)
        .ok_or(TokenError::NotFound)?;
    let app = Apps::find_by_id(stored.app)
        .one(db)
        .await?
        .filter(|app| app.org_id == stored.org)
        .ok_or(TokenError::NotFound)?;
    Ok(Registered {
        service_account: accounts::dto(db, &account).await?,
        trust_policy,
        app: PreviewApp::of(&app),
        created_account: stored.created_account,
        created_policy: stored.created_policy,
    })
}

// ── The approval page's preview ──────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PreviewOrg {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct PreviewAccount {
    pub name: String,
    pub role: &'static str,
    /// `false`: approving creates it.
    pub exists: bool,
}

#[derive(Debug, Serialize)]
pub struct PreviewApp {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

impl PreviewApp {
    fn of(app: &apps::Model) -> Self {
        Self {
            id: app.id,
            slug: app.slug.clone(),
            name: app.name.clone(),
        }
    }
}

/// The run a policy trusts, as it would be stored.
#[derive(Debug, Serialize)]
pub struct PreviewPolicy {
    /// `owner/repo` as GitHub names it.
    pub repository: String,
    pub workflow_path: String,
    pub environment: Option<String>,
    /// A policy already trusts this run to publish this app: approving writes nothing.
    pub exists: bool,
}

/// What approving would write, as the server would write it.
#[derive(Debug, Serialize)]
pub struct Preview {
    pub org: PreviewOrg,
    pub account: PreviewAccount,
    pub app: PreviewApp,
    pub policy: PreviewPolicy,
}

/// The preview of `body`: every check the approval makes, and nothing written.
pub(super) async fn preview(
    db: &DatabaseConnection,
    actor: &RequestActor,
    body: &Value,
) -> Result<Preview, TokenError> {
    let request = parse(body)?;
    let checked = check(db, actor, &request).await?;
    let role = match &checked.account {
        Some(account) => store::role_of(account).unwrap_or(AccountRole::Member),
        None => AccountRole::Member,
    };
    Ok(Preview {
        org: PreviewOrg {
            id: checked.org.id,
            slug: checked.org.slug.clone(),
            name: checked.org.name.clone(),
        },
        account: PreviewAccount {
            name: request.account.clone(),
            role: role.as_str(),
            exists: checked.account.is_some(),
        },
        app: PreviewApp::of(&checked.app),
        policy: PreviewPolicy {
            repository: checked.planned.repository().to_string(),
            workflow_path: checked.planned.workflow_path().to_string(),
            environment: checked.planned.environment().map(str::to_string),
            exists: checked.existing.is_some(),
        },
    })
}
