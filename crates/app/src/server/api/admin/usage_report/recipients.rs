//! Who gets the report, and the switch for each person.
//!
//! Its own router because it is its own authority: reading a report is
//! `operate_platform`, but listing staff and changing something for another
//! staff member belongs to whoever administers staff access
//! (`Action::PlatformGrants`). A person's own switch stays with the report's
//! routes, so nobody needs this capability to stop their own mail.

use axum::{
    Json, Router,
    extract::Path,
    http::StatusCode,
    response::Response,
    routing::{get, put},
};
use chrono::{DateTime, Utc};
use oxy_app_core::audit::{AuditEntry, RequestActor, record_best_effort};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_authz::Scope;
use serde::{Deserialize, Serialize};

use crate::server::api::admin::internal_jobs::{connect, db_err, error_body};
use crate::server::router::AppState;

use super::delivery::{self, Recipient};
use super::store;

/// Mounted under `/api/admin`, behind `Action::PlatformGrants`.
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/usage-report/recipients", get(list_recipients))
        .route("/usage-report/recipients/{email}", put(set_recipient))
}

/// One person the report is for, as the console lists them.
#[derive(Serialize)]
pub struct RecipientView {
    email: String,
    /// `global_owner`, or the role their grant was issued as.
    role: &'static str,
    /// Whether their grant reaches every organization.
    scope_all: bool,
    /// How many organizations a bounded grant names; `null` for an unbounded one.
    org_count: Option<usize>,
    enabled: bool,
    is_self: bool,
    /// Who last changed it; `null` while it is the default.
    updated_by: Option<String>,
    updated_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
pub struct Recipients {
    recipients: Vec<RecipientView>,
}

#[derive(Deserialize)]
pub struct SetRecipient {
    enabled: bool,
}

/// Everyone the weekly usage report is for, and whether each is emailed it.
/// The staff list, so it is read by the people who administer staff access.
pub async fn list_recipients(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<Recipients>, Response> {
    let db = connect().await?;
    let own = actor.email.as_deref().unwrap_or_default();
    let all = delivery::recipients(&db).await.map_err(db_err)?;
    Ok(Json(Recipients {
        recipients: all.into_iter().map(|r| view(r, own)).collect(),
    }))
}

/// Turn the weekly usage report email on or off for another person. Answers
/// 404 for an address the report is not for. The change is audited, and the
/// person can see who made it.
pub async fn set_recipient(
    actor: RequestActor,
    Path(email): Path<String>,
    Json(body): Json<SetRecipient>,
) -> Result<Json<RecipientView>, Response> {
    let db = connect().await?;
    let own = actor.user.email.clone().unwrap_or_default();
    let updated = delivery::set_enabled(&db, &email, body.enabled, &own)
        .await
        .map_err(db_err)?
        .ok_or_else(not_a_recipient)?;
    let action = if body.enabled {
        "usage_report.email_turned_on"
    } else {
        "usage_report.email_turned_off"
    };
    let target = updated.staff.email.clone();
    record_best_effort(
        &db,
        AuditEntry::for_request(&actor, action).target("staff", target.clone(), target),
    )
    .await;
    Ok(Json(view(updated, &own)))
}

fn view(recipient: Recipient, own: &str) -> RecipientView {
    let Recipient {
        staff,
        enabled,
        updated_by,
        updated_at,
    } = recipient;
    RecipientView {
        is_self: store::normalize(own) == staff.email,
        role: staff.role.map_or("global_owner", |role| role.as_str()),
        scope_all: staff.scope.is_all(),
        org_count: match &staff.scope {
            Scope::All => None,
            Scope::Orgs(orgs) => Some(orgs.len()),
        },
        email: staff.email,
        enabled,
        updated_by,
        updated_at,
    }
}

fn not_a_recipient() -> Response {
    error_body(
        StatusCode::NOT_FOUND,
        "not_a_recipient",
        Some("That address does not get the usage report.".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::authz::globals::StaffAddress;
    use oxy_authz::PlatformRole;
    use uuid::Uuid;

    fn recipient(email: &str, role: Option<PlatformRole>, scope: Scope) -> Recipient {
        Recipient {
            staff: StaffAddress {
                email: email.to_string(),
                scope,
                role,
            },
            enabled: true,
            updated_by: None,
            updated_at: None,
        }
    }

    #[test]
    fn the_owner_reads_as_global_owner_and_a_grant_as_its_role() {
        let owner = view(
            recipient("root@oxy.tech", None, Scope::All),
            "root@oxy.tech",
        );
        let json = serde_json::to_value(&owner).unwrap();
        assert_eq!(json["role"], "global_owner");
        assert_eq!(json["scope_all"], true);
        assert!(json["org_count"].is_null());
        assert_eq!(json["is_self"], true);
        assert_eq!(json["enabled"], true);
        assert!(json["updated_by"].is_null());

        let bounded = view(
            recipient(
                "admin@oxy.tech",
                Some(PlatformRole::GlobalAdmin),
                Scope::Orgs(vec![Uuid::from_u128(1), Uuid::from_u128(2)]),
            ),
            " Root@Oxy.Tech ",
        );
        let json = serde_json::to_value(&bounded).unwrap();
        assert_eq!(json["role"], "global_admin");
        assert_eq!(json["scope_all"], false);
        assert_eq!(json["org_count"], 2);
        assert_eq!(json["is_self"], false);
    }
}
