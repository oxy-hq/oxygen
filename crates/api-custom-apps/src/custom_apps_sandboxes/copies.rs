//! Whether a sandbox holds a copy of production rows, and so the day it is
//! deleted for it: the one rule the maintenance sweep's fork pass deletes by
//! ([`super::maintenance`], step 4) and `env show` reports
//! (`EnvironmentDto::copies_expire_at`), so the two cannot disagree.
//!
//! A sandbox holds a copy while its planes copy production
//! ([`super::maintenance::expires_from_fork`]), **or** while its Airhouse
//! overlay still lists a table it copied: changing the Airhouse plane does not
//! empty the overlay — only a reset or a teardown does.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use entity::app_environments;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::planes::{AirhouseMode, ResolvedPlanes, planes_from_row};
use oxy_oltp::env_branches::EXPIRY_DAYS;
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};

use super::airhouse_overlay::set_pin;
use super::delete::forked_at;
use super::maintenance::expires_from_fork;
use crate::custom_apps_env_branch::planes::record_fork_at;
use crate::custom_apps_functions::env_policy::airhouse_overlay::overlay_key;

/// The registry key of `row`'s Airhouse overlay; `None` for a name that is
/// not an environment.
pub(super) fn overlay_key_of(row: &app_environments::Model) -> Option<String> {
    AppEnvironment::parse(&row.name).map(|environment| overlay_key(row.app_id, &environment))
}

/// The overlay keys among `keys` whose registry still lists a table they
/// copied (any state but `dropped`).
pub(super) async fn holding_copies<C: ConnectionTrait>(
    conn: &C,
    keys: &[String],
) -> Result<HashSet<String>, DbErr> {
    if keys.is_empty() {
        return Ok(HashSet::new());
    }
    let placeholders: Vec<String> = (1..=keys.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "SELECT DISTINCT preview_key FROM workspace_preview_tables \
          WHERE state <> 'dropped' AND preview_key IN ({})",
        placeholders.join(", ")
    );
    let values = keys.iter().map(|key| key.clone().into());
    let rows = conn
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values,
        ))
        .await?;
    rows.iter()
        .map(|row| row.try_get("", "preview_key"))
        .collect()
}

/// Whether `row`'s overlay still lists a table it copied.
pub(super) async fn holds_copies<C: ConnectionTrait>(
    conn: &C,
    row: &app_environments::Model,
) -> Result<bool, DbErr> {
    let Some(key) = overlay_key_of(row) else {
        return Ok(false);
    };
    Ok(!holding_copies(conn, &[key]).await?.is_empty())
}

/// Whether `row`'s planes say it expires from its fork.
pub(super) fn expires_row(
    row: &app_environments::Model,
    expires: fn(&ResolvedPlanes) -> bool,
) -> bool {
    let Some(environment) = AppEnvironment::parse(&row.name) else {
        return false;
    };
    match planes_from_row(Some(row), &environment) {
        Ok(planes) => planes.as_ref().is_some_and(expires),
        Err(why) => {
            tracing::warn!(app_id = %row.app_id, environment = %row.name, %why,
                "sandbox_maintenance: planes unreadable; not expiring from the fork");
            false
        }
    }
}

/// When the fork pass deletes `row`, given whether its overlay `holding` a
/// copied table: [`EXPIRY_DAYS`] past its fork point while it holds a copy of
/// production, else `None`.
pub(super) fn copies_expire_at(
    row: &app_environments::Model,
    holding: bool,
) -> Option<DateTime<Utc>> {
    (holding || expires_row(row, expires_from_fork))
        .then(|| forked_at(row) + chrono::Duration::days(EXPIRY_DAYS))
}

/// What a planes write from `before` to `after` (the same sandbox's row, in
/// the write's transaction) starts:
///
/// - **The fork clock, when it starts copying.** A sandbox that held no copy
///   and now copies production counts its 30 days from now, not from a fork
///   point that may be months old: the overlay copies on write, from live
///   production. Without this, turning `airhouse=branch` on for an old
///   sandbox had the next sweep delete it. One that already held a copy keeps
///   its clock: the oldest copy governs. An OLTP cut queued by the same write
///   records its own cut point after this (`queue_for_change`).
/// - **A snapshot pinned at now, when it enters `snapshot`.** A pin left from
///   an earlier stretch on `snapshot` would read production days back and be
///   refused as too old; a pin taken from an old fork point would be too.
pub(super) async fn start_copying<C: ConnectionTrait>(
    conn: &C,
    before: &app_environments::Model,
    after: &app_environments::Model,
) -> Result<(), DbErr> {
    let Some(environment) = AppEnvironment::parse(&after.name) else {
        return Ok(());
    };
    let now = Utc::now();
    let starts = !expires_row(before, expires_from_fork) && expires_row(after, expires_from_fork);
    if starts && !holds_copies(conn, before).await? {
        record_fork_at(conn, after.app_id, &after.name, now).await?;
    }
    let snapshot = |row| airhouse_of(row, &environment) == Some(AirhouseMode::Snapshot);
    if !snapshot(before) && snapshot(after) {
        set_pin(conn, after.app_id, &environment, Some(now)).await?;
    }
    Ok(())
}

/// `row`'s Airhouse mode; `None` when its planes do not resolve.
fn airhouse_of(
    row: &app_environments::Model,
    environment: &AppEnvironment,
) -> Option<AirhouseMode> {
    planes_from_row(Some(row), environment)
        .ok()
        .flatten()
        .map(|planes| planes.airhouse)
}
