//! Publish limits: how many apps an org may register, how often one publisher
//! may publish, and how many functions one bundle may declare.
//!
//! Publish is the registration path, so without a count a loop over slugs
//! registers unbounded apps; and each publish inflates up to 256 MiB, writes
//! every file to the store and walks the prefix in `gc_builds`, so without a
//! rate one publisher can keep a replica busy. Both refusals come before the
//! inflate. The defaults are far above any org's real use and are set per
//! deployment with `OXY_CUSTOMER_APPS_MAX_APPS_PER_ORG` and
//! `OXY_CUSTOMER_APPS_MAX_PUBLISHES_PER_HOUR`.
//!
//! The rate is counted in Postgres (`oxy_app_core::rate_limit`), so it is the
//! same budget whichever replica takes the publish; until 2026-11 it was a
//! per-process table and N replicas meant N budgets. The limiter fails open on
//! a database error, which a publish — the next step of which is a database
//! write — would surface on its own.

use std::time::Duration;

use entity::apps;
use oxy_app_core::rate_limit::{self, Decision};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use uuid::Uuid;

use super::{PublishError, PublishInput};

const DEFAULT_MAX_APPS_PER_ORG: u64 = 500;
const DEFAULT_MAX_PUBLISHES_PER_HOUR: u32 = 120;
const RATE_WINDOW: Duration = Duration::from_secs(3600);

/// The most functions one `oxy-app.json` may declare. Each is a row, an
/// artifact and, with a schedule or webhook, a registration on every publish.
pub(super) const MAX_FUNCTIONS_PER_BUNDLE: usize = 200;

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// Refuse a publish that would register a new app past the org's cap.
/// Publishing to an app that already exists is never counted.
pub(super) async fn refuse_new_app_past_cap(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<(), PublishError> {
    let limit = env_or(
        "OXY_CUSTOMER_APPS_MAX_APPS_PER_ORG",
        DEFAULT_MAX_APPS_PER_ORG,
    );
    let count = apps::Entity::find()
        .filter(apps::Column::OrgId.eq(org_id))
        .count(db)
        .await
        .map_err(|e| PublishError::Db(e.to_string()))?;
    if count >= limit {
        return Err(PublishError::AppLimit { limit });
    }
    Ok(())
}

/// Who a publish is counted against: the person, or the app a machine
/// (publish-token) publish is confined to. `None` for neither, which only an
/// in-process caller (seeding) produces; it is not limited.
fn publisher_key(input: &PublishInput) -> Option<Uuid> {
    input.published_by.or(input.machine_app_id)
}

/// Count this publish against its publisher's hourly budget in this org, and
/// refuse it once the budget is spent.
pub(super) async fn refuse_past_publish_rate(
    db: &DatabaseConnection,
    org_id: Uuid,
    input: &PublishInput,
) -> Result<(), PublishError> {
    let Some(publisher) = publisher_key(input) else {
        return Ok(());
    };
    let limit = env_or(
        "OXY_CUSTOMER_APPS_MAX_PUBLISHES_PER_HOUR",
        DEFAULT_MAX_PUBLISHES_PER_HOUR,
    );
    let key = format!("publish:{org_id}:{publisher}");
    match rate_limit::take(db, &key, u64::from(limit), RATE_WINDOW).await {
        Decision::Allowed => Ok(()),
        limited @ Decision::Limited { .. } => Err(PublishError::RateLimited {
            limit,
            retry_after_secs: limited.retry_after_secs(),
        }),
    }
}
