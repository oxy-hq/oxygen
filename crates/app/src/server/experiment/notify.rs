//! Owner notices. `owner:` names a member by email, or an org team; a value
//! that resolves to nobody still reaches the org's owners and admins, and the
//! title says so. A notice is written once per (run, reason, recipient): a retried
//! tick may repeat an action, never a notice.

use entity::{
    experiment_runs, notifications, org_members, org_team_members, org_teams, users, workspaces,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, PaginatorTrait,
    QueryFilter, Set,
    sea_query::{Expr, ExprTrait, Func},
};
use uuid::Uuid;

pub const NOTICE_KIND: &str = "experiment";
pub const SUBJECT_KIND: &str = "experiment_run";

/// Tell the run's owner `reason`, once per `(run, reason, recipient)`.
///
/// The dedupe is check-then-insert with no unique key: it is safe only because
/// the tick's claim serialises a run, so two notifies for one run never race.
///
/// Never errors the caller: a notice that cannot be written is logged loudly.
pub async fn notify_owner(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    owner: &str,
    reason: &str,
) {
    if let Err(e) = try_notify(db, run, owner, reason).await {
        tracing::warn!(
            target: "experiment", run_id = %run.id, error = %e, %reason,
            "owner notice was not written"
        );
    }
}

async fn try_notify(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    owner: &str,
    reason: &str,
) -> Result<(), DbErr> {
    let org = workspaces::Entity::find_by_id(run.workspace_id)
        .one(db)
        .await?;
    let Some(org_id) = org.and_then(|w| w.org_id) else {
        tracing::warn!(target: "experiment", run_id = %run.id, %reason, "workspace has no org to notify");
        return Ok(());
    };
    let (recipients, resolved) = recipients(db, org_id, owner).await?;
    let name = super::store::experiment_name(db, run).await?;
    let title = if resolved {
        format!("Experiment {name}")
    } else {
        format!(
            "Experiment {name} (owner `{owner}` matched no member or team; sent to the org's \
             owners and admins)"
        )
    };
    for user_id in recipients {
        // Per recipient: a write that failed mid-loop leaves the rest unnotified,
        // and the retry must reach them without repeating the ones already told.
        if already_sent(db, run.id, reason, user_id).await? {
            continue;
        }
        // The inbox row directly, as before platform notices existed: the
        // `oxy.*` kinds render from a record and store no text, which a
        // free-text reason does not fit yet.
        notifications::ActiveModel {
            id: Set(Uuid::new_v4()),
            org_id: Set(org_id),
            user_id: Set(user_id),
            kind: Set(NOTICE_KIND.into()),
            title: Set(title.clone()),
            body: Set(Some(reason.to_string())),
            link: Set(None),
            subject_kind: Set(Some(SUBJECT_KIND.into())),
            subject_id: Set(Some(run.id.to_string())),
            created_at: Set(chrono::Utc::now().fixed_offset()),
            read_at: Set(None),
            message_id: Set(None),
            app_id: Set(None),
            thread_key: Set(None),
        }
        .insert(db)
        .await?;
    }
    Ok(())
}

async fn already_sent(
    db: &DatabaseConnection,
    run_id: Uuid,
    reason: &str,
    user_id: Uuid,
) -> Result<bool, DbErr> {
    let n = notifications::Entity::find()
        .filter(notifications::Column::UserId.eq(user_id))
        .filter(notifications::Column::SubjectKind.eq(SUBJECT_KIND))
        .filter(notifications::Column::SubjectId.eq(run_id.to_string()))
        .filter(notifications::Column::Body.eq(reason))
        .count(db)
        .await?;
    Ok(n > 0)
}

/// `(user ids, whether `owner` itself resolved)`.
pub async fn recipients(
    db: &DatabaseConnection,
    org_id: Uuid,
    owner: &str,
) -> Result<(Vec<Uuid>, bool), DbErr> {
    let named = if owner.contains('@') {
        member_by_email(db, org_id, owner).await?
    } else {
        team_members(db, org_id, owner).await?
    };
    if !named.is_empty() {
        return Ok((named, true));
    }
    let officers = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(
            org_members::Column::Role
                .is_in([org_members::OrgRole::Owner, org_members::OrgRole::Admin]),
        )
        .all(db)
        .await?
        .into_iter()
        .map(|m| m.user_id)
        .collect();
    Ok((officers, false))
}

async fn member_by_email(
    db: &DatabaseConnection,
    org_id: Uuid,
    email: &str,
) -> Result<Vec<Uuid>, DbErr> {
    // Both sides lowered by Postgres, as `oxy_platform::filters::email_matches`
    // does: one definition of "same letters", served by `idx_users_email_lower`.
    let ids: Vec<Uuid> = users::Entity::find()
        .filter(
            Expr::expr(Func::lower(Expr::col((
                users::Entity,
                users::Column::Email,
            ))))
            .eq(Func::lower(Expr::val(email))),
        )
        .all(db)
        .await?
        .into_iter()
        .map(|u| u.id)
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.is_in(ids))
        .all(db)
        .await?
        .into_iter()
        .map(|m| m.user_id)
        .collect())
}

async fn team_members(
    db: &DatabaseConnection,
    org_id: Uuid,
    team: &str,
) -> Result<Vec<Uuid>, DbErr> {
    let Some(team) = org_teams::Entity::find()
        .filter(org_teams::Column::OrgId.eq(org_id))
        .filter(org_teams::Column::Name.eq(team))
        .one(db)
        .await?
    else {
        return Ok(Vec::new());
    };
    Ok(org_team_members::Entity::find()
        .filter(org_team_members::Column::TeamId.eq(team.id))
        .all(db)
        .await?
        .into_iter()
        .map(|m| m.user_id)
        .collect())
}
