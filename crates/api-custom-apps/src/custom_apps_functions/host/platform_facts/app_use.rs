//! Everything the facts take from app page views: each app's last open by
//! anyone, the customer's own last open, how many of the customer's people
//! opened it in `last7d`, and each org's views, viewers and active people per
//! window.
//!
//! The views come from `oxy_custom_apps::usage_reads`, which answers each app
//! from its workspace's usage store (its Airhouse once the workspace's usage
//! history is copied there, Postgres until then). The counting is done here,
//! from the people themselves, because a count of distinct people per org
//! cannot be added up from per-store counts: a person who opened two apps is
//! one person.
//!
//! All of it is production views only, and every customer read leaves out
//! staff (members of an internal org) exactly as every `*People` count does.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use oxy_custom_apps::usage_reads::{self, Customers, ReportApp, TimeWindow};
use sea_orm::{DatabaseConnection, DbErr, FromQueryResult};
use uuid::Uuid;

use super::Windows;
use super::queries::{AppRow, WindowRow};

/// An instant per app (a last view).
#[derive(Clone, Debug, FromQueryResult)]
pub(super) struct AppAtRow {
    pub app_id: Uuid,
    pub at: DateTime<Utc>,
}

/// A count per app (its people in `last7d`).
#[derive(Clone, Debug, FromQueryResult)]
pub(super) struct AppCountRow {
    pub app_id: Uuid,
    pub people: i64,
}

/// One non-staff person with a thread in one org's window.
#[derive(Clone, Debug, FromQueryResult)]
pub(super) struct OrgPersonRow {
    pub org_id: Uuid,
    pub idx: i64,
    pub user_id: Uuid,
}

/// What the views add to the facts.
#[derive(Default)]
pub(super) struct ViewFacts {
    /// Each app's newest production view, staff's included.
    pub last_viewed: Vec<AppAtRow>,
    /// Each app's newest production view by a non-staff person, inside the
    /// horizon.
    pub customer_last_viewed: Vec<AppAtRow>,
    /// Each app's distinct non-staff production viewers in `last7d`.
    pub people7d: Vec<AppCountRow>,
    /// Each org's views and distinct non-staff viewers per window.
    pub views: Vec<WindowRow>,
    /// Each org's distinct non-staff people per window with a view or a
    /// thread: `thread_people` and the viewers, counted once.
    pub active: Vec<WindowRow>,
}

/// Read the views of `apps` and count them. `thread_people` is each org's
/// non-staff people with a thread, per window, read with the other sources.
pub(super) async fn read(
    db: &DatabaseConnection,
    apps: &[AppRow],
    staff: &[Uuid],
    windows: &Windows,
    activity_since: DateTime<Utc>,
    thread_people: &[OrgPersonRow],
) -> Result<ViewFacts, DbErr> {
    let targets: Vec<ReportApp> = apps
        .iter()
        .map(|a| ReportApp {
            app_id: a.id,
            workspace_id: a.workspace_id,
        })
        .collect();
    let spans: Vec<TimeWindow> = windows
        .starts
        .iter()
        .copied()
        .zip(windows.ends.iter().copied())
        .collect();
    let customers = Customers {
        staff,
        since: activity_since,
    };
    let (last, viewers) = futures::future::try_join(
        usage_reads::last_views(db, &targets, Some(customers)),
        usage_reads::viewers_by_window(db, &targets, &spans),
    )
    .await?;
    let org_of: HashMap<Uuid, Uuid> = apps.iter().map(|a| (a.id, a.org_id)).collect();
    let staff: HashSet<Uuid> = staff.iter().copied().collect();
    let last7d = windows.len().saturating_sub(1);

    let mut out = ViewFacts::default();
    for views in last.values() {
        out.last_viewed.push(AppAtRow {
            app_id: views.app_id,
            at: views.anyone,
        });
        if let Some(at) = views.customer {
            out.customer_last_viewed.push(AppAtRow {
                app_id: views.app_id,
                at,
            });
        }
    }

    let mut people7d: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();
    let mut org_views: HashMap<(Uuid, usize), (i64, HashSet<Uuid>)> = HashMap::new();
    let mut active: HashMap<(Uuid, usize), HashSet<Uuid>> = HashMap::new();
    for v in &viewers {
        let Some(org_id) = org_of.get(&v.app_id).copied() else {
            continue;
        };
        let customer = !staff.contains(&v.user_id);
        let (count, people) = org_views.entry((org_id, v.window)).or_default();
        *count += v.views;
        if customer {
            people.insert(v.user_id);
            active
                .entry((org_id, v.window))
                .or_default()
                .insert(v.user_id);
            if v.window == last7d {
                people7d.entry(v.app_id).or_default().insert(v.user_id);
            }
        }
    }
    for p in thread_people {
        if let Ok(window) = usize::try_from(p.idx) {
            active
                .entry((p.org_id, window))
                .or_default()
                .insert(p.user_id);
        }
    }

    out.people7d = people7d
        .into_iter()
        .map(|(app_id, people)| AppCountRow {
            app_id,
            people: people.len() as i64,
        })
        .collect();
    out.views = org_views
        .into_iter()
        .map(|((org_id, window), (events, people))| WindowRow {
            org_id,
            idx: window as i64,
            events,
            failures: 0,
            people: people.len() as i64,
        })
        .collect();
    out.active = active
        .into_iter()
        .map(|((org_id, window), people)| WindowRow {
            org_id,
            idx: window as i64,
            events: 0,
            failures: 0,
            people: people.len() as i64,
        })
        .collect();
    Ok(out)
}
