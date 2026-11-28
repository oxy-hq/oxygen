//! `POST /admin/orgs/{org_id}/oltp/audit` and
//! `POST /admin/orgs/{org_id}/oltp/roles/{role}/rotate` — the confinement
//! audit and a writer's password rotation, for staff.
//!
//! Both were the Rust `oxy oltp audit | rotate` (deleted 2026-11-21), which ran
//! on an operator's laptop with the deployment's database URL, sealing key and
//! provider token. Like [`super::admin`], there is no router here: the routes
//! are declared by `crate::admin::oltp`, which fences each one by
//! scope and writes its audit row before delegating to these.
//!
//! **The audit answers 200 when it finds a violation.** The check ran, and the
//! findings are its answer (`clean: false`); `oxyc oltp audit` turns that into
//! exit 9. Only an org with no database (404) or a database that would not
//! open a session (502) is an error status.
//!
//! **A rotation never returns the password,** and never echoes the statement
//! that set it: a failed `ALTER ROLE … PASSWORD` carries the new password in
//! its statement text ([`oxy_oltp::sql::SqlError::Statement`]), so the error body
//! is the server's reason alone ([`rotate_error`]).

use axum::extract::{Json, Path};
use axum::http::StatusCode;
use serde::Serialize;
use tracing::{error, instrument, warn};
use uuid::Uuid;

use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_platform::db::establish_connection;

use oxy_oltp::confinement_audit::{AuditError, ConfinementAudit, audit_org};

fn audit_status(e: AuditError) -> (StatusCode, String) {
    let code = match &e {
        AuditError::NotProvisioned(_) => StatusCode::NOT_FOUND,
        // The tenant database refused or could not be reached: the audit did
        // not run, which is not the same answer as "it ran and failed".
        AuditError::Connect(_) => StatusCode::BAD_GATEWAY,
        AuditError::Owner(_) | AuditError::Query(_) | AuditError::Grants(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    if code == StatusCode::INTERNAL_SERVER_ERROR {
        error!("oltp audit: {e}");
    }
    (code, e.to_string())
}

/// Every role in the org's database: what it is, whether it is still
/// confined, and what it can reach outside its own schema.
///
/// Opens a session as the tenant's owner, so it wakes a suspended compute —
/// which is why the route is a POST although it changes nothing in the
/// tenant: an act an operator takes, never a read a page load or a prefetch
/// may make (decided 2026-11-21, the same line `test-connection` draws).
#[instrument(skip(user), fields(user_id = %user.id, org_id = %org_id))]
pub async fn audit(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
) -> Result<Json<ConfinementAudit>, (StatusCode, String)> {
    let db = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    audit_org(&db, org_id).await.map(Json).map_err(audit_status)
}

/// What a rotation reports. Never the password or a DSN.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RotatedRole {
    /// The writer as it was named: `app:<slug>` or `pipeline:<source>`.
    pub writer: String,
    /// The Postgres role whose password changed.
    pub role: String,
    pub schema: String,
}

/// A provisioner error as a status and a body that cannot carry the password.
///
/// The status is [`super::admin::provisioner_status`]'s. The body is the error's
/// text, except for a failed statement, where it is the server's reason alone —
/// the statement is the one that set the password.
pub(crate) fn rotate_error(e: oxy_oltp::provisioner::ProvisionerError) -> (StatusCode, String) {
    use oxy_oltp::provisioner::ProvisionerError as P;
    let message = match &e {
        P::Sql(oxy_oltp::sql::SqlError::Statement { source_message, .. }) => {
            format!("statement failed: {source_message}")
        }
        other => other.to_string(),
    };
    (super::admin::provisioner_status(e).0, message)
}

/// Rotate one writer's password, reseal it, and re-assert confinement.
///
/// Only writers: the analyst and the owner are minted with the tenant and have
/// no rotation of their own. Re-provisioning re-mints them.
#[instrument(skip(user), fields(user_id = %user.id, org_id = %org_id))]
pub async fn rotate(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((org_id, role)): Path<(Uuid, String)>,
) -> Result<Json<RotatedRole>, (StatusCode, String)> {
    // Parsed before the provisioner is built, so a malformed role is a 400 and
    // not a 503 or a 500 from somewhere deeper.
    let writer = super::admin::parse_writer(&role).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let db = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let provisioner = oxy_oltp::provisioner::from_env(db)
        .await
        .map_err(super::admin::provisioner_status)?;

    warn!(user = %user.label(), org_id = %org_id, writer = %writer, "rotating an OLTP writer's password");
    let creds = provisioner
        .rotate_writer(org_id, &writer, oxy_oltp::schema::GrantLevel::ReadWrite)
        .await
        .map_err(rotate_error)?;
    Ok(Json(RotatedRole {
        writer: writer.to_string(),
        role: creds.role_name.clone(),
        schema: creds.schema_name.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_oltp::provisioner::ProvisionerError as P;

    /// A failed `ALTER ROLE … PASSWORD` must not hand the new password back in
    /// the error body, while keeping the server's reason and the status.
    #[test]
    fn a_failed_password_statement_does_not_echo_the_password() {
        let secret = "s3cr3t-Pa55word-xyz";
        let e = P::Sql(oxy_oltp::sql::SqlError::Statement {
            statement: format!("ALTER ROLE app_a_rw WITH LOGIN PASSWORD '{secret}'"),
            source_message: "[OXY01] role app_a_rw is not confined".to_string(),
        });
        let (status, body) = rotate_error(e);
        assert!(!body.contains(secret), "the password leaked: {body}");
        assert!(body.contains("OXY01"), "the reason survives: {body}");
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[test]
    fn other_errors_keep_their_text_and_status() {
        let org = Uuid::nil();
        let (status, body) = rotate_error(P::NotProvisioned(org));
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains(&org.to_string()), "{body}");
    }

    /// The JSON `oxyc oltp rotate` parses, and no credential in it.
    #[test]
    fn the_rotation_response_is_pinned_and_carries_no_credential() {
        let json = serde_json::to_value(RotatedRole {
            writer: "app:bookings".into(),
            role: "app_bookings_rw".into(),
            schema: "app_bookings".into(),
        })
        .expect("serializes");
        assert_eq!(
            json,
            serde_json::json!({
                "writer": "app:bookings",
                "role": "app_bookings_rw",
                "schema": "app_bookings",
            })
        );
        for key in ["dsn", "password"] {
            assert!(json.get(key).is_none(), "{key} must never be returned");
        }
    }

    #[test]
    fn audit_statuses() {
        let org = Uuid::nil();
        assert_eq!(
            audit_status(AuditError::NotProvisioned(org)).0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            audit_status(AuditError::Connect("quota".into())).0,
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            audit_status(AuditError::Query("x".into())).0,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
