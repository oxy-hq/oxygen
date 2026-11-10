//! Bringing `notification_node_settings` back in step with the three old mute
//! tables, at every `oxy serve` and `oxy worker` boot.
//!
//! # Why
//!
//! While this release mirrors every mute into the old tables
//! ([`super::store::mirror_mute`], same transaction), the two agree: a
//! channel is `false` at a node exactly where an old table has its row. They
//! diverge only when something writes the old tables alone — the previous
//! release, during a rollback to it (or the minutes of a rollout). The
//! migration's one-shot backfill has already run by then, so without this a
//! roll-forward would ignore a mute made on the old binary (the person is
//! told again on a channel they turned off) and keep a mute the old binary
//! removed.
//!
//! The difference between the two is exactly those changes, so this applies
//! it:
//!
//! - an old-table mute where the node's channel **inherits** becomes `false`;
//! - a `false` with no old-table row goes back to inherit.
//!
//! An explicit `true` and every floor are left alone: the old binary could
//! not see or write either. A node left with nothing set is deleted, and
//! each person changed gets a new settings `version`.
//!
//! # Against live writes, and other pods
//!
//! One transaction under a transaction-scoped advisory lock, so pods booting
//! together run it one after another and the second finds nothing to do.
//! Within it, the people whose tables differ are found first, then **their
//! settings rows are locked** (`FOR UPDATE`, in id order) — the lock every
//! other writer takes first ([`super::store::lock`]) — and only then is
//! anything compared again and written, for those people alone. So a mute or
//! unmute in flight finishes before the comparison reads it (no stale read
//! can revert it), and the lock order is the writers' own: person, then
//! nodes, so there is no deadlock with them. With nothing different — every
//! boot but the one after a rollback — nothing is locked.
//!
//! # Contracting
//!
//! This and the mirror are a pair: **the release that stops calling
//! `mirror_mute` must also stop calling this** — otherwise every mute made
//! after it would have no old-table row and be cleared at the next boot.

use std::collections::BTreeSet;

use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};
use uuid::Uuid;

use super::store;

/// Every old-table mute, as (person, source, node) with a flag per channel.
const OLD_MUTES: &str = "\
    old_mutes AS ( \
      SELECT user_id, app_id, node_type, node, \
             bool_or(channel = 'push') AS push, \
             bool_or(channel = 'email') AS email, \
             bool_or(channel = 'slack') AS slack \
      FROM ( \
        SELECT user_id, app_id, 'kind' AS node_type, kind AS node, channel \
          FROM notification_mutes \
        UNION ALL \
        SELECT user_id, NULL::uuid, 'kind', kind, channel FROM notification_platform_mutes \
        UNION ALL \
        SELECT user_id, app_id, 'scope', scope, channel FROM notification_scope_mutes \
      ) rows \
      GROUP BY user_id, app_id, node_type, node \
    )";

/// The same node in both.
const SAME_NODE: &str = "s.user_id = o.user_id AND s.app_id IS NOT DISTINCT FROM o.app_id \
    AND s.node_type = o.node_type AND s.node = o.node";

/// People whose two tables differ: an old-table mute where the settings
/// table inherits, or a `false` there with no old-table row.
const DIFFERING: &str = "\
    SELECT o.user_id FROM old_mutes o \
    LEFT JOIN notification_node_settings s ON {same} \
    WHERE (o.push AND s.push IS NULL) OR (o.email AND s.email IS NULL) \
       OR (o.slack AND s.slack IS NULL) \
    UNION \
    SELECT s.user_id FROM notification_node_settings s \
    LEFT JOIN old_mutes o ON {same} \
    WHERE (s.push = false AND NOT coalesce(o.push, false)) \
       OR (s.email = false AND NOT coalesce(o.email, false)) \
       OR (s.slack = false AND NOT coalesce(o.slack, false))";

/// A node with an old-table mute and no row of its own: the row, off there.
const INSERT_MUTED: &str = "\
    INSERT INTO notification_node_settings \
      (id, user_id, app_id, node_type, node, push, email, slack) \
    SELECT gen_random_uuid(), o.user_id, o.app_id, o.node_type, o.node, \
           CASE WHEN o.push THEN false END, \
           CASE WHEN o.email THEN false END, \
           CASE WHEN o.slack THEN false END \
    FROM old_mutes o \
    WHERE o.user_id IN ({people}) \
      AND NOT EXISTS (SELECT 1 FROM notification_node_settings s WHERE {same}) \
    ON CONFLICT DO NOTHING \
    RETURNING user_id";

/// A node with a row: an old-table mute turns an inheriting channel off.
const UPDATE_MUTED: &str = "\
    UPDATE notification_node_settings s SET \
      push = CASE WHEN s.push IS NULL AND o.push THEN false ELSE s.push END, \
      email = CASE WHEN s.email IS NULL AND o.email THEN false ELSE s.email END, \
      slack = CASE WHEN s.slack IS NULL AND o.slack THEN false ELSE s.slack END, \
      updated_at = now() \
    FROM old_mutes o \
    WHERE {same} AND s.user_id IN ({people}) \
      AND ((s.push IS NULL AND o.push) OR (s.email IS NULL AND o.email) \
           OR (s.slack IS NULL AND o.slack)) \
    RETURNING s.user_id";

