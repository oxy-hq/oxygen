//! The demo fixture: `pulse-demo` ("Pulse (demo fixture)"), a custom app with
//! Oxy Functions and a seven-day release history, in the Local org's demo
//! workspace. `oxy dev seed --fixtures` builds it; `just up` passes the flag. It
//! feeds the console's release timeline, Issues and function views with data
//! a fresh dev box otherwise lacks (`oxy-starter` has no functions).
//!
//! **What is real and what is seeded.**
//!
//! - Real: the five builds, their publishes and the four promotes — the
//!   production code paths, called in this process (`releases`); the schedules
//!   the promotes register; the audit rows; and the runs queued at the end
//!   (`RUNS`), which the server's worker executes on production's build.
//! - Seeded: the timestamps of the release history (`calendar`), and the
//!   traffic before the seed — invocation rows (`history`) and Web Vitals
//!   (`vitals`). No real path can make either happen in the past.
//!
//! **The end state** is the bad release live: r1, r2 and r3 promoted in turn,
//! r4 promoted six hours ago with a `store` that throws on a stale store id
//! and runs several times slower, and r5 — the fix — published to staging. The
//! demo is to roll back from r4 and then promote r5.
//!
//! **No OLTP.** The fixture never provisions a per-org OLTP database: doing so
//! from a seed every developer runs would change how the demo workspace's own
//! compile settles its `schemas/`, on every later `just up`. It prints the
//! command that would.
//!
//! **Idempotent.** The app's id is derived from the org (`fixture_app_id`), so
//! every path proves the row is the fixture's before touching it, and a
//! developer's own `local/pulse-demo` is left alone. A fixture whose five
//! builds exist and whose last-written row (the sentinel, `vitals::
//! sentinel_event_id`) is there is *present* and nothing is done, whatever the
//! presenter has since moved; anything less — a seed that stopped part way —
//! is removed and rebuilt.
//!
//! The bundle is `customer-apps/examples/pulse-demo/` — outside the
//! `./examples` workspace the compile walks, and reviewable as plain files.

mod bundle;
mod calendar;
mod clear;
mod history;
mod releases;
mod runs;
mod vitals;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use airhouse::LOCAL_ORG_ID;
use chrono::Utc;
use entity::{app_builds, apps, custom_app_event, org_members, organizations, users, workspaces};
use oxy::theme::StyledText;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder,
};
use uuid::Uuid;

use crate::custom_apps_cache;

pub use bundle::{RELEASES, Release, releases as bundle_releases};
pub use calendar::{Calendar, LIVE, Stretch};
pub use history::{R4_STORE_ERROR, Seeded, invocations as seeded_invocations};
pub use runs::QUEUED_RUNS;
pub use vitals::{MIN_SESSIONS, PageLoad, SCREENS, VITALS_KEYS, page_loads};

/// The fixture's slug.
pub const APP_SLUG: &str = "pulse-demo";
const APP_NAME: &str = "Pulse (demo fixture)";

/// The fixture's app id in `org_id`.
pub fn fixture_app_id(org_id: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("oxy.app-fixture.{org_id}.{APP_SLUG}").as_bytes(),
    )
}

/// A step that failed, and why.
#[derive(Debug)]
pub struct FixtureError(String);

impl FixtureError {
    fn step(step: impl std::fmt::Display, cause: impl std::fmt::Display) -> Self {
        Self(format!("{step}: {cause}"))
    }
}

impl std::fmt::Display for FixtureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<DbErr> for FixtureError {
    fn from(e: DbErr) -> Self {
        Self(format!("database: {e}"))
    }
}

/// What one run of the fixture did.
#[derive(Debug)]
pub enum Outcome {
    /// A gate said no; the reason.
    Skipped(String),
    /// `local/pulse-demo` exists with an id this seed did not give it.
    Foreign,
    /// Already built; nothing was done.
    Present,
    Built(Built),
}

#[derive(Debug)]
pub struct Built {
    pub app_id: Uuid,
    /// An earlier, partial or outdated fixture was removed first.
    pub rebuilt: bool,
    /// `app_builds.id` per release, r1 first.
    pub builds: Vec<Uuid>,
    pub invocations: usize,
    pub events: usize,
    /// The real runs queued, or why none were.
    pub queued: Result<usize, String>,
}

