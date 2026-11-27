//! `POST …/checks/claim`: a runner takes one attempt.
//!
//! One transaction over the attempt's rows, locked, so two claims take turns
//! and a claim sent twice changes nothing the second time:
//!
//! 1. the runner is named on every row (`{label, url, started_at}`), in
//!    place of whatever the column held. **Only the platform checker names
//!    itself**: its label and link are kept as sent (a GitHub Actions run
//!    and its page). A claim by anyone else is a run by hand, and the
//!    platform names it from who called (`<who>, by hand`, no link, `by`),
//!    whatever label and link the request carried ([`Claimant::runner`]).
//!    **Every claim sets `started_at` to its own time**: a runner that is
//!    run again (GitHub's *Re-run jobs*, a run by hand after the first one
//!    died) is the one at work, so the thirty minutes a claimed attempt may
//!    stay open are measured from it, and a rerun cannot close its checks
//!    under it;
//! 2. every check still `queued` is `running`;
//! 3. the `validation` verdict is written: the platform knows it already;
//! 4. while staging still serves the build, its `"check": true` functions are
//!    started **once** (their run ids kept in the `functions` row's detail
//!    while it runs), or the check is `skipped` when it declares none;
//! 5. when staging no longer serves the build, `functions`, `loads`, `smoke`
//!    and `changes` are closed as `skipped`: each would judge whatever staging
//!    serves now. **Nobody judged them**, and the rows say so
//!    (`Outcome::not_checked`, `detail.not_checked: true`): such a skip is no
//!    pass, so the attempt never rolls up as `passed` and the promote checks
//!    gate counts each required one as unmet (`dto::not_checked`). The skip
//!    of step 4 is the check's own decision and stays a pass.
//!
//! **A production attempt** (`"environment": "production"`) has one check,
//! `production_smoke`, and none of steps 3 to 5: nothing is validated and no
//! function is started. When production no longer serves the build (it moved
//! on, or was rolled back), the check is closed as `skipped` and marked the
//! same way, for the same reason as step 5.
//!
//! **The answer is the runner's job** ([`ClaimAnswer`]): the attempt and its
//! environment, the page to open, and, beside phase 1's fields, the screens
//! the build declares (`screens`, `null` to discover them), what production
//! serves now for a staging attempt to compare with (`production`), what
//! production showed before the move for a production attempt to be judged
//! against (`baseline`), and what one attempt may keep (`limits`). See
//! [`super::job`] and `super::super::production::baseline`.
//!
//! The functions are queued through the pool while the transaction holds the
//! rows, and their ids are written in it. A commit that fails after they were
//! queued leaves them running unrecorded, and the next claim starts them
//! again: a check function may run twice then, never zero times.
//!
//! `409 attempt_closed` when every check already has a verdict.

use chrono::{DateTime, Utc};
use entity::{app_build_checks, app_builds, apps, organizations};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{DatabaseConnection, DatabaseTransaction, EntityTrait, TransactionTrait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::environment::CheckEnvironment;
use super::super::error::ChecksError;
use super::super::kind::{CheckKind, Outcome, cap};
use super::super::production::baseline::{self, Baseline};
use super::super::stalled::{self, Runner};
use super::super::store::{self, AttemptKey};
use super::super::{design_system, functions, settled, validation};
use super::job::{self, Limits, ProductionRef};
use crate::custom_apps_environments;

/// The longest runner label and link kept.
const MAX_LABEL_CHARS: usize = 120;
const MAX_URL_CHARS: usize = 500;

#[derive(Debug, Deserialize)]
pub struct ClaimRequest {
    /// The attempt to take; the newest when absent or null.
    #[serde(default)]
    pub attempt: Option<i32>,
    /// `staging` or `production`; staging when absent or null.
    #[serde(default)]
    pub environment: Option<String>,
    pub runner: RunnerRequest,
}

#[derive(Debug, Deserialize)]
pub struct RunnerRequest {
    pub label: String,
    /// The run's page; absent or null for a run by hand.
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AppRef {
    pub id: Uuid,
    /// The workspace the app was published from (`apps.project_id`): the
    /// `{id}` of the data-plane routes its page reads through.
    pub workspace_id: Uuid,
    pub org_slug: String,
    pub slug: String,
}

#[derive(Debug, Serialize)]
pub struct BuildRef {
    pub id: Uuid,
    pub build_id: String,
    pub sdk_version: Option<String>,
}

/// The job a claim answers: what to check, where, and what is left to do.
#[derive(Debug, Serialize)]
pub struct ClaimAnswer {
    pub app: AppRef,
    pub build: BuildRef,
    /// The attempt's environment: `staging` or `production`.
    pub environment: String,
    pub attempt: i32,
    /// Whether that environment still serves the build.
    pub serving: bool,
    /// The app's path URL on this deployment; it ends in `/`.
    pub url: String,
    /// How the platform names the credential that claimed: `app_checker` for
    /// the checker's service token, a person as every log names them.
    pub identity: String,
    pub sdk_floor: &'static str,
    /// The kinds still without a verdict after this claim.
    pub open: Vec<String>,
    /// The screens the build's manifest declares (`checks.screens`), as
    /// app-relative paths (`""` is the root); `null` when it declares none
    /// and the runner reads them off the root page.
    pub screens: Option<Vec<String>>,
    /// A staging attempt: the build production serves now, to compare with;
    /// `null` when it serves nothing or this same build, and always for a
    /// production attempt.
    pub production: Option<ProductionRef>,
    /// A production attempt: what the build production served right before
    /// the move showed; `null` when nothing is known of that build, and
    /// always for a staging attempt.
    pub baseline: Option<Baseline>,
    pub limits: Limits,
}

/// The runner a request names: a label, and a link that is an `http(s)` URL
/// (the console renders it as one) or nothing. Asked of every claim; kept
/// only for the platform checker's ([`Claimant::runner`]).
pub fn runner_named(request: &RunnerRequest) -> Result<(String, Option<String>), ChecksError> {
    let invalid = |why: &str| ChecksError::bad_request("invalid_runner", why);
    let label = request.label.trim();
    if label.is_empty() {
        return Err(invalid("runner.label is empty: say what runs the checks"));
    }
    let url = match request.url.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => {
            let web = url::Url::parse(raw)
                .ok()
                .is_some_and(|parsed| matches!(parsed.scheme(), "http" | "https"));
            if !web || raw.chars().count() > MAX_URL_CHARS {
                return Err(invalid("runner.url is not an http(s) URL"));
            }
            Some(raw.to_string())
        }
    };
    Ok((cap(label, MAX_LABEL_CHARS), url))
}

/// Who claims, and where.
#[derive(Clone, Copy, Debug)]
pub struct Claimant<'a> {
    /// The answer's `identity`: `app_checker` for the checker's service
    /// token, anyone else as every log names them.
    pub identity: &'a str,
    /// Whether the claim rode in on the platform checker's service token.
    pub checker: bool,
    /// This deployment's origin, for the app's URL.
    pub origin: &'a str,
}

