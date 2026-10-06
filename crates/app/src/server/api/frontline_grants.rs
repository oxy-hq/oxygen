//! Granting a frontline worker the apps they were enrolled to use.
//!
//! A worker reaches an app through exactly one thing: an `app_members` row on
//! it (`Ring::AppAccess`'s frontline term, and `user_can_access_app`). Before
//! this existed, that row had no writer a worker could pass — the access
//! settings validated every grantee as an org member, which a worker never is —
//! so the whole flow was reachable only by hand-editing the table. Now the
//! access settings accept an active worker (`org_teams::service`), and
//! enrolment can grant the apps in the same call, because "add Maria to the
//! crew" and "let Maria open Store Ops" are one decision for the manager who
//! makes it.
//!
//! Validation runs BEFORE the worker is created, so a request naming another
//! org's app is refused whole rather than leaving a worker enrolled and
//! ungranted.

use entity::prelude::AppMembers;
use entity::{app_members, apps};
use oxy_authz::{Action, Resource};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

/// Whether `actor` may decide an app's audience in this org — the same ring the
/// app's access settings enforce (`Action::AppAccessManage`, `Ring::AppGrant`).
///
/// `OrgAdmin` on the enrol route is not that ring: it enforces `MemberSetRole`,
/// which a partner holding `manage_members` without `manage_apps` passes. Left
/// unchecked, such a partner could write through enrolment the grants the
/// access settings refuse them — the capability split the model exists to
/// keep. `existing_allow` is the guard the route already passed.
pub async fn may_grant_apps(
    db: &DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    org_id: Uuid,
) -> bool {
    oxy_server_authz::enforce_for(
        db,
        caller,
        "frontline.enrol_grants",
        Action::AppAccessManage,
        Resource::org(org_id),
        true,
    )
    .await
}

/// Sort and dedupe. `write_access` dedupes for the same reason: the upsert
/// tolerates a repeated id, but the log line and the echoed list would count
/// it twice.
pub fn normalize_app_ids(mut ids: Vec<Uuid>) -> Vec<Uuid> {
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[derive(Debug, thiserror::Error)]
pub enum GrantError {
    /// Some of the named apps are not this org's (or do not exist). The ids are
    /// reported so the admin can see which — an app id is not a secret to the
    /// org admin naming it.
    #[error("not this org's apps: {0:?}")]
    NotThisOrg(Vec<Uuid>),
    #[error("database error: {0}")]
    Db(#[from] DbErr),
}

/// Every id names an app published from THIS org; the rows come back so the
/// caller can name them. Cheap, and the enrol handler runs it BEFORE the worker
/// exists so a bad request leaves nothing behind — `grant_apps_to_worker` runs
/// it again as the safety net for any future caller, and that inner call is not
/// the one that keeps the ordering guarantee.
pub async fn validate_apps_in_org(
    db: &DatabaseConnection,
    org_id: Uuid,
    app_ids: &[Uuid],
) -> Result<Vec<apps::Model>, GrantError> {
    if app_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mine = apps::Entity::find()
        .filter(apps::Column::OrgId.eq(org_id))
        .filter(apps::Column::Id.is_in(app_ids.to_vec()))
        .all(db)
        .await?;
    let foreign: Vec<Uuid> = app_ids
        .iter()
        .copied()
        .filter(|id| !mine.iter().any(|a| a.id == *id))
        .collect();
    if foreign.is_empty() {
        Ok(mine)
    } else {
        Err(GrantError::NotThisOrg(foreign))
    }
}

/// Write a `member` grant on each app for the worker. Idempotent: an app they
/// already hold is left as it is (its role included — enrolment does not
/// demote an app admin). Returns the apps granted, which is all of `app_ids`
/// when validation passed. Authorization is the CALLER's — see
/// [`may_grant_apps`]; this writes what it is told.
pub async fn grant_apps_to_worker(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
    app_ids: &[Uuid],
    actor: Option<Uuid>,
) -> Result<Vec<apps::Model>, GrantError> {
    let granted = validate_apps_in_org(db, org_id, app_ids).await?;
    if app_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<app_members::ActiveModel> = app_ids
        .iter()
        .map(|app_id| app_members::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            app_id: ActiveValue::Set(*app_id),
            user_id: ActiveValue::Set(user_id),
            role: ActiveValue::Set(app_members::ROLE_MEMBER.to_string()),
            created_by: ActiveValue::Set(actor),
            created_at: ActiveValue::NotSet,
        })
        .collect();
    AppMembers::insert_many(rows)
        .on_conflict(
            OnConflict::columns([app_members::Column::AppId, app_members::Column::UserId])
                .do_nothing()
                .to_owned(),
        )
        .exec_without_returning(db)
        .await?;
    // The `(user, app)` access verdict is cached per replica for a minute; a
    // worker granted just now must not wait it out at the kiosk.
    crate::server::api::custom_apps_auth::invalidate_access_cache();
    Ok(granted)
}

#[cfg(test)]
mod tests {
    use super::normalize_app_ids;
    use uuid::Uuid;

    #[test]
    fn app_ids_are_deduped_before_they_are_counted_or_echoed() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        assert_eq!(normalize_app_ids(vec![b, a, b, a, b]), vec![a, b]);
        assert!(normalize_app_ids(vec![]).is_empty());
    }
}