/// A `false` whose old-table row is gone goes back to inherit.
const CLEAR_UNMUTED: &str = "\
    UPDATE notification_node_settings s SET \
      push = CASE WHEN s.push = false AND NOT coalesce(o.push, false) THEN NULL ELSE s.push END, \
      email = CASE WHEN s.email = false AND NOT coalesce(o.email, false) THEN NULL ELSE s.email END, \
      slack = CASE WHEN s.slack = false AND NOT coalesce(o.slack, false) THEN NULL ELSE s.slack END, \
      updated_at = now() \
    FROM notification_node_settings t \
    LEFT JOIN old_mutes o ON o.user_id = t.user_id AND o.app_id IS NOT DISTINCT FROM t.app_id \
      AND o.node_type = t.node_type AND o.node = t.node \
    WHERE s.id = t.id AND s.user_id IN ({people}) \
      AND ((s.push = false AND NOT coalesce(o.push, false)) \
           OR (s.email = false AND NOT coalesce(o.email, false)) \
           OR (s.slack = false AND NOT coalesce(o.slack, false))) \
    RETURNING s.user_id";

/// What one reconcile changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Reconciled {
    /// Channels turned off from an old-table mute (rows written).
    pub muted: u64,
    /// Rows whose `false` went back to inherit.
    pub unmuted: u64,
    /// People whose settings changed.
    pub people: usize,
}

/// Apply the old tables' rollback-era changes to the settings table.
pub async fn reconcile(db: &DatabaseConnection) -> Result<Reconciled, DbErr> {
    let txn = db.begin().await?;
    txn.execute_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended('notification_settings_reconcile', 0))",
    ))
    .await?;
    let differing: BTreeSet<Uuid> = users_of(&txn, DIFFERING, &[]).await?.into_iter().collect();
    if differing.is_empty() {
        txn.commit().await?;
        return Ok(Reconciled::default());
    }
    let people: Vec<Uuid> = differing.into_iter().collect();
    lock_people(&txn, &people).await?;
    // Compared again from here: every statement's snapshot is taken after
    // the people's writes in flight have committed.
    let inserted = users_of(&txn, INSERT_MUTED, &people).await?;
    let updated = users_of(&txn, UPDATE_MUTED, &people).await?;
    let cleared = users_of(&txn, CLEAR_UNMUTED, &people).await?;
    txn.execute_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        format!(
            "DELETE FROM notification_node_settings \
             WHERE user_id IN ({}) AND push IS NULL AND email IS NULL AND slack IS NULL \
               AND min_level IS NULL",
            in_list(&people)
        ),
    ))
    .await?;
    let changed: BTreeSet<Uuid> = inserted
        .iter()
        .chain(&updated)
        .chain(&cleared)
        .copied()
        .collect();
    for user in &changed {
        store::bump(&txn, *user).await?;
    }
    txn.commit().await?;
    Ok(Reconciled {
        muted: (inserted.len() + updated.len()) as u64,
        unmuted: cleared.len() as u64,
        people: changed.len(),
    })
}

/// Take `people`'s settings rows, in id order, creating any that are
/// missing — what [`store::lock`] does for one person.
async fn lock_people(txn: &impl ConnectionTrait, people: &[Uuid]) -> Result<(), DbErr> {
    let ids = in_list(people);
    txn.execute_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        format!(
            "INSERT INTO notification_user_settings (user_id) \
             SELECT id FROM users WHERE id IN ({ids}) ORDER BY id ON CONFLICT DO NOTHING"
        ),
    ))
    .await?;
    txn.query_all_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        format!(
            "SELECT user_id FROM notification_user_settings WHERE user_id IN ({ids}) \
             ORDER BY user_id FOR UPDATE"
        ),
    ))
    .await?;
    Ok(())
}

/// `people` as a SQL list of uuid literals. Built from `Uuid`s, whose text
/// is hex and dashes only.
fn in_list(people: &[Uuid]) -> String {
    people
        .iter()
        .map(|id| format!("'{id}'::uuid"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Run `statement` (after the old-mutes CTE, for `people`) and the person of
/// each row it read or wrote.
async fn users_of(
    txn: &impl ConnectionTrait,
    statement: &str,
    people: &[Uuid],
) -> Result<Vec<Uuid>, DbErr> {
    let statement = statement
        .replace("{same}", SAME_NODE)
        .replace("{people}", &in_list(people));
    let sql = format!("WITH {OLD_MUTES} {statement}");
    txn.query_all_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await?
        .iter()
        .map(|row| row.try_get::<Uuid>("", "user_id"))
        .collect()
}

/// [`reconcile`] at boot, after migrations: best-effort, logged, never a
/// reason not to start.
pub async fn at_boot() {
    let db = match oxy::database::client::establish_connection().await {
        Ok(db) => db,
        Err(e) => {
            tracing::warn!(error = %e, "notification settings not reconciled: no database");
            return;
        }
    };
    match reconcile(&db).await {
        Ok(r) if r.people > 0 => tracing::info!(
            muted = r.muted,
            unmuted = r.unmuted,
            people = r.people,
            "notification settings reconciled with mutes made on a previous release"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "notification settings could not be reconciled"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_people_list_is_uuid_literals_only() {
        let a = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let b = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        assert_eq!(
            in_list(&[a, b]),
            "'11111111-1111-1111-1111-111111111111'::uuid,'22222222-2222-2222-2222-222222222222'::uuid"
        );
        for statement in [INSERT_MUTED, UPDATE_MUTED, CLEAR_UNMUTED] {
            assert!(
                statement.contains("{people}"),
                "every write is for the locked people only"
            );
        }
    }
}
