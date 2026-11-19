//! The `availability` field on the `/customer-apps` list rows, so the console
//! takes it from the list instead of asking once per app. (The per-app
//! `GET /customer-apps/{org}/{app}/availability` route that used to answer it
//! was retired.)
//!
//! ## How the reading is built
//!
//! [`AvailabilityReading::from_windows`] turns an app's windows and open-builds
//! line into the reading; this module calls it with the windows
//! [`super::verdict::measure`] already read for the whole page, from
//! `get_fleet_availability`. `get_app_availability`, the per-app read, shares
//! its predicates.
//!
//! ## What it will not say
//!
//! The field is present only for a published app that was measured. A draft, a
//! deployment with capture off, a failed query and a store that did not answer
//! within [`BUDGET`] all leave it absent. Absent means "not read here" — a
//! consumer says nothing, and never reads it as
//! `healthy`. `no_opinion` inside the field is a measured "not enough traffic".
//!
//! ## Cost
//!
//! A per-app availability read is seven ClickHouse round trips for one app:
//! one per window in `ALERT_WINDOWS_MINUTES` (six) and one for open builds. It
//! is not an in-memory read, so the list never does it per app.
//!
//! On the list the windows come from `verdict::measure`, which was already
//! reading them to compute `verdict` and keeps them in its cache: the field adds
//! no availability read. Open builds are the one added read: one `app_environments`
//! query and one grouped ClickHouse query for the apps on the page that are not
//! cached, none when they all are. Both caches are `TtlLru`s with a 30 second
//! [`TTL`](super::verdict) and 4096 entries, so a console that refetches on every
//! window focus costs one set of reads per 30 seconds per replica rather than one
//! per focus. Past 4096 apps the least recently read entry goes first.

use std::collections::HashMap;
use std::future::Future;
use std::sync::LazyLock;
use std::time::Duration;

use oxy::database::client::establish_connection;
use uuid::Uuid;

use super::verdict::{AppRef, BUDGET, ListVerdict, Measured, measure};
use crate::custom_apps_availability::AvailabilityReading;
use crate::custom_apps_cache::{CACHE_MAX_ENTRIES, TtlLru};
use crate::custom_apps_open_builds::{self as open_builds, OpenBuilds};

/// Same freshness as the verdict cache beside it: the two describe the same
/// recent traffic, and a reading older than its verdict would contradict it.
const OPEN_BUILDS_TTL: Duration = Duration::from_secs(30);

static OPEN_BUILDS: LazyLock<TtlLru<Uuid, OpenBuilds>> =
    LazyLock::new(|| TtlLru::new(CACHE_MAX_ENTRIES, OPEN_BUILDS_TTL));

/// The build production serves, per app, for the apps asked about. `None` when
/// the read failed: the caller reports open builds as not measured and does not
/// remember it.
pub(super) async fn production_builds(ids: Vec<Uuid>) -> Option<HashMap<Uuid, Option<Uuid>>> {
    let db = establish_connection()
        .await
        .map_err(|e| tracing::warn!("list availability: DB connection failed: {e}"))
        .ok()?;
    crate::custom_apps_environments::fixed_pointers_batch(&db, &ids)
        .await
        .map_err(|e| tracing::warn!("list availability: production pointers lookup failed: {e}"))
        .ok()
        .map(|pointers| {
            pointers
                .into_iter()
                .map(|(id, p)| (id, p.production))
                .collect()
        })
}

/// Each app's `verdict` and `availability` for a list page. Every app asked
/// about is in the result.
///
/// `live_builds` is called only with the apps whose open builds are not cached,
/// and not at all when there are none.
pub(super) async fn rows_for<F, Fut>(
    apps: &[AppRef],
    live_builds: F,
) -> HashMap<Uuid, (ListVerdict, Option<AvailabilityReading>)>
where
    F: FnOnce(Vec<Uuid>) -> Fut,
    Fut: Future<Output = Option<HashMap<Uuid, Option<Uuid>>>>,
{
    let measured = measure(apps).await;

    // Only apps whose windows were read can carry a reading.
    let readable: Vec<&AppRef> = apps
        .iter()
        .filter(|a| {
            matches!(
                measured.get(&a.id),
                Some(Some(Measured {
                    windows: Some(_),
                    ..
                }))
            )
        })
        .collect();
    let mut open = open_builds_for(&readable, live_builds).await;

    apps.iter()
        .map(|app| {
            let (verdict, reading) = match measured.get(&app.id) {
                None | Some(None) => (ListVerdict::Draft, None),
                Some(Some(m)) => {
                    let reading = m.windows.as_deref().map(|windows| {
                        let line = open
                            .remove(&app.id)
                            .unwrap_or_else(OpenBuilds::not_measured);
                        AvailabilityReading::from_windows(windows, line)
                    });
                    (m.health.into(), reading)
                }
            };
            (app.id, (verdict, reading))
        })
        .collect()
}

/// The open-builds line for each of `apps`, from the cache or one grouped read.
async fn open_builds_for<F, Fut>(apps: &[&AppRef], live_builds: F) -> HashMap<Uuid, OpenBuilds>
where
    F: FnOnce(Vec<Uuid>) -> Fut,
    Fut: Future<Output = Option<HashMap<Uuid, Option<Uuid>>>>,
{
    let mut out = HashMap::with_capacity(apps.len());
    let mut missing: Vec<&AppRef> = Vec::new();
    for app in apps {
        match OPEN_BUILDS.get(&app.id) {
            Some(line) => {
                out.insert(app.id, line);
            }
            None => missing.push(app),
        }
    }
    if missing.is_empty() {
        return out;
    }

    let Some(live) = live_builds(missing.iter().map(|a| a.id).collect()).await else {
        // Not remembered: the pointers were not read, so "not measured" here is
        // about this request and says nothing about the app.
        return out;
    };
    let targets: Vec<open_builds::OpenBuildsTarget> = missing
        .iter()
        .map(|a| (a.org_id, a.id, live.get(&a.id).copied().flatten()))
        .collect();
    match tokio::time::timeout(BUDGET, open_builds::for_apps(&targets)).await {
        Ok(lines) => {
            for (id, line) in lines {
                OPEN_BUILDS.insert(id, line.clone());
                out.insert(id, line);
            }
        }
        Err(_) => tracing::warn!(
            "list availability: open builds did not answer in {BUDGET:?}; reporting not measured"
        ),
    }
    out
}

#[cfg(test)]
#[path = "list_availability_tests.rs"]
mod tests;
