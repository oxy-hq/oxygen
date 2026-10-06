//! Admitting a **sandbox agent token** (`oxy_sbx_`): the checks and the one
//! mapping that are this kind's alone (sandbox agent credential design §3.2,
//! decision 8). Pure — the store reads the rows and hands them in.
//!
//! The kind is fixed at mint: never all-access, no partner standing, always an
//! expiry, and no grant but `app_sandbox`. A row that says otherwise was not
//! minted by this release, so it is refused rather than read charitably
//! ([`refuse_widened`], [`refuse_misplaced`]) — and an `app_sandbox` grant on
//! any other kind is refused the same way, since nothing enforces it there.
//!
//! What the rest of the token machinery reads is a token's **workspace**
//! grants, in four places: staff standing is bounded to their orgs, a staff
//! tool needs an admin ceiling over the org, the custom-app paths answer a
//! token with no grant on the app's workspace as they answer an unknown app,
//! and the validator refuses a grant kind it cannot read. So each live
//! `app_sandbox` grant is given the least that passes all four: one workspace
//! grant, on the workspace its app is published from, at `Admin`
//! ([`place_sandbox_apps`]). That is wider than the app — which is why the
//! kind holds no tenant standing at all (`PrincipalFacts::narrowed_by`).

use entity::api_tokens;
use oxy_authz::{RoleCeiling, TokenGrant};
use uuid::Uuid;

use super::admission::{ReadGrants, Refusal};
use super::credential::{CredentialContext, StoredKind};

/// A sandbox-agent row may not claim what the kind never holds, nor mirror an
/// `api_keys` row (which would make it read as a legacy key and narrow
/// nothing), nor live forever.
pub(super) fn refuse_widened(row: &api_tokens::Model) -> Result<(), Refusal> {
    if row.legacy_api_key_id.is_some() {
        return Err(Refusal::SandboxWidened("an api_keys mirror"));
    }
    if row.all_access {
        return Err(Refusal::SandboxWidened("all_access"));
    }
    if row.partner {
        return Err(Refusal::SandboxWidened("partner"));
    }
    if row.expires_at.is_none() {
        return Err(Refusal::SandboxWidened("no expiry"));
    }
    Ok(())
}

/// `app_sandbox` grants belong to a sandbox agent token and to nothing else,
/// and such a token holds no other kind of grant.
pub(super) fn refuse_misplaced(kind: StoredKind, grants: &ReadGrants) -> Result<(), Refusal> {
    if kind == StoredKind::SandboxAgent {
        if !grants.workspace.is_empty() || !grants.app_publish.is_empty() {
            return Err(Refusal::UnknownGrant(
                "other than app_sandbox on a sandbox agent token".to_string(),
            ));
        }
        return Ok(());
    }
    if !grants.app_sandbox.is_empty() {
        return Err(Refusal::UnknownGrant(format!(
            "kind 'app_sandbox' on a {} token",
            kind.as_str()
        )));
    }
    Ok(())
}

/// Where a granted app is right now: the org that owns it and the workspace it
/// is published from (`apps.org_id`, `apps.project_id`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SandboxAppHome {
    pub app_id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
}

/// Give a sandbox agent credential the workspace grants its apps stand for,
/// from where each app lives now. A grant whose app is gone, or is no longer
/// in the org the grant names, is dropped: it reaches nothing.
///
/// A no-op for every other kind, which holds no `app_sandbox` grant.
pub fn place_sandbox_apps(credential: &mut CredentialContext, homes: &[SandboxAppHome]) {
    if !credential.is_sandbox_agent() {
        return;
    }
    let home_of = |app_id: Uuid, org_id: Uuid| {
        homes
            .iter()
            .find(|h| h.app_id == app_id && h.org_id == org_id)
    };
    credential
        .app_sandbox
        .retain(|grant| home_of(grant.app_id, grant.org_id).is_some());
    let mut grants: Vec<TokenGrant> = Vec::new();
    for app in &credential.app_sandbox {
        let Some(home) = home_of(app.app_id, app.org_id) else {
            continue;
        };
        let grant = TokenGrant {
            org_id: home.org_id,
            workspace_id: Some(home.workspace_id),
            ceiling: RoleCeiling::Admin,
        };
        // Two apps published from one workspace need one grant between them.
        if !grants.contains(&grant) {
            grants.push(grant);
        }
    }
    credential.grants = grants;
}
