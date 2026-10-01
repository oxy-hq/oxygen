//! Staff-facing status and reset for an org's OLTP branch.
//!
//! Mounted, like the rest of the admin surface, by
//! `oxy_app::server::api::admin::oltp`, which fences every org-keyed route
//! before delegating here — see `admin.rs` for why there is no router in this
//! crate.
//!
//! **Reset is two-phase by contract.** Without `"confirm": true` the route
//! resets nothing and answers `428` with the apps a reset would hit, so no
//! client — a console button, a script, an `oxyc` that forgot to prompt — can
//! discard an org's staging data with a bare POST. The ruling behind it
//! (env design §11 #16): org-level, on demand, confirmed, listing every app.

use axum::extract::{Json, Path};
use axum::http::StatusCode;
use chrono::{DateTime, FixedOffset, Utc};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use tracing::{info, instrument, warn};
use uuid::Uuid;

use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_platform::db::establish_connection;

use crate::OltpBranch;
use crate::branches::{
    AffectedApp, AffectedPipeline, STALE_AFTER_DAYS, affected_apps, affected_pipelines, age_of,
};

/// One org's branch, as the console and `oxyc oltp status` show it. No
/// credentials, ever.
#[derive(Debug, Serialize)]
pub struct BranchStatusResponse {
    pub branch: OltpBranch,
    pub provisioned: bool,
    /// `active`, `provisioning` or `resetting`; `None` when not provisioned.
    pub status: Option<String>,
    pub host: Option<String>,
    pub database: Option<String>,
    pub provider_branch_id: Option<String>,
    pub created_at: Option<DateTime<FixedOffset>>,
    pub last_reset_at: Option<DateTime<FixedOffset>>,
    /// Whole days since the last cut — creation, or the last reset.
    pub age_days: Option<i64>,
    /// `age_days` past [`STALE_AFTER_DAYS`]. A warning, never an action: the
    /// ruling is that nothing resets on a timer.
    pub stale: bool,
    pub stale_after_days: i64,
    /// The org's apps with an OLTP writer — whose staging data a reset
    /// discards. Listed whether or not the branch exists, so a console can say
    /// who a branch would serve before anyone makes one.
    pub affected_apps: Vec<AffectedApp>,
    /// The Airway pipelines' `raw_*` schemas, which a reset re-copies too.
    pub affected_pipelines: Vec<AffectedPipeline>,
}

impl BranchStatusResponse {
    fn none(
        branch: OltpBranch,
        affected_apps: Vec<AffectedApp>,
        affected_pipelines: Vec<AffectedPipeline>,
    ) -> Self {
        Self {
            branch,
            provisioned: false,
            status: None,
            host: None,
            database: None,
            provider_branch_id: None,
            created_at: None,
            last_reset_at: None,
            age_days: None,
            stale: false,
            stale_after_days: STALE_AFTER_DAYS,
            affected_apps,
            affected_pipelines,
        }
    }
}

/// What the status read says about `org_id`'s `branch`.
pub async fn status_for(
    db: &DatabaseConnection,
    org_id: Uuid,
    branch: OltpBranch,
) -> Result<BranchStatusResponse, sea_orm::DbErr> {
    let (tenant, row) = crate::branches::find(db, org_id, branch).await?;
    let (apps, pipelines) = match &tenant {
        Some(t) => (
            affected_apps(db, org_id, t.id).await?,
            affected_pipelines(db, t.id).await?,
        ),
        None => (Vec::new(), Vec::new()),
    };
    let Some(row) = row else {
        return Ok(BranchStatusResponse::none(branch, apps, pipelines));
    };
    let age = age_of(Utc::now().into(), row.created_at, row.last_reset_at);
    Ok(BranchStatusResponse {
        branch,
        provisioned: true,
        status: Some(row.status.as_str().to_string()),
        host: Some(row.host),
        database: Some(row.database_name),
        provider_branch_id: Some(row.provider_branch_id),
        created_at: Some(row.created_at),
        last_reset_at: row.last_reset_at,
        age_days: Some(age.age_days),
        stale: age.stale,
        stale_after_days: STALE_AFTER_DAYS,
        affected_apps: apps,
        affected_pipelines: pipelines,
    })
}

/// `staging` → the branch; anything else is a 400 rather than a guess.
pub fn parse_branch(name: &str) -> Result<OltpBranch, (StatusCode, String)> {
    OltpBranch::parse(name).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("unknown OLTP branch {name:?} — the only one is `staging`"),
        )
    })
}

fn db_error(e: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::error!("oltp branch: {e}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal error".to_string(),
    )
}

/// `GET …/oltp/branches/{branch}` — the branch's status, age and reach.
#[instrument(skip(user), fields(user_id = %user.id, org_id = %org_id))]
pub async fn get_branch(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((org_id, branch)): Path<(Uuid, String)>,
) -> Result<Json<BranchStatusResponse>, (StatusCode, String)> {
    let branch = parse_branch(&branch)?;
    let db = establish_connection().await.map_err(db_error)?;
    status_for(&db, org_id, branch)
        .await
        .map(Json)
        .map_err(db_error)
}

