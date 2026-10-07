//! Test fixture: a request authenticated with a sandbox agent token.

use oxy_app_core::audit::RequestActor;
use oxy_auth::token::{AppSandboxGrant, CredentialContext, StoredKind};
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::{RoleCeiling, TokenGrant};
use uuid::Uuid;

/// The credential admission builds for token `token_id`, granted `app_id` of
/// `org_id` published from `workspace_id`.
pub fn credential(
    token_id: Uuid,
    org_id: Uuid,
    app_id: Uuid,
    workspace_id: Uuid,
) -> CredentialContext {
    CredentialContext {
        token_id,
        kind: StoredKind::SandboxAgent,
        principal_user_id: Uuid::from_u128(0x5B00),
        all_access: false,
        platform: true,
        partner: false,
        name: "agent".into(),
        display_prefix: "oxy_sbx_Ab3x".into(),
        legacy_api_key_id: None,
        grants: vec![TokenGrant {
            org_id,
            workspace_id: Some(workspace_id),
            ceiling: RoleCeiling::Admin,
        }],
        app_publish: Vec::new(),
        app_sandbox: vec![AppSandboxGrant {
            org_id,
            app_id,
            staging: false,
        }],
        blocked_orgs: Vec::new(),
        service_account: None,
        expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(8)),
    }
}

/// The minter, carrying `credential` as the auth entry points set it.
pub fn user(credential: Option<CredentialContext>) -> AuthenticatedUser {
    AuthenticatedUser {
        id: Uuid::from_u128(0x5B00),
        email: Some("minter@oxy.tech".into()),
        name: "Minter".into(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential,
    }
}

/// The request's actor: the minter on `credential`, or in a browser session.
pub fn actor(credential: Option<CredentialContext>) -> RequestActor {
    let mut actor = RequestActor::session(user(credential.clone()));
    actor.credential = credential;
    actor
}

/// An `apps` row: app `id` of `org_id`, published from `workspace_id`.
pub fn app(id: Uuid, org_id: Uuid, workspace_id: Uuid) -> entity::apps::Model {
    let now = chrono::Utc::now().fixed_offset();
    entity::apps::Model {
        visibility: "org".to_string(),
        id,
        slug: "ops".to_string(),
        name: "Ops".to_string(),
        org_id,
        project_id: workspace_id,
        branch: "main".to_string(),
        source_repo: String::new(),
        status: "created".to_string(),
        source_type: "s3".to_string(),
        source_config: serde_json::json!({}),
        bootstrap_pr_url: None,
        last_synced_at: None,
        manifest_override: None,
        published_at: None,
        repo_path: None,
        draft_build_id: None,
        published_build_id: None,
        last_promoted_by: None,
        last_promoted_at: None,
        created_at: now,
        updated_at: now,
    }
}
