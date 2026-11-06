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
//! The rate is counted per process, like the event-ingest and function
//! limiters: behind N replicas a publisher gets up to N times the budget. It
//! bounds the load one replica takes, which is what it is for.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use entity::apps;
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
pub(super) fn refuse_past_publish_rate(
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
    match take((org_id, publisher), limit, Instant::now()) {
        Ok(()) => Ok(()),
        Err(retry_after) => Err(PublishError::RateLimited {
            limit,
            retry_after_secs: retry_after.as_secs().max(1),
        }),
    }
}

struct Window {
    start: Instant,
    count: u32,
}

fn windows() -> &'static Mutex<HashMap<(Uuid, Uuid), Window>> {
    static TABLE: std::sync::OnceLock<Mutex<HashMap<(Uuid, Uuid), Window>>> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Take one publish from `key`'s fixed window, or say how long until the
/// window resets. Expired windows are evicted on the way through, so the
/// table holds only publishers active in the last hour.
fn take(key: (Uuid, Uuid), limit: u32, now: Instant) -> Result<(), Duration> {
    let mut table = windows().lock().unwrap_or_else(|e| e.into_inner());
    table.retain(|_, w| now.duration_since(w.start) < RATE_WINDOW);
    let window = table.entry(key).or_insert(Window {
        start: now,
        count: 0,
    });
    if window.count >= limit {
        return Err(RATE_WINDOW.saturating_sub(now.duration_since(window.start)));
    }
    window.count += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_publisher_is_refused_once_the_window_is_spent_and_only_that_publisher() {
        let (org, me, other) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let start = Instant::now();
        for _ in 0..3 {
            assert!(take((org, me), 3, start).is_ok());
        }
        let wait = take((org, me), 3, start + Duration::from_secs(600)).unwrap_err();
        assert_eq!(wait, Duration::from_secs(3000), "the rest of the hour");
        assert!(
            take((org, other), 3, start).is_ok(),
            "another publisher's own budget"
        );
        assert!(
            take((Uuid::new_v4(), me), 3, start).is_ok(),
            "another org's own budget"
        );
    }

    #[test]
    fn the_window_resets_after_an_hour() {
        let key = (Uuid::new_v4(), Uuid::new_v4());
        let start = Instant::now();
        assert!(take(key, 1, start).is_ok());
        assert!(take(key, 1, start).is_err());
        assert!(take(key, 1, start + RATE_WINDOW).is_ok());
    }
}