#[derive(Debug, Default, Deserialize)]
pub struct ResetRequest {
    /// Must be `true` for anything to happen. Absent means `false`.
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Serialize)]
pub struct ResetResponse {
    /// Whether the branch was reset — `false` on the unconfirmed 428.
    pub reset: bool,
    #[serde(flatten)]
    pub branch: BranchStatusResponse,
}

/// What a reset request may do — decided before anything is built or touched.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResetGate {
    /// No such branch: 404, whether or not it was confirmed.
    NoBranch,
    /// 428 with the apps it would hit; nothing reset.
    NeedsConfirm,
    Proceed,
}

/// The one rule the route exists to enforce: nothing is reset without an
/// explicit `confirm: true`.
pub(crate) fn reset_gate(provisioned: bool, confirm: bool) -> ResetGate {
    match (provisioned, confirm) {
        (false, _) => ResetGate::NoBranch,
        (true, false) => ResetGate::NeedsConfirm,
        (true, true) => ResetGate::Proceed,
    }
}

/// `POST …/oltp/branches/{branch}/reset` `{"confirm": true}`.
///
/// Unconfirmed: `428 Precondition Required` with the affected apps, and
/// nothing touched. Confirmed: re-cut from production, and the fresh status.
/// A branch that does not exist is a 404 either way.
#[instrument(skip(user, body), fields(user_id = %user.id, org_id = %org_id))]
pub async fn reset_branch(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((org_id, branch)): Path<(Uuid, String)>,
    Json(body): Json<ResetRequest>,
) -> Result<(StatusCode, Json<ResetResponse>), (StatusCode, String)> {
    let branch = parse_branch(&branch)?;
    let db = establish_connection().await.map_err(db_error)?;
    let before = status_for(&db, org_id, branch).await.map_err(db_error)?;
    match reset_gate(before.provisioned, body.confirm) {
        ResetGate::NoBranch => {
            let e = crate::provisioner::ProvisionerError::BranchNotProvisioned(org_id, branch);
            return Err((StatusCode::NOT_FOUND, e.to_string()));
        }
        ResetGate::NeedsConfirm => {
            info!(user = %user.label(), "OLTP branch reset requested without confirm; nothing reset");
            return Ok((
                StatusCode::PRECONDITION_REQUIRED,
                Json(ResetResponse {
                    reset: false,
                    branch: before,
                }),
            ));
        }
        ResetGate::Proceed => {}
    }

    let provisioner = crate::provisioner::from_env(db.clone())
        .await
        .map_err(super::admin::provisioner_status)?;
    warn!(
        user = %user.label(),
        apps = before.affected_apps.len(),
        "RESETTING an OLTP branch — every app's staging data in the org goes with it"
    );
    provisioner
        .reset_branch(org_id, branch)
        .await
        .map_err(super::admin::provisioner_status)?;
    let after = status_for(&db, org_id, branch).await.map_err(db_error)?;
    Ok((
        StatusCode::OK,
        Json(ResetResponse {
            reset: true,
            branch: after,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_staging_parses() {
        assert_eq!(parse_branch("staging").unwrap(), OltpBranch::Staging);
        for bad in ["Staging", "prod", "production", "", "staging "] {
            assert_eq!(
                parse_branch(bad).unwrap_err().0,
                StatusCode::BAD_REQUEST,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn nothing_is_reset_without_confirm() {
        assert_eq!(reset_gate(true, false), ResetGate::NeedsConfirm);
        assert_eq!(reset_gate(true, true), ResetGate::Proceed);
        // A missing branch is a 404 even when confirmed — there is nothing to
        // reset, and "confirmed" must not read as "create it".
        assert_eq!(reset_gate(false, true), ResetGate::NoBranch);
        assert_eq!(reset_gate(false, false), ResetGate::NoBranch);
    }

    #[test]
    fn an_absent_confirm_is_no() {
        let body: ResetRequest = serde_json::from_str("{}").unwrap();
        assert!(!body.confirm, "a bare POST must not reset anything");
        let body: ResetRequest = serde_json::from_str(r#"{"confirm": true}"#).unwrap();
        assert!(body.confirm);
    }

    #[test]
    fn the_unprovisioned_status_carries_the_line_and_no_age() {
        let s = serde_json::to_value(BranchStatusResponse::none(
            OltpBranch::Staging,
            vec![],
            vec![],
        ))
        .unwrap();
        assert_eq!(s["branch"], "staging");
        assert_eq!(s["provisioned"], false);
        assert_eq!(s["stale"], false);
        assert_eq!(s["stale_after_days"], 30);
        assert!(s["age_days"].is_null());
    }
}
