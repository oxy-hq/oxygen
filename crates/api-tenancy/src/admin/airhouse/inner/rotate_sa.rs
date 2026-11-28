//! Rotate one workspace's Airhouse service account: the bearer-leak response.
//!
//! This was `oxy admin airhouse rotate-sa <workspace_id>`, a CLI that needed
//! the deployment's `OXY_DATABASE_URL`, sealing key and Airhouse admin token on
//! the operator's laptop. It is now a route (`oxyc airhouse rotate-sa`), so the
//! person needs a token and the server holds the secrets. The work is the same
//! call, [`TenantProvisioner::rotate_service_account`](airhouse::TenantProvisioner).
//!
//! Like [`super::provision`], there is no `router()` here: the route is
//! declared by `oxy_api_tenancy::admin::airhouse`, which fences and audits it
//! before delegating.
//!
//! **Revoke comes before mint.** A failure after the revoke (Airhouse down
//! for the mint, a sealing error) leaves the old SA dead and the tenants row
//! still naming it, so the workspace cannot mint until the call succeeds.
//! Running it again is the fix: the revoke is idempotent on the Airhouse side.

use axum::extract::{Json, Path};
use axum::http::StatusCode;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use serde::Serialize;
use tracing::{info, instrument};
use uuid::Uuid;

use ::airhouse::provisioner::{ProvisionerError, RotatedServiceAccount};

/// What a rotation answers. Ids and a time, never a bearer: the new one is
/// sealed onto the tenants row and nowhere else.
#[derive(Debug, Serialize)]
pub struct RotatedSa {
    pub workspace_id: Uuid,
    pub old_sa_id: String,
    pub new_sa_id: String,
    /// RFC 3339.
    pub rotated_at: String,
}

impl From<RotatedServiceAccount> for RotatedSa {
    fn from(r: RotatedServiceAccount) -> Self {
        Self {
            workspace_id: r.workspace_id,
            old_sa_id: r.old_sa_id,
            new_sa_id: r.new_sa_id,
            rotated_at: r.rotated_at.to_rfc3339(),
        }
    }
}

/// Revoke the workspace's Airhouse SA and mint a fresh one under the same
/// deterministic name. Not idempotent in effect (every call makes a new SA),
/// but safe to repeat.
#[instrument(skip(user), fields(user_id = %user.id, workspace_id = %workspace_id))]
pub async fn rotate_service_account(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<RotatedSa>, (StatusCode, String)> {
    let db = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Airhouse not configured on this deployment is a normal state, not a
    // fault, so 503, as `provision` answers.
    let provisioner = ::airhouse::config::provisioner_for(db).ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Airhouse is not configured on this deployment".to_string(),
    ))?;

    info!(user = %user.label(), workspace_id = %workspace_id, "rotating airhouse SA");
    let rotated = provisioner
        .rotate_service_account(workspace_id)
        .await
        .map_err(|e| (status_for(&e), e.to_string()))?;
    Ok(Json(rotated.into()))
}

/// The status each failure answers with.
///
/// - `WorkspaceNotFound` is raised when there is no **tenant row** (the shim
///   has already checked that the workspace exists): nothing to rotate, 404.
/// - `TenantHasNoServiceAccount` is a tenant that was never bound. That is a
///   state an operator resolves by provisioning first, so 409.
/// - Anything from Airhouse is a 502: the failure is upstream, and the call is
///   safe to repeat.
fn status_for(e: &ProvisionerError) -> StatusCode {
    match e {
        ProvisionerError::WorkspaceNotFound(_) => StatusCode::NOT_FOUND,
        ProvisionerError::TenantHasNoServiceAccount(_) => StatusCode::CONFLICT,
        ProvisionerError::Airhouse(_) | ProvisionerError::AirhouseTimeout(_) => {
            StatusCode::BAD_GATEWAY
        }
        ProvisionerError::Db(_)
        | ProvisionerError::Crypto(_)
        | ProvisionerError::InvalidTenantName(_)
        | ProvisionerError::TenantNameTaken(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::airhouse::AirhouseError;

    #[test]
    fn each_failure_answers_the_status_an_operator_acts_on() {
        let ws = Uuid::nil();
        assert_eq!(
            status_for(&ProvisionerError::WorkspaceNotFound(ws)),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_for(&ProvisionerError::TenantHasNoServiceAccount(ws)),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_for(&ProvisionerError::Airhouse(AirhouseError::Provisioning(
                "down".into()
            ))),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            status_for(&ProvisionerError::AirhouseTimeout(20)),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            status_for(&ProvisionerError::Crypto("seal".into())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    /// The response names the accounts and the time, and nothing that could
    /// authenticate.
    #[test]
    fn the_response_carries_ids_and_a_time_only() {
        let rotated = RotatedServiceAccount {
            workspace_id: Uuid::nil(),
            old_sa_id: "sa-old".into(),
            new_sa_id: "sa-new".into(),
            rotated_at: chrono::DateTime::parse_from_rfc3339("2026-11-21T00:00:00+00:00").unwrap(),
        };
        let body = serde_json::to_value(RotatedSa::from(rotated)).unwrap();
        let mut keys: Vec<_> = body.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["new_sa_id", "old_sa_id", "rotated_at", "workspace_id"]
        );
        assert_eq!(body["rotated_at"], "2026-11-21T00:00:00+00:00");
    }
}
