//! Settings in Oxy's database: `notification_node_settings` (one row per
//! node a person set something at) and `notification_user_settings` (one
//! row per person).
//!
//! **Every write holds the person's row** ([`lock`]), so two requests from
//! one person serialize, and a node row needs no upsert on a partial index.
//! Each write also bumps the person's `version` (the settings document's
//! `ETag`).
//!
//! **Mutes are mirrored into the three old mute tables** ([`mirror_mute`]) so
//! a re-pinned previous release keeps them. Delivery never reads those
//! tables; what the previous release writes to them while rolled back to is
//! brought back at boot by [`super::reconcile`]. Contracting them: first a
//! release that stops calling [`mirror_mute`] **and** `reconcile` (a pair:
//! one without the other clears every new mute), then one that drops the
//! tables.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::{notification_node_settings as nodes, notification_user_settings as people};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr,
    EntityTrait, QueryFilter, Set, Statement, TransactionTrait,
};
use uuid::Uuid;

use super::model::{GlobalSettings, Node, NodeSettings};
use super::quiet::QuietHours;
use crate::catalog::model::{Level, ancestors};
use crate::delivery::Channel;

/// One source's settings for one person, by node.
pub type SourceNodes = HashMap<Node, NodeSettings>;

/// One person's settings, by source (`None` = Oxygen).
pub type UserNodes = HashMap<Option<Uuid>, SourceNodes>;

fn settings_of(row: &nodes::Model) -> NodeSettings {
    NodeSettings {
        push: row.push,
        email: row.email,
        slack: row.slack,
        min_level: row.min_level.as_deref().and_then(Level::parse),
    }
}

fn global_of(row: &people::Model) -> GlobalSettings {
    let quiet_hours = match (
        row.quiet_start,
        row.quiet_end,
        row.quiet_days,
        &row.quiet_allow,
    ) {
        (Some(start), Some(end), Some(days), Some(allow)) => Some(QuietHours {
            start: u16::try_from(start).unwrap_or(0),
            end: u16::try_from(end).unwrap_or(0),
            days: u8::try_from(days).unwrap_or(0),
            allow: Level::parse(allow).unwrap_or(Level::Critical),
        }),
        _ => None,
    };
    GlobalSettings {
        paused_until: row.paused_until.map(|t| t.with_timezone(&Utc)),
        quiet_hours,
        timezone: row.timezone.clone(),
    }
}

/// `user_id`'s person-level settings and the settings `version`.
pub async fn person(
    db: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<(GlobalSettings, i64), DbErr> {
    Ok(people::Entity::find_by_id(user_id)
        .one(db)
        .await?
        .map(|row| (global_of(&row), row.version))
        .unwrap_or_default())
}

/// The person-level settings of each of `users` who has any.
pub async fn people_settings(
    db: &impl ConnectionTrait,
    users: &[Uuid],
) -> Result<HashMap<Uuid, GlobalSettings>, DbErr> {
    Ok(people::Entity::find()
        .filter(people::Column::UserId.is_in(users.iter().copied()))
        .all(db)
        .await?
        .iter()
        .map(|row| (row.user_id, global_of(row)))
        .collect())
}

/// Everything `user_id` set at any node of any source.
pub async fn nodes_for_user(db: &impl ConnectionTrait, user_id: Uuid) -> Result<UserNodes, DbErr> {
    let mut out: UserNodes = HashMap::new();
    for row in nodes::Entity::find()
        .filter(nodes::Column::UserId.eq(user_id))
        .all(db)
        .await?
    {
        if let Some(node) = Node::from_stored(&row.node_type, row.node.clone()) {
            out.entry(row.app_id)
                .or_default()
                .insert(node, settings_of(&row));
        }
    }
    Ok(out)
}

/// What each of `users` set on the way from `kind` (in `category`) up to
/// source `app`: the kind, every category above it, the source.
pub async fn nodes_for_kind(
    db: &impl ConnectionTrait,
    users: &[Uuid],
    app: Option<Uuid>,
    kind: &str,
    category: &str,
) -> Result<HashMap<Uuid, SourceNodes>, DbErr> {
    let mut scopes = ancestors(category);
    scopes.push(String::new());
    let on_path = sea_orm::Condition::any()
        .add(
            sea_orm::Condition::all()
                .add(nodes::Column::NodeType.eq("kind"))
                .add(nodes::Column::Node.eq(kind)),
        )
        .add(
            sea_orm::Condition::all()
                .add(nodes::Column::NodeType.eq("scope"))
                .add(nodes::Column::Node.is_in(scopes)),
        );
    let find = nodes::Entity::find()
        .filter(nodes::Column::UserId.is_in(users.iter().copied()))
        .filter(on_path);
    let find = match app {
        Some(app) => find.filter(nodes::Column::AppId.eq(app)),
        None => find.filter(nodes::Column::AppId.is_null()),
    };
    let mut out: HashMap<Uuid, SourceNodes> = HashMap::new();
    for row in find.all(db).await? {
        if let Some(node) = Node::from_stored(&row.node_type, row.node.clone()) {
            out.entry(row.user_id)
                .or_default()
                .insert(node, settings_of(&row));
        }
    }
    Ok(out)
}

/// Take `user_id`'s settings row for this transaction, creating it when the
/// person has none. Every write below runs after this. Returns the current
/// `version`.
pub async fn lock(txn: &impl ConnectionTrait, user_id: Uuid) -> Result<i64, DbErr> {
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO notification_user_settings (user_id) VALUES ($1) ON CONFLICT DO NOTHING",
        [user_id.into()],
    ))
    .await?;
    let row = txn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT version FROM notification_user_settings WHERE user_id = $1 FOR UPDATE",
            [user_id.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("notification_user_settings".into()))?;
    row.try_get("", "version")
}

