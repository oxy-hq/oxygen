//! The apps that could tell a person something, decided for all of them at
//! once.
//!
//! The inverse of `oxy_app_core::audience::app_audience`: an app's
//! audience is its org's members plus the org's active frontline workers
//! holding a grant on it, so a person's candidate apps are those of the orgs
//! they belong to, plus the apps they hold a frontline grant on. Each is then
//! decided by the same rule `preferences::may_be_told_by` applies to one app —
//! `Standing::admits` and `preferences::may_be_told` — from the person's own
//! rows ([`PersonReach`]), never an org's roster. So the catalogue costs the
//! same handful of queries for a person in one app or in fifty, however many
//! members each org has.

use std::collections::{BTreeSet, HashSet};

use entity::{app_environments, apps, notifications};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect,
    QueryTrait,
};
use uuid::Uuid;

use crate::preferences::may_be_told;
use oxy_app_core::audience::PersonReach;

/// The apps to weigh for the person `reach` describes, in one query: those
/// with a build in an org they are a member of, or that they hold a grant on
/// in an org where they are active crew; and every app in `also` (the
/// sources they received from or muted), whatever its build.
pub async fn candidates(
    db: &DatabaseConnection,
    reach: &PersonReach,
    also: &BTreeSet<Uuid>,
) -> Result<Vec<apps::Model>, DbErr> {
    let granted: Vec<Uuid> = if reach.crew_orgs.is_empty() {
        Vec::new()
    } else {
        reach.granted_apps.iter().copied().collect()
    };
    if reach.member_orgs.is_empty() && granted.is_empty() && also.is_empty() {
        return Ok(Vec::new());
    }
    // A build in production or staging: an environment row naming one.
    let with_build = app_environments::Entity::find()
        .select_only()
        .column(app_environments::Column::AppId)
        .filter(app_environments::Column::Name.is_in(["production", "staging"]))
        .filter(app_environments::Column::BuildId.is_not_null())
        .into_query();
    let has_build = apps::Column::Id.in_subquery(with_build);
    let in_reach = Condition::any()
        .add(apps::Column::OrgId.is_in(reach.member_orgs.iter().copied()))
        .add(
            Condition::all()
                .add(apps::Column::Id.is_in(granted))
                .add(apps::Column::OrgId.is_in(reach.crew_orgs.iter().copied())),
        );
    apps::Entity::find()
        .filter(
            Condition::any()
                .add(Condition::all().add(in_reach).add(has_build))
                .add(apps::Column::Id.is_in(also.iter().copied())),
        )
        .all(db)
        .await
}

/// The ids of those of `apps` that can notify `user_id`, whose rows `reach`
/// holds: [`may_be_told`] for each, as `preferences::may_be_told_by` decides
/// it for one, with one more query — which of them the person received from.
pub async fn told_by_many(
    db: &DatabaseConnection,
    user_id: Uuid,
    reach: &PersonReach,
    apps: &[apps::Model],
) -> Result<HashSet<Uuid>, DbErr> {
    if apps.is_empty() {
        return Ok(HashSet::new());
    }
    let received: HashSet<Uuid> = notifications::Entity::find()
        .select_only()
        .column(notifications::Column::AppId)
        .filter(notifications::Column::UserId.eq(user_id))
        .filter(notifications::Column::AppId.is_in(apps.iter().map(|a| a.id)))
        .distinct()
        .into_tuple::<Option<Uuid>>()
        .all(db)
        .await?
        .into_iter()
        .flatten()
        .collect();
    Ok(apps
        .iter()
        .filter(|a| {
            let in_audience = reach.standing(a.org_id, a.id).admits();
            may_be_told(received.contains(&a.id), in_audience)
        })
        .map(|a| a.id)
        .collect())
}
