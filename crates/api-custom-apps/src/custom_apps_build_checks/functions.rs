//! The `functions` check: every function the build marks `"check": true`, run
//! on staging through the server path `oxyc checks run` uses (`POST
//! …/functions/{name}/runs?environment=staging` lands in
//! `custom_apps_functions::check_run::trigger`). A run passes when it is
//! `done` and its answer is not an object whose `ok` is `false`: `oxyc checks
//! run`'s rule, word for word.
//!
//! The platform runs them, never the checker. A runner's claim [`start`]s
//! them once and keeps their run ids in the row's detail ([`pending_detail`])
//! while the row is `running`; each `…/checks/functions/settle` call then
//! reads those runs ([`readings`]) and, once every one is terminal or the
//! deadline has passed, writes the verdict ([`settle`]).
//!
//! The runs are queued tasks like any other check run: the worker resolves
//! staging's build when it runs one, which is why a claim starts them only
//! while staging serves the build being checked.
//!
//! **Each run is held to the build it ran.** Staging can move between the
//! claim and the worker (publish A, claim A, publish B, the worker runs B's
//! function), so a run is a verdict on the build being checked only when the
//! invocation it wrote names that build ([`judges`]). One that ran another
//! build, or ended `done` without saying which, is `not_this_build`: never a
//! pass, and unless a run of this build failed the check is closed as judged
//! by nobody (`Outcome::not_checked`), which satisfies no gate.

use std::time::Duration;

use entity::app_functions;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::kind::{CheckStatus, Finding, MAX_MESSAGE_CHARS, Outcome, Severity, cap};
use crate::admin::apps::function_run::{PlatformRunReading, read_for_platform};
use crate::custom_apps_functions::FunctionJobTrigger;
use crate::custom_apps_functions::check_run::{self, Asked, manifest_marks_check};

/// How long every run of one attempt may take, all of them together, from
/// the moment the check started.
pub const FUNCTIONS_DEADLINE: Duration = Duration::from_secs(300);

const TERMINAL: [&str; 4] = ["done", "failed", "cancelled", "timed_out"];

/// The key of [`pending_detail`] in a running row's detail.
const PENDING_KEY: &str = "pending";

/// One function's result, as `detail.functions` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FunctionResult {
    pub name: String,
    pub run_id: Option<String>,
    /// The run's terminal status, `timed_out` past the deadline, `refused`
    /// when it could not be queued, or [`NOT_THIS_BUILD`].
    pub status: String,
    pub passed: bool,
    pub error: Option<String>,
    /// How long this function itself ran, as its invocation recorded it;
    /// `None` for a run that never ran one or had not finished. Never how
    /// long the check had been open when it settled.
    pub duration_ms: Option<u64>,
    pub invocation_id: Option<Uuid>,
    /// The build the run's invocation ran; `None` when it wrote none.
    pub ran_build: Option<Uuid>,
}

/// The status of a run that is no verdict on the build being checked: it ran
/// another build, or does not say which build it ran.
pub const NOT_THIS_BUILD: &str = "not_this_build";

/// One check function a claim started, or could not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    pub name: String,
    /// The queued run; `None` when it could not be queued.
    pub run_id: Option<String>,
    /// Why it could not be queued.
    pub error: Option<String>,
}

/// The names of the build's `"check": true` functions, sorted.
pub async fn check_functions<C: ConnectionTrait>(
    c: &C,
    build_pk: Uuid,
) -> Result<Vec<String>, sea_orm::DbErr> {
    let rows = app_functions::Entity::find()
        .filter(app_functions::Column::BuildId.eq(build_pk))
        .order_by_asc(app_functions::Column::Name)
        .all(c)
        .await?;
    Ok(rows
        .into_iter()
        .filter(|f| manifest_marks_check(f.manifest_json.as_ref()))
        .map(|f| f.name)
        .collect())
}

async fn trigger(db: &DatabaseConnection, app_id: Uuid, name: &str) -> Result<String, String> {
    let asked = Asked {
        trigger: FunctionJobTrigger::Manual,
        environment: &AppEnvironment::Staging,
        credential_token_id: None,
    };
    check_run::trigger(db, app_id, name, None, asked)
        .await
        .map_err(|e| e.to_string())
}