impl Claimant<'_> {
    /// The runner as it is kept for a claim made `at`. The platform checker's
    /// is the one its request `named`: a GitHub Actions run and its page.
    /// Anyone else's is a run by hand, named by the platform from who called
    /// and never from what they sent, so it cannot pass for the checker's.
    pub fn runner(&self, named: (String, Option<String>), at: DateTime<Utc>) -> Runner {
        let started_at = at.fixed_offset();
        if self.checker {
            let (label, url) = named;
            return Runner {
                label,
                url,
                started_at,
                by: None,
            };
        }
        let who = cap(self.identity, MAX_LABEL_CHARS - BY_HAND.len());
        Runner {
            label: format!("{who}{BY_HAND}"),
            url: None,
            started_at,
            by: Some(who),
        }
    }
}

/// What follows the caller's name in the label of a run by hand.
const BY_HAND: &str = ", by hand";

/// Take `request`'s attempt of `build`'s checks in `environment` for the
/// runner it names.
pub async fn claim(
    db: &DatabaseConnection,
    (app, build): (&apps::Model, &app_builds::Model),
    request: &ClaimRequest,
    environment: CheckEnvironment,
    by: Claimant<'_>,
) -> Result<ClaimAnswer, ChecksError> {
    let named = runner_named(&request.runner)?;
    let key = super::attempt_of(db, build, environment, request.attempt).await?;
    let org = organizations::Entity::find_by_id(app.org_id)
        .one(db)
        .await?
        .ok_or(ChecksError::NotFound)?;
    let runner = by.runner(named, Utc::now());
    let serving = take(db, (app, build), &key, environment, &runner).await?;
    let rows = store::rows_of(db, &key).await?;
    tracing::info!(app_id = %app.id, build = %build.build_id, attempt = key.attempt,
        environment = environment.name(), runner = %runner.label,
        "build checks: attempt claimed");
    let (production, baseline) = match environment {
        CheckEnvironment::Staging => (job::production_of(db, app, build).await?, None),
        CheckEnvironment::Production => (None, baseline::of_build(db, build).await?),
    };
    Ok(ClaimAnswer {
        app: AppRef {
            id: app.id,
            workspace_id: app.project_id,
            org_slug: org.slug.clone(),
            slug: app.slug.clone(),
        },
        build: BuildRef {
            id: build.id,
            build_id: build.build_id.clone(),
            sdk_version: build.sdk_version.clone(),
        },
        url: app_url(by.origin, &org.slug, &app.slug),
        identity: by.identity.to_string(),
        environment: key.environment,
        attempt: key.attempt,
        serving,
        sdk_floor: design_system::SDK_FLOOR,
        open: super::open_kinds(&rows),
        screens: job::declared_screens(build.manifest_json.as_ref()),
        production,
        baseline,
        limits: job::LIMITS,
    })
}

