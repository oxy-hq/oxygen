//! What a custom-app environment's own OLTP branch IS to the rest of Oxy: its
//! provider name, how long it lives, how many an org may hold, how far back it
//! may be forked, and the reads that find it (`oltp_env_branches`).
//!
//! Beside [`crate::branches`] (the org's one shared staging branch) and kept
//! apart from it: an environment's branch is per app, disposable, and cut at
//! a point the caller picks. The provisioner that cuts, resets and deletes
//! these rows builds on the reads here; the connection a function call gets is
//! [`crate::resolver::resolve_env_branch_writer_for_org`].

use chrono::{DateTime, Duration, Utc};
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr,
    EntityTrait, PaginatorTrait, QueryFilter, Statement,
};
use uuid::Uuid;

use crate::entity::env_branches::{self as rows, Entity as EnvBranches};
use crate::entity::tenants::{self as oltp_tenants, Entity as OltpTenants};

/// Days an environment's branch lives past its fork point before the provider
/// reaps it (and the sandbox sweep tears its sandbox down).
pub const EXPIRY_DAYS: i64 = 30;

/// The env var that caps how many environment branches one org may hold.
pub const MAX_ENV: &str = "OXY_APP_ENV_OLTP_BRANCHES_MAX";
/// The cap when [`MAX_ENV`] is unset or unparsable.
pub const MAX_DEFAULT: u64 = 8;

/// The env var for how many hours back a branch may be forked.
pub const HISTORY_ENV: &str = "OXY_OLTP_BRANCH_HISTORY_HOURS";
/// The window when [`HISTORY_ENV`] is unset or unparsable.
pub const HISTORY_DEFAULT_HOURS: i64 = 24;

/// How far ahead of the server's clock a fork point may sit before it is
/// refused as "in the future" — clock skew between a caller and the server.
const FUTURE_SLACK_SECONDS: i64 = 60;

/// Which environment of which app in which org.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvBranchKey {
    pub org_id: Uuid,
    pub app_id: Uuid,
    pub env_name: String,
}

/// The branch's name at the provider: `oxy-env-<app id, no dashes>-<env>`.
/// Derived from the key so a retried cut adopts the branch an earlier attempt
/// made, and distinct per environment so two never collide.
pub fn provider_name(app_id: Uuid, env_name: &str) -> String {
    format!("oxy-env-{}-{env_name}", app_id.simple())
}

/// When a branch forked at `fork_at` expires.
pub fn expires_at(fork_at: DateTime<Utc>) -> DateTime<Utc> {
    fork_at + Duration::days(EXPIRY_DAYS)
}

/// The most environment branches one org may hold.
pub fn max_per_org() -> u64 {
    parse_max(std::env::var(MAX_ENV).ok().as_deref())
}

/// What a refusal at the cap says: the org's `others` branches, the `max` it
/// may hold, and the way out. One text for the request-time check
/// (`PlaneSpec::validate`) and the cut's own (`cut_env_branch`), so a person
/// reads the same sentence whichever caught it.
pub fn cap_refusal(others: u64, max: u64) -> String {
    format!(
        "this org has {others} sandbox OLTP branches, the most {MAX_ENV} allows ({max}); use \
         --plane oltp=schema, the cheap mode, or delete a sandbox"
    )
}

fn parse_max(raw: Option<&str>) -> u64 {
    parse_or_default(MAX_ENV, raw, MAX_DEFAULT)
}

fn history_hours() -> i64 {
    parse_or_default(
        HISTORY_ENV,
        std::env::var(HISTORY_ENV).ok().as_deref(),
        HISTORY_DEFAULT_HOURS,
    )
}

/// `raw` parsed, or `default` — loudly, since a typo'd limit that silently
/// reverts is one nobody finds until it bites.
fn parse_or_default<T: std::str::FromStr + Copy>(name: &str, raw: Option<&str>, default: T) -> T {
    let Some(raw) = raw else { return default };
    raw.trim().parse().unwrap_or_else(|_| {
        tracing::warn!(env = name, value = raw, "unparsable; using the default");
        default
    })
}

