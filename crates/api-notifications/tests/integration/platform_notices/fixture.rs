//! The seed and read helpers `platform_notices` and its modules share: an
//! org with two owners, an admin and a member (and no OLTP store), its
//! workspaces, and who was told. `oxy-server`'s `billing_past_due_notice`
//! includes this file by path (`#[path]`): billing's notice is driven by
//! `oxy-api-billing` and read back through this surface, and a surface may not
//! depend on another, not even for its tests.

use chrono::Utc;
use entity::org_members::OrgRole;
use entity::workspaces::WorkspaceStatus;
use entity::{
    notification_deliveries, notifications, org_members, organizations, users, workspaces,
};
use sea_orm::ActiveValue::Set;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

pub(crate) struct Org {
    pub(crate) id: Uuid,
    pub(crate) slug: String,
    pub(crate) owner: Uuid,
    pub(crate) second_owner: Uuid,
    pub(crate) admin: Uuid,
    pub(crate) member: Uuid,
}

pub(crate) async fn seed_user(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: Set(id),
        email: Set(Some(format!("p-{}@example.com", id.simple()))),
        name: Set(format!("Person {}", &id.simple().to_string()[..6])),
        picture: Set(None),
        email_verified: Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    id
}

pub(crate) async fn join(db: &DatabaseConnection, org: Uuid, user: Uuid, role: OrgRole) {
    let now = Utc::now().fixed_offset();
    org_members::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org),
        user_id: Set(user),
        role: Set(role),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .expect("join org");
}

/// An org with two owners, an admin and a member — and no OLTP store, which a
/// platform notice must not need.
pub(crate) async fn seed_org(db: &DatabaseConnection) -> Org {
    let id = Uuid::new_v4();
    let slug = format!("notices-{}", id.simple());
    let now = Utc::now().fixed_offset();
    organizations::ActiveModel {
        id: Set(id),
        name: Set("Acme".into()),
        slug: Set(slug.clone()),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    let org = Org {
        id,
        slug,
        owner: seed_user(db).await,
        second_owner: seed_user(db).await,
        admin: seed_user(db).await,
        member: seed_user(db).await,
    };
    join(db, id, org.owner, OrgRole::Owner).await;
    join(db, id, org.second_owner, OrgRole::Owner).await;
    join(db, id, org.admin, OrgRole::Admin).await;
    join(db, id, org.member, OrgRole::Member).await;
    org
}

pub(crate) async fn seed_workspace(
    db: &DatabaseConnection,
    org: Uuid,
    name: &str,
    status: WorkspaceStatus,
    created_by: Option<Uuid>,
) -> Uuid {
    let id = Uuid::new_v4();
    let now = Utc::now().fixed_offset();
    workspaces::ActiveModel {
        id: Set(id),
        name: Set(name.into()),
        created_at: Set(now),
        updated_at: Set(now),
        org_id: Set(Some(org)),
        status: Set(status),
        created_by: Set(created_by),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    id
}

pub(crate) async fn told(
    db: &DatabaseConnection,
    kind: &str,
    subject_id: &str,
) -> Vec<notifications::Model> {
    notifications::Entity::find()
        .filter(notifications::Column::Kind.eq(kind))
        .filter(notifications::Column::SubjectId.eq(subject_id))
        .all(db)
        .await
        .expect("query notices")
}

pub(crate) fn people(rows: &[notifications::Model]) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = rows.iter().map(|r| r.user_id).collect();
    ids.sort();
    ids
}

pub(crate) fn sorted(mut ids: Vec<Uuid>) -> Vec<Uuid> {
    ids.sort();
    ids
}

pub(crate) async fn push_outcome(
    db: &DatabaseConnection,
    signal: Uuid,
) -> (String, Option<String>) {
    let row = notification_deliveries::Entity::find_by_id((signal, "push".to_string()))
        .one(db)
        .await
        .expect("query delivery")
        .expect("a push delivery was recorded");
    (row.status, row.reason)
}