/// Bump `user_id`'s `version`, creating the row if needed: every write to
/// the person's settings or devices calls this. Returns the new version.
pub async fn bump(db: &impl ConnectionTrait, user_id: Uuid) -> Result<i64, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO notification_user_settings (user_id, version) VALUES ($1, 1) \
             ON CONFLICT (user_id) DO UPDATE \
               SET version = notification_user_settings.version + 1, updated_at = now() \
             RETURNING version",
            [user_id.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("notification_user_settings".into()))?;
    row.try_get("", "version")
}

/// Write `user_id`'s person-level settings. Under [`lock`].
pub async fn write_global(
    txn: &impl ConnectionTrait,
    user_id: Uuid,
    global: &GlobalSettings,
) -> Result<(), DbErr> {
    let q = global.quiet_hours.as_ref();
    people::ActiveModel {
        user_id: Set(user_id),
        paused_until: Set(global.paused_until.map(|t| t.fixed_offset())),
        timezone: Set(global.timezone.clone()),
        quiet_start: Set(q.map(|q| q.start as i16)),
        quiet_end: Set(q.map(|q| q.end as i16)),
        quiet_days: Set(q.map(|q| i16::from(q.days))),
        quiet_allow: Set(q.map(|q| q.allow.as_str().to_string())),
        updated_at: Set(Utc::now().fixed_offset()),
        ..Default::default()
    }
    .update(txn)
    .await?;
    Ok(())
}

/// Advance how far quiet hours' summary has reached. Under [`lock`].
pub async fn summarized_through(
    txn: &impl ConnectionTrait,
    user_id: Uuid,
    through: DateTime<Utc>,
) -> Result<(), DbErr> {
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE notification_user_settings SET quiet_summary_through = $2 WHERE user_id = $1",
        [user_id.into(), through.fixed_offset().into()],
    ))
    .await?;
    Ok(())
}