/// Queue one run of each of `names` on staging.
pub async fn start(db: &DatabaseConnection, app_id: Uuid, names: &[String]) -> Vec<Started> {
    let mut started = Vec::with_capacity(names.len());
    for name in names {
        let queued = trigger(db, app_id, name).await;
        started.push(Started {
            name: name.clone(),
            run_id: queued.as_ref().ok().cloned(),
            error: queued.err().map(|why| cap(&why, MAX_MESSAGE_CHARS)),
        });
    }
    started
}

/// The detail of a `functions` row while its runs are going: the runs to
/// read back, and no findings yet.
pub fn pending_detail(started: &[Started]) -> Value {
    json!({ "findings": [], PENDING_KEY: started })
}

/// The runs a running row's detail holds; `None` when it holds none (the
/// check has not started its functions).
pub fn pending_of(detail: Option<&Value>) -> Option<Vec<Started>> {
    serde_json::from_value(detail?.get(PENDING_KEY)?.clone()).ok()
}

/// Each started run as it stands now, in `started`'s order: `None` for one
/// that was never queued, does not exist, or could not be read this time.
pub(crate) async fn readings(
    db: &DatabaseConnection,
    started: &[Started],
) -> Vec<Option<PlatformRunReading>> {
    let mut read = Vec::with_capacity(started.len());
    for function in started {
        let Some(run_id) = function.run_id.as_deref() else {
            read.push(None);
            continue;
        };
        match read_for_platform(db, run_id).await {
            Ok(reading) => read.push(reading),
            Err(e) => {
                tracing::warn!(%run_id, error = %e, "build checks: a run could not be read");
                read.push(None);
            }
        }
    }
    read
}

/// Where the check stands, given each run's reading.
#[derive(Clone, Debug, PartialEq)]
pub enum Settled {
    /// Runs are still going, inside the deadline.
    Running { done: usize, total: usize },
    /// Every run is terminal, or the deadline passed: the verdict.
    Done(Outcome),
}

fn is_terminal(reading: &Option<PlatformRunReading>) -> bool {
    reading
        .as_ref()
        .is_some_and(|r| TERMINAL.contains(&r.status.as_str()))
}

/// The verdict on `build` over `started` and its `readings` (same order),
/// `elapsed` after the check started: `Running` while a queued run is not
/// terminal and the deadline has not passed; otherwise every run is judged,
/// one still going as `timed_out`, one of another build as no verdict.
pub(crate) fn settle(
    started: &[Started],
    readings: &[Option<PlatformRunReading>],
    elapsed: Duration,
    build: Uuid,
) -> Settled {
    let settled = |(function, reading): &(&Started, &Option<PlatformRunReading>)| {
        function.run_id.is_none() || is_terminal(reading)
    };
    let pairs: Vec<(&Started, &Option<PlatformRunReading>)> =
        started.iter().zip(readings).collect();
    let done = pairs.iter().filter(|pair| settled(pair)).count();
    if done < pairs.len() && elapsed < FUNCTIONS_DEADLINE {
        return Settled::Running {
            done,
            total: pairs.len(),
        };
    }
    let results: Vec<FunctionResult> = pairs
        .into_iter()
        .map(|(function, reading)| result_of(function, reading.clone(), build))
        .collect();
    Settled::Done(score(&results))
}

/// Whether a run is a verdict on `build`: the invocation it wrote ran that
/// build. A run that wrote none ran no build's function, and is judged by how
/// it ended, except `done`: a finished run always wrote one, and without it
/// nothing says which build answered.
fn judges(reading: &PlatformRunReading, build: Uuid) -> bool {
    match reading.ran_build {
        Some(ran) => ran == build,
        None => reading.status != "done",
    }
}

/// One started function's result on `build`: refused when it was never
/// queued, no verdict when its run is another build's, judged when it is
/// terminal, `timed_out` otherwise.
fn result_of(
    function: &Started,
    reading: Option<PlatformRunReading>,
    build: Uuid,
) -> FunctionResult {
    let Some(run_id) = function.run_id.clone() else {
        let why = function.error.clone().unwrap_or_default();
        return refused(&function.name, why);
    };
    let terminal = is_terminal(&reading);
    let mut reading = reading.unwrap_or(PlatformRunReading {
        status: "queued".to_string(),
        answer: None,
        error: None,
        invocation_id: None,
        ran_build: None,
        duration_ms: None,
    });
    if !judges(&reading, build) {
        return not_this_build(&function.name, run_id, reading);
    }
    if !terminal {
        reading.status = "timed_out".to_string();
    }
    judge(&function.name, run_id, reading)
}