/// `oxy dev seed --fixtures`: build the fixture unless it is present, and say
/// what happened. Warns instead of failing, like the seed's other extras.
pub async fn seed_demo_fixture(conn: &DatabaseConnection, workspace_id: Uuid, compiled: bool) {
    match seed(conn, workspace_id, compiled).await {
        Ok(outcome) => runs::print_outcome(&outcome),
        Err(e) => println!(
            "{} pulse-demo fixture not seeded: {e}\n  Re-run `oxy dev seed --fixtures` once the cause is fixed.",
            "⚠️".warning()
        ),
    }
}

/// [`seed_demo_fixture`] without the printing. `compiled` says the demo
/// workspace compiled, which the queued runs need (`ctx.query`).
pub async fn seed(
    conn: &DatabaseConnection,
    workspace_id: Uuid,
    compiled: bool,
) -> Result<Outcome, FixtureError> {
    if !oxy_app::surface::seed::is_local_db() {
        return Ok(Outcome::Skipped(
            "OXY_DATABASE_URL does not look local".into(),
        ));
    }
    let Some(org) = organizations::Entity::find_by_id(LOCAL_ORG_ID)
        .one(conn)
        .await?
    else {
        return Ok(Outcome::Skipped("there is no Local org".into()));
    };
    if workspaces::Entity::find_by_id(workspace_id)
        .one(conn)
        .await?
        .is_none()
    {
        return Ok(Outcome::Skipped("the demo workspace does not exist".into()));
    }
    let Some(operator) = operator(conn).await? else {
        return Ok(Outcome::Skipped(
            "the seeded App Operator does not exist".into(),
        ));
    };
    let app_id = fixture_app_id(org.id);
    let releases = bundle::releases();
    let rebuilt = match state(conn, org.id, app_id, &releases).await? {
        State::Foreign => return Ok(Outcome::Foreign),
        State::Present => return Ok(Outcome::Present),
        State::Stale(app) => clear::remove(conn, &app).await?,
        State::Absent => false,
    };
    let fixture = Fixture {
        conn,
        org: &org,
        workspace_id,
        operator: &operator,
        app_id,
    };
    let mut built = fixture.build(&releases, compiled).await?;
    built.rebuilt = rebuilt;
    Ok(Outcome::Built(built))
}

/// `oxy dev seed --clear`'s half: remove the fixture if this seed built it.
/// Answers how many apps were removed (0 or 1).
pub async fn clear_fixture(conn: &DatabaseConnection) -> Result<u64, FixtureError> {
    let app_id = fixture_app_id(LOCAL_ORG_ID);
    let Some(app) = apps::Entity::find_by_id(app_id).one(conn).await? else {
        return Ok(0);
    };
    if app.slug != APP_SLUG || app.org_id != LOCAL_ORG_ID {
        return Ok(0);
    }
    Ok(u64::from(clear::remove(conn, &app).await?))
}

enum State {
    Absent,
    Foreign,
    Present,
    /// The fixture's row, partial or out of date.
    Stale(Box<apps::Model>),
}

/// Present: the row carries the derived id, exactly the five builds of the
/// embedded bundle exist, and the sentinel — the last row a build writes — is
/// there. A seed interrupted anywhere before it is `Stale`.
async fn state(
    conn: &DatabaseConnection,
    org_id: Uuid,
    app_id: Uuid,
    releases: &[Release],
) -> Result<State, DbErr> {
    let Some(app) = apps::Entity::find()
        .filter(apps::Column::OrgId.eq(org_id))
        .filter(apps::Column::Slug.eq(APP_SLUG))
        .one(conn)
        .await?
    else {
        return Ok(State::Absent);
    };
    if app.id != app_id {
        return Ok(State::Foreign);
    }
    let builds: HashMap<String, Uuid> = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .all(conn)
        .await?
        .into_iter()
        .map(|b| (b.build_id, b.id))
        .collect();
    let all =
        builds.len() == releases.len() && releases.iter().all(|r| builds.contains_key(&r.build_id));
    // Written last (`Fixture::build`): present only when the whole history is.
    let complete = custom_app_event::Entity::find_by_id(vitals::sentinel_event_id(app_id))
        .one(conn)
        .await?
        .is_some_and(|e| e.app_id == app_id);
    Ok(if all && complete {
        State::Present
    } else {
        State::Stale(Box::new(app))
    })
}