/// The claim's one transaction over the attempt's locked rows (module docs):
/// name `runner` on them, mark the queued ones running, and decide what the
/// platform decides. Answers whether the attempt's environment still serves
/// the build.
async fn take(
    db: &DatabaseConnection,
    (app, build): (&apps::Model, &app_builds::Model),
    key: &AttemptKey,
    environment: CheckEnvironment,
    runner: &Runner,
) -> Result<bool, ChecksError> {
    let txn = db.begin().await?;
    let rows = store::lock_rows(&txn, key).await?;
    if rows.is_empty() {
        return Err(ChecksError::NotFound);
    }
    if !rows.iter().any(stalled::is_open) {
        return Err(ChecksError::conflict(
            "attempt_closed",
            format!(
                "every check of attempt {} already has a verdict",
                key.attempt
            ),
        ));
    }
    store::set_runner(&txn, key, runner).await?;
    store::mark_queued_running(&txn, key, runner.started_at.with_timezone(&Utc)).await?;
    let serving = match environment {
        CheckEnvironment::Staging => {
            decide_the_platforms(db, &txn, (app, build), key, &rows).await?
        }
        CheckEnvironment::Production => production_serves(&txn, (app, build), key).await?,
    };
    // A claim can be the write that settles an attempt (its last open check
    // was the platform's own): under the lock, so it is said once.
    let settled = store::settled_by(&txn, key, &rows).await?;
    txn.commit().await?;
    if settled {
        settled::on_staging_attempt_settled(db, app, build, key).await;
    }
    Ok(serving)
}

/// A production attempt's one decision at a claim: whether production still
/// serves the build. When it does not, `production_smoke` is closed as
/// `skipped`, judged by nobody: it would judge another build.
async fn production_serves(
    txn: &DatabaseTransaction,
    (app, build): (&apps::Model, &app_builds::Model),
    key: &AttemptKey,
) -> Result<bool, ChecksError> {
    let production = AppEnvironment::Production;
    let serving =
        custom_apps_environments::pointer(txn, app.id, &production).await? == Some(build.id);
    if !serving {
        let moved = Outcome::not_checked(format!(
            "production no longer serves build {}; it is checked again when it does",
            build.build_id
        ));
        store::finish(txn, key, CheckKind::ProductionSmoke, &moved).await?;
    }
    Ok(serving)
}

/// `<origin>/customer-apps/<org_slug>/<app_slug>/`.
pub fn app_url(origin: &str, org_slug: &str, app_slug: &str) -> String {
    let path = oxy_shared::utils::custom_app_url::build_pretty_url(org_slug, app_slug);
    format!("{}{path}", origin.trim_end_matches('/'))
}

/// What the platform decides at a claim, over the `rows` as they were locked:
/// the validation verdict, and the functions check's start or its skip; or,
/// once staging has moved on, that nobody judges what needs staging.
/// Answers whether staging still serves the build.
async fn decide_the_platforms(
    db: &DatabaseConnection,
    txn: &DatabaseTransaction,
    (app, build): (&apps::Model, &app_builds::Model),
    key: &AttemptKey,
    rows: &[app_build_checks::Model],
) -> Result<bool, ChecksError> {
    let open = |kind: CheckKind| {
        rows.iter()
            .find(|row| row.kind == kind.as_str() && stalled::is_open(row))
    };
    if open(CheckKind::Validation).is_some() {
        store::finish(txn, key, CheckKind::Validation, &validation::check(build)).await?;
    }
    let staging = AppEnvironment::Staging;
    let serving = custom_apps_environments::pointer(txn, app.id, &staging).await? == Some(build.id);
    if !serving {
        let moved = Outcome::not_checked(format!(
            "staging no longer serves build {}; run the checks again while it does",
            build.build_id
        ));
        for kind in [
            CheckKind::Functions,
            CheckKind::Loads,
            CheckKind::Smoke,
            CheckKind::Changes,
        ] {
            store::finish(txn, key, kind, &moved).await?;
        }
        return Ok(false);
    }
    let unstarted = open(CheckKind::Functions)
        .filter(|row| functions::pending_of(row.detail.as_ref()).is_none());
    if unstarted.is_some() {
        start_functions(db, txn, app.id, build.id, key).await?;
    }
    Ok(true)
}

/// Start the build's check functions, or skip the check when it has none.
async fn start_functions(
    db: &DatabaseConnection,
    txn: &DatabaseTransaction,
    app_id: Uuid,
    build_pk: Uuid,
    key: &AttemptKey,
) -> Result<(), ChecksError> {
    let names = functions::check_functions(txn, build_pk).await?;
    if names.is_empty() {
        let none =
            Outcome::skipped("the build declares no checks (no function has \"check\": true)");
        store::finish(txn, key, CheckKind::Functions, &none).await?;
        return Ok(());
    }
    let started = functions::start(db, app_id, &names).await;
    let pending = functions::pending_detail(&started);
    store::keep_while_running(txn, key, CheckKind::Functions, pending).await?;
    Ok(())
}

#[cfg(test)]
#[path = "claim_tests.rs"]
mod tests;
