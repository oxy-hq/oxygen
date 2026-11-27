//! The five routes a runner drives one attempt of a build's checks through.
//!
//! The runner is one `oxyc checks run <app-id> --build <row id> --attempt <n>
//! --environment <e> --report` on a GitHub Actions runner, signed in as the
//! platform checker (`X-API-Key: oxysvc_…`, role `app_checker`); a person with
//! non-production reach over the app may run the same command by hand.
//!
//! | Method + path (under `/api/customer-apps/{id}/builds/{build}/checks`) | What it does |
//! | --- | --- |
//! | `POST /claim` | names the runner on the attempt (the checker as it names itself, anyone else as the platform knows them), marks its open checks `running`, and decides what the platform knows already: a staging attempt's `validation` verdict and its check functions, started once; answers the job ([`claim`], [`job`]) |
//! | `POST /functions/settle` | reads the started functions and, once they are done, writes the `functions` verdict ([`settle`]) |
//! | `GET /stylesheets` | the build's `index.html` and CSS, for the theme guard (`super::design_system`) |
//! | `POST /report` | writes the runner's verdicts: `loads` and `design_system`, `smoke` and `changes` where a staging attempt has them, `production_smoke` for a production attempt ([`report`]) |
//! | `PUT /files/{name}` | keeps one file of the attempt's evidence in the build store: a screen's picture, a diff, a changed screen's text, the CSS digest (`super::files`) |
//!
//! **An attempt is named by its environment and its number.** `claim`,
//! `functions/settle` and `report` take `"environment"` (`staging` or
//! `production`) beside `"attempt"` in the body, a file in its query; absent,
//! it is staging ([`super::environment`]).
//!
//! **Mounted outside the staff console's layers** ([`routes`], merged into the
//! `/customer-apps` nest after `admin::apps::guarded`): the checker holds no
//! console standing, so those layers would refuse it. Behind the API's
//! authentication like every route there, and decided in each handler
//! (`super::admit`): the platform checker on the environment the attempt names
//! (`Action::AppCheck` on staging, `Action::AppCheckProduction` on
//! production) or a caller with non-production reach
//! (`Action::AppNonProduction`); never a publish token, never a sandbox agent
//! token. Anyone else gets the bare not-found an unknown build gets. FleetOk:
//! Postgres, the build store and queued tasks, no working copy.
//!
//! **A run by hand leaves a trace.** A caller who is not the platform checker
//! is named as the attempt's runner by the platform (`<who>, by hand`,
//! [`claim::Claimant::runner`]), and each report of theirs that writes a
//! verdict writes an audit row with it, `app.build_checks_reported_by_hand`
//! ([`report::ReportedBy`]).
//!
//! **These five are every write the checker has.** Each writes only
//! `app_build_checks` rows of the build named in the path and files under
//! that build's checks prefix, only while a row has no verdict, and a claim
//! queues only functions the build itself marks `"check": true`.
//!
//! Refusals are `{"code", "message"}` (`super::error`): `409 attempt_closed`,
//! `400 invalid_body`, `400 invalid_environment`, `400 invalid_runner`, `400
//! kind_not_reportable`, `400 invalid_status`, `400 invalid_detail`, `400
//! detail_too_large`, `413 stylesheets_too_large`, and a file's own
//! (`super::files`).

pub mod claim;
pub mod handlers;
pub mod job;
pub mod report;
pub mod settle;

use axum::Router;
use axum::routing::{get, post, put};
use entity::{app_build_checks, app_builds};
use oxy_app_core::AppState;
use sea_orm::DatabaseConnection;

use super::environment::CheckEnvironment;
use super::error::ChecksError;
use super::kind::CheckKind;
use super::stalled;
use super::store::{self, AttemptKey};

/// The runner routes, paths relative to `/api/customer-apps`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/{id}/builds/{build}/checks/claim", post(handlers::claim))
        .route(
            "/{id}/builds/{build}/checks/functions/settle",
            post(handlers::settle),
        )
        .route(
            "/{id}/builds/{build}/checks/stylesheets",
            get(handlers::stylesheets),
        )
        .route("/{id}/builds/{build}/checks/report", post(handlers::report))
        // A file's bytes are read by the handler itself, up to the limit a
        // claim names (`files::MAX_FILE_BYTES`), so a file past it is the
        // route's own `413 file_too_large`.
        .route(
            "/{id}/builds/{build}/checks/files/{name}",
            put(super::files::put),
        )
}

/// The attempt of `build`'s checks in `environment` a request names:
/// `attempt`, or the newest there when it names none. Not-found for a build
/// never checked there; whether a named attempt exists is its rows' to say.
pub(crate) async fn attempt_of(
    db: &DatabaseConnection,
    build: &app_builds::Model,
    environment: CheckEnvironment,
    attempt: Option<i32>,
) -> Result<AttemptKey, ChecksError> {
    let attempt = match attempt {
        Some(named) => named,
        None => store::latest_attempt(db, build.id, environment.name())
            .await?
            .ok_or(ChecksError::NotFound)?,
    };
    Ok(AttemptKey {
        build_pk: build.id,
        environment: environment.name().to_string(),
        attempt,
    })
}

/// The kinds of `rows` still without a verdict, in a build's list order.
pub(crate) fn open_kinds(rows: &[app_build_checks::Model]) -> Vec<String> {
    let mut open: Vec<&app_build_checks::Model> =
        rows.iter().filter(|row| stalled::is_open(row)).collect();
    open.sort_by(|a, b| CheckKind::list_key(&a.kind).cmp(&CheckKind::list_key(&b.kind)));
    open.into_iter().map(|row| row.kind.clone()).collect()
}