/// When quiet hours' summary last reached, if it ever ran. Under [`lock`].
pub async fn summary_through(
    txn: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<Option<DateTime<Utc>>, DbErr> {
    Ok(people::Entity::find_by_id(user_id)
        .one(txn)
        .await?
        .and_then(|row| row.quiet_summary_through)
        .map(|t| t.with_timezone(&Utc)))
}

fn find_node(user_id: Uuid, app: Option<Uuid>, node: &Node) -> sea_orm::Select<nodes::Entity> {
    let find = nodes::Entity::find()
        .filter(nodes::Column::UserId.eq(user_id))
        .filter(nodes::Column::NodeType.eq(node.type_str()))
        .filter(nodes::Column::Node.eq(node.name()));
    match app {
        Some(app) => find.filter(nodes::Column::AppId.eq(app)),
        None => find.filter(nodes::Column::AppId.is_null()),
    }
}

/// What `user_id` set at `node` of source `app` (`None` = Oxygen).
pub async fn node(
    db: &impl ConnectionTrait,
    user_id: Uuid,
    app: Option<Uuid>,
    node: &Node,
) -> Result<NodeSettings, DbErr> {
    Ok(find_node(user_id, app, node)
        .one(db)
        .await?
        .map(|row| settings_of(&row))
        .unwrap_or_default())
}

/// Set what `user_id` has at `node` to `next`: an update, an insert, or —
/// when nothing is left set — a delete. Every channel that changed is
/// mirrored into the old mute tables. Under [`lock`].
pub async fn put_node(
    txn: &impl ConnectionTrait,
    user_id: Uuid,
    app: Option<Uuid>,
    node: &Node,
    next: &NodeSettings,
) -> Result<(), DbErr> {
    let row = find_node(user_id, app, node).one(txn).await?;
    let before = row.as_ref().map(settings_of).unwrap_or_default();
    match (row, next.is_empty()) {
        (None, true) => {}
        (Some(row), true) => {
            nodes::Entity::delete_by_id(row.id).exec(txn).await?;
        }
        (row, false) => {
            let model = nodes::ActiveModel {
                id: Set(row.as_ref().map_or_else(Uuid::new_v4, |r| r.id)),
                user_id: Set(user_id),
                app_id: Set(app),
                node_type: Set(node.type_str().to_string()),
                node: Set(node.name().to_string()),
                push: Set(next.push),
                email: Set(next.email),
                slack: Set(next.slack),
                min_level: Set(next.min_level.map(|l| l.as_str().to_string())),
                updated_at: Set(Utc::now().fixed_offset()),
            };
            match row {
                Some(_) => model.update(txn).await.map(|_| ())?,
                None => model.insert(txn).await.map(|_| ())?,
            }
        }
    }
    for channel in Channel::ALL {
        let (was, now) = (before.channel(channel), next.channel(channel));
        if (was == Some(false)) != (now == Some(false)) {
            mirror_mute(txn, user_id, app, node, channel, now == Some(false)).await?;
        }
    }
    Ok(())
}

/// Set one channel at one node — `Some(false)` off, `Some(true)` on, `None`
/// inherit — in a transaction of its own, and bump the version. What the old
/// preferences route writes; checked by the caller.
pub async fn set_channel(
    db: &DatabaseConnection,
    user_id: Uuid,
    app: Option<Uuid>,
    at: &Node,
    channel: Channel,
    value: Option<bool>,
) -> Result<(), DbErr> {
    let txn = db.begin().await?;
    lock(&txn, user_id).await?;
    let mut next = node(&txn, user_id, app, at).await?;
    next.set_channel(channel, value);
    put_node(&txn, user_id, app, at, &next).await?;
    bump(&txn, user_id).await?;
    txn.commit().await
}

/// Write a mute (or its removal) into the old table for its node shape, so
/// the previous release still honours it after a rollback.
pub async fn mirror_mute(
    txn: &impl ConnectionTrait,
    user_id: Uuid,
    app: Option<Uuid>,
    node: &Node,
    channel: Channel,
    muted: bool,
) -> Result<(), DbErr> {
    let (sql, values): (&str, Vec<sea_orm::Value>) = match (node, app, muted) {
        (Node::Kind(kind), Some(app), true) => (
            "INSERT INTO notification_mutes (user_id, app_id, kind, channel) \
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
            vec![
                user_id.into(),
                app.into(),
                kind.as_str().into(),
                channel.as_str().into(),
            ],
        ),
        (Node::Kind(kind), Some(app), false) => (
            "DELETE FROM notification_mutes \
             WHERE user_id = $1 AND app_id = $2 AND kind = $3 AND channel = $4",
            vec![
                user_id.into(),
                app.into(),
                kind.as_str().into(),
                channel.as_str().into(),
            ],
        ),
        (Node::Kind(kind), None, true) => (
            "INSERT INTO notification_platform_mutes (user_id, kind, channel) \
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            vec![
                user_id.into(),
                kind.as_str().into(),
                channel.as_str().into(),
            ],
        ),
        (Node::Kind(kind), None, false) => (
            "DELETE FROM notification_platform_mutes \
             WHERE user_id = $1 AND kind = $2 AND channel = $3",
            vec![
                user_id.into(),
                kind.as_str().into(),
                channel.as_str().into(),
            ],
        ),
        (Node::Scope(scope), _, true) => (
            "INSERT INTO notification_scope_mutes (id, user_id, app_id, scope, channel) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
            vec![
                Uuid::new_v4().into(),
                user_id.into(),
                app.into(),
                scope.as_str().into(),
                channel.as_str().into(),
            ],
        ),
        (Node::Scope(scope), _, false) => (
            "DELETE FROM notification_scope_mutes \
             WHERE user_id = $1 AND app_id IS NOT DISTINCT FROM $2 \
               AND scope = $3 AND channel = $4",
            vec![
                user_id.into(),
                app.into(),
                scope.as_str().into(),
                channel.as_str().into(),
            ],
        ),
    };
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await?;
    Ok(())
}