struct Fixture<'a> {
    conn: &'a DatabaseConnection,
    org: &'a organizations::Model,
    workspace_id: Uuid,
    operator: &'a AuthenticatedUser,
    app_id: Uuid,
}

impl Fixture<'_> {
    async fn build(&self, releases: &[Release], compiled: bool) -> Result<Built, FixtureError> {
        let cal = Calendar::at(Utc::now());
        self.insert_app_row().await?;
        let builds = releases::run(
            self.conn,
            self.operator,
            self.org,
            self.workspace_id,
            self.app_id,
            releases,
        )
        .await?;
        calendar::redate(self.conn, self.app_id, &cal, &builds).await?;
        // The re-date rewrote the app row's stamps after the publishes cached
        // it. Process-local, so the server's own cache still expires by TTL.
        custom_apps_cache::invalidate_app_resolution_cache();
        let (viewer, email) = viewer(self.conn, self.org.id, self.operator).await?;
        let rows = history::invocations(self.app_id, &cal, viewer);
        let invocations =
            history::write_invocations(self.conn, self.app_id, &builds, &rows).await?;
        let queued = if compiled {
            runs::queue_real_runs(self.conn, self.app_id).await
        } else {
            Err("the demo workspace was not compiled, and every function queries it".into())
        };
        // LAST: its final row is the sentinel `state` reads, so a seed that
        // stops anywhere before here is rebuilt by the next one.
        let loads = vitals::page_loads(self.app_id, &cal, &builds);
        let events =
            vitals::write_events(self.conn, self.app_id, (viewer, &email), &builds, &loads).await?;
        Ok(Built {
            app_id: self.app_id,
            rebuilt: false,
            builds,
            invocations,
            events,
            queued,
        })
    }

    /// The row the first publish then finds by `(org, slug)` and keeps: a
    /// publish would mint a random id, and the derived one is what proves the
    /// row is the fixture's.
    async fn insert_app_row(&self) -> Result<(), DbErr> {
        let now = Utc::now().fixed_offset();
        apps::ActiveModel {
            id: Set(self.app_id),
            slug: Set(APP_SLUG.to_string()),
            name: Set(APP_NAME.to_string()),
            org_id: Set(self.org.id),
            project_id: Set(self.workspace_id),
            branch: Set("main".to_string()),
            source_repo: Set("oxy-hq/oxygen".to_string()),
            status: Set("created".to_string()),
            source_type: Set("s3".to_string()),
            source_config: Set(serde_json::json!({})),
            repo_path: Set(Some(format!("{}/{APP_SLUG}", self.org.slug))),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(self.conn)
        .await
        .map(|_| ())
    }
}

/// The seeded unscoped App Operator: the fixture's publisher and promoter.
async fn operator(conn: &DatabaseConnection) -> Result<Option<AuthenticatedUser>, DbErr> {
    let Some(email) = oxy_app::surface::seed::unscoped_app_operator_email() else {
        return Ok(None);
    };
    Ok(users::Entity::find()
        .filter(users::Column::Email.eq(email))
        .one(conn)
        .await?
        .map(AuthenticatedUser::from))
}

/// Who the seeded route calls and page loads belong to: the Local org's
/// first member (its Owner when there is one), else the operator.
async fn viewer(
    conn: &DatabaseConnection,
    org_id: Uuid,
    operator: &AuthenticatedUser,
) -> Result<(Uuid, String), DbErr> {
    let members = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .order_by_asc(org_members::Column::CreatedAt)
        .all(conn)
        .await?;
    let first = members
        .iter()
        .find(|m| m.role == org_members::OrgRole::Owner)
        .or(members.first());
    if let Some(member) = first
        && let Some(user) = users::Entity::find_by_id(member.user_id).one(conn).await?
        && let Some(email) = user.email
    {
        return Ok((user.id, email));
    }
    let email = operator.email.clone().unwrap_or_default();
    Ok((operator.id, email))
}