/// Whether `at` is a fork point the server will cut: not in the future, and no
/// older than the window of history the provider keeps. `Err` names which.
pub fn check_fork_at(at: DateTime<Utc>, now: DateTime<Utc>) -> Result<(), String> {
    check_fork_at_within(at, now, history_hours())
}

fn check_fork_at_within(at: DateTime<Utc>, now: DateTime<Utc>, hours: i64) -> Result<(), String> {
    if at > now + Duration::seconds(FUTURE_SLACK_SECONDS) {
        return Err(format!("the fork point {at} is in the future (now {now})"));
    }
    if at < now - Duration::hours(hours) {
        return Err(format!(
            "the fork point {at} is older than the {hours} hours of history the database keeps"
        ));
    }
    Ok(())
}

/// The org's tenant and the environment's branch row, whichever exist.
pub(crate) async fn find_with_tenant(
    db: &DatabaseConnection,
    key: &EnvBranchKey,
) -> Result<(Option<oltp_tenants::Model>, Option<rows::Model>), DbErr> {
    let Some(tenant) = OltpTenants::find()
        .filter(oltp_tenants::Column::OrgId.eq(key.org_id))
        .one(db)
        .await?
    else {
        return Ok((None, None));
    };
    let row = EnvBranches::find()
        .filter(rows::Column::TenantRowId.eq(tenant.id))
        .filter(rows::Column::AppId.eq(key.app_id))
        .filter(rows::Column::EnvName.eq(key.env_name.as_str()))
        .one(db)
        .await?;
    Ok((Some(tenant), row))
}

/// The environment's branch row.
pub async fn find(
    db: &DatabaseConnection,
    key: &EnvBranchKey,
) -> Result<Option<rows::Model>, DbErr> {
    Ok(find_with_tenant(db, key).await?.1)
}

/// The environment's branch row for a caller that holds the app but not the
/// org. An app belongs to one org, so `(app, env)` names one row.
pub async fn find_for_app<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    env_name: &str,
) -> Result<Option<rows::Model>, DbErr> {
    EnvBranches::find()
        .filter(rows::Column::AppId.eq(app_id))
        .filter(rows::Column::EnvName.eq(env_name))
        .one(db)
        .await
}

/// Every branch row the app's environments hold, for a view that shows them
/// all: one query however many sandboxes the app has.
pub async fn list_for_app<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
) -> Result<Vec<rows::Model>, DbErr> {
    EnvBranches::find()
        .filter(rows::Column::AppId.eq(app_id))
        .all(db)
        .await
}

/// Whether the org has an OLTP tenant: without one there is nothing to branch.
pub async fn has_tenant<C: ConnectionTrait>(db: &C, org_id: Uuid) -> Result<bool, DbErr> {
    let tenants = OltpTenants::find()
        .filter(oltp_tenants::Column::OrgId.eq(org_id))
        .count(db)
        .await?;
    Ok(tenants > 0)
}

/// How many environment branches the org holds, in every status — a failed one
/// still occupies a provider branch.
pub async fn count_for_org<C: ConnectionTrait>(db: &C, org_id: Uuid) -> Result<u64, DbErr> {
    let Some(tenant) = OltpTenants::find()
        .filter(oltp_tenants::Column::OrgId.eq(org_id))
        .one(db)
        .await?
    else {
        return Ok(0);
    };
    EnvBranches::find()
        .filter(rows::Column::TenantRowId.eq(tenant.id))
        .count(db)
        .await
}

/// Up to `limit` environment branches no sandbox holds any more, oldest
/// first: no `app_environments` row for their `(app, env)`.
///
/// `app_environments` cascades from its app and its owner, and
/// `oltp_env_branches.app_id` deliberately has no foreign key, so an app or
/// user delete removes the sandbox without a teardown and leaves its branch —
/// billed at the provider (an `empty` one has no provider expiry) and counted
/// against [`MAX_ENV`] by [`count_for_org`] forever. The maintenance sweep
/// deletes each through `OltpProvisioner::delete_orphaned_env_branch`, which
/// reads [`is_orphaned`] again under the branch's lock.
pub async fn orphaned<C: ConnectionTrait>(db: &C, limit: u64) -> Result<Vec<EnvBranchKey>, DbErr> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT t.org_id, b.app_id, b.env_name FROM oltp_env_branches b \
               JOIN oltp_tenants t ON t.id = b.tenant_row_id \
              WHERE NOT EXISTS (SELECT 1 FROM app_environments e \
                                 WHERE e.app_id = b.app_id AND e.name = b.env_name) \
              ORDER BY b.created_at, b.id LIMIT $1",
            [limit.into()],
        ))
        .await?;
    rows.into_iter()
        .map(|row| {
            Ok(EnvBranchKey {
                org_id: row.try_get("", "org_id")?,
                app_id: row.try_get("", "app_id")?,
                env_name: row.try_get("", "env_name")?,
            })
        })
        .collect()
}