/// A run that is no verdict on the build being checked ([`judges`]).
fn not_this_build(name: &str, run_id: String, reading: PlatformRunReading) -> FunctionResult {
    let why = match reading.ran_build {
        Some(ran) => format!("ran build {ran}, not the build being checked"),
        None => "the run does not say which build it ran".to_string(),
    };
    FunctionResult {
        name: name.to_string(),
        run_id: Some(run_id),
        status: NOT_THIS_BUILD.to_string(),
        passed: false,
        error: Some(why),
        duration_ms: reading.duration_ms,
        invocation_id: reading.invocation_id,
        ran_build: reading.ran_build,
    }
}

fn refused(name: &str, why: String) -> FunctionResult {
    FunctionResult {
        name: name.to_string(),
        run_id: None,
        status: "refused".to_string(),
        passed: false,
        error: Some(cap(&why, MAX_MESSAGE_CHARS)),
        duration_ms: None,
        invocation_id: None,
        ran_build: None,
    }
}

/// A terminal run, scored as `oxyc checks run` scores it.
pub(crate) fn judge(name: &str, run_id: String, reading: PlatformRunReading) -> FunctionResult {
    let answer_failed = answers_not_ok(reading.answer.as_deref());
    let passed = reading.status == "done" && !answer_failed;
    let error = (!passed).then(|| {
        let why = match (&reading.error, reading.status.as_str()) {
            (Some(error), _) => error.clone(),
            (None, "timed_out") => {
                format!("did not finish within {}s", FUNCTIONS_DEADLINE.as_secs())
            }
            (None, "done") => "answer reported \"ok\": false".to_string(),
            (None, "cancelled") => "run cancelled".to_string(),
            (None, other) => format!("status {other}"),
        };
        cap(&why, MAX_MESSAGE_CHARS)
    });
    FunctionResult {
        name: name.to_string(),
        run_id: Some(run_id),
        status: reading.status,
        passed,
        error,
        duration_ms: reading.duration_ms,
        invocation_id: reading.invocation_id,
        ran_build: reading.ran_build,
    }
}

/// An answer that parses to an object whose `ok` is `false`.
fn answers_not_ok(answer: Option<&str>) -> bool {
    answer
        .and_then(|a| serde_json::from_str::<serde_json::Value>(a).ok())
        .is_some_and(|v| v.get("ok") == Some(&serde_json::Value::Bool(false)))
}

/// The check closed as judged by nobody: no run of this build failed, and
/// `strays` of the `total` runs were another build's.
fn not_judged(results: &[FunctionResult], strays: usize) -> Outcome {
    let total = results.len();
    let mut outcome = Outcome::not_checked(format!(
        "{strays} of {total} check functions ran on another build (staging moved on before \
         they ran); run the checks again while staging serves this build"
    ));
    outcome.detail["functions"] = json!(results);
    outcome
}

/// The check's verdict over every function's result. A run of this build
/// that failed is a failure whatever the others did; else a run that was
/// another build's leaves the check judged by nobody ([`not_judged`]).
pub fn score(results: &[FunctionResult]) -> Outcome {
    let failed: Vec<&FunctionResult> = results.iter().filter(|r| !r.passed).collect();
    let strays = failed.iter().filter(|r| r.status == NOT_THIS_BUILD).count();
    if strays > 0 && strays == failed.len() {
        return not_judged(results, strays);
    }
    let findings: Vec<Finding> = failed
        .iter()
        .map(|r| {
            let why = r.error.clone().unwrap_or_else(|| r.status.clone());
            Finding::new(Severity::Error, format!("{}: {why}", r.name)).at(r.name.clone())
        })
        .collect();
    let extra = json!({ "functions": results });
    let total = results.len();
    if failed.is_empty() {
        let summary = format!("{total} of {total} functions passed");
        return Outcome::with(CheckStatus::Passed, summary, &[], extra);
    }
    let names: Vec<&str> = failed.iter().map(|r| r.name.as_str()).collect();
    let all_refused = failed.iter().all(|r| r.status == "refused");
    let status = if all_refused {
        CheckStatus::Error
    } else {
        CheckStatus::Failed
    };
    let summary = format!(
        "{} of {total} functions failed: {}",
        failed.len(),
        names.join(", ")
    );
    Outcome::with(status, summary, &findings, extra)
}

#[cfg(test)]
#[path = "functions_tests.rs"]
mod tests;