/// Whether no `app_environments` row holds `(app_id, env_name)`: the branch
/// is [`orphaned`]. A sandbox created again under the name holds it again.
pub async fn is_orphaned<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    env_name: &str,
) -> Result<bool, DbErr> {
    use entity::app_environments::{Column, Entity as AppEnvironments};
    let holders = AppEnvironments::find()
        .filter(Column::AppId.eq(app_id))
        .filter(Column::Name.eq(env_name))
        .count(db)
        .await?;
    Ok(holders == 0)
}

/// Move an environment's branch row to `to`, recording `error` (`None` clears
/// it). Unconditional: the callers are the one holding the branch's lock or the
/// migration apply that owns the `migrating` state, and a failure must be
/// recordable whatever the row last said. Needs no provisioner.
pub async fn mark_env_branch(
    db: &DatabaseConnection,
    row_id: Uuid,
    to: rows::EnvBranchStatus,
    error: Option<String>,
) -> Result<(), DbErr> {
    EnvBranches::update_many()
        .set(rows::ActiveModel {
            status: ActiveValue::Set(to),
            error: ActiveValue::Set(error),
            updated_at: ActiveValue::Set(Utc::now().into()),
            ..Default::default()
        })
        .filter(rows::Column::Id.eq(row_id))
        .exec(db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap()
    }

    #[test]
    fn the_provider_name_is_derived_and_distinct_per_environment() {
        let app = Uuid::parse_str("0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0").unwrap();
        let name = provider_name(app, "dev-a1");
        assert_eq!(name, "oxy-env-0b1c2d3e4f5061728394a5b6c7d8e9f0-dev-a1");
        assert_eq!(name, provider_name(app, "dev-a1"), "stable across retries");
        assert_ne!(name, provider_name(app, "dev-b2"));
        assert_ne!(name, provider_name(Uuid::new_v4(), "dev-a1"));
    }

    #[test]
    fn expiry_is_thirty_days_after_the_fork() {
        assert_eq!(expires_at(now()), now() + Duration::days(30));
        assert_eq!(EXPIRY_DAYS, 30);
    }

    #[test]
    fn a_fork_in_the_future_or_past_the_history_window_is_refused() {
        let hours = HISTORY_DEFAULT_HOURS;
        let future = check_fork_at_within(now() + Duration::minutes(2), now(), hours);
        assert!(future.unwrap_err().contains("future"));
        let old = check_fork_at_within(now() - Duration::hours(25), now(), hours);
        assert!(old.unwrap_err().contains("24 hours"));
        assert!(check_fork_at_within(now() - Duration::hours(23), now(), hours).is_ok());
        // Clock skew inside the slack is not "the future".
        assert!(check_fork_at_within(now() + Duration::seconds(30), now(), hours).is_ok());
    }

    #[test]
    fn the_cap_refusal_names_the_count_the_limit_and_the_way_out() {
        let said = cap_refusal(3, 3);
        assert_eq!(
            said,
            "this org has 3 sandbox OLTP branches, the most OXY_APP_ENV_OLTP_BRANCHES_MAX \
             allows (3); use --plane oltp=schema, the cheap mode, or delete a sandbox"
        );
    }

    #[test]
    fn the_cap_defaults_to_eight() {
        assert_eq!(MAX_DEFAULT, 8);
        assert_eq!(parse_max(None), 8);
        assert_eq!(parse_max(Some("not a number")), 8, "unparsable: default");
        assert_eq!(parse_max(Some(" 3 ")), 3);
    }
}
