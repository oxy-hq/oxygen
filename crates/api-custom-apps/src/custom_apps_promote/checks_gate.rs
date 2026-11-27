//! The promote checks gate: which of a build's staging checks hold its promote
//! to production (`internal-docs/custom-app-sandboxes.md` section 5.9).
//!
//! Asked only where the app's org has the `promote_checks_gate` flag on, of
//! the **newest staging attempt of the build being promoted**, read as `GET
//! …/checks` answers it (`custom_apps_build_checks::dto`: a stalled attempt's
//! open checks read `error`, a check nobody judged is no pass).
//!
//! | The app's settings say | A promote is held while | Blocker |
//! | --- | --- | --- |
//! | `required` lists a kind | that kind is not `passed` or `skipped`, and no longer open: `failed`, `error`, no row of it in the attempt, or a `skipped` nobody decided by judging the build (`dto::not_checked`: nothing was started for the attempt, or staging had moved on by the claim) | `checks_not_passed` |
//! | `required` lists a kind | that kind is still `queued` or `running` | `checks_running` |
//! | `seen_required` | screens differ from production and nobody has marked them seen; also while the comparison is still running (`checks_running`), and when it never produced an answer to look at (it failed, never ran, or its runner stopped) | `changes_not_seen` |
//!
//! `unmet` and `running` never share a kind. An advisory row is never a
//! required kind's answer. `validation` is not asked here: it has its own
//! blocker.
//!
//! **A rollback is exempt**: the emergency path, to a build production served
//! before. A move that re-stamps the build production already serves is too.
//! **An override** (`checks_override`) clears these three blockers and no
//! other; the checks stay as they are.
//!
//! Decided twice, like invariant 1: by the plan ([`read`], through
//! `blockers::evaluate`), and by the one production move ([`at_move`], from
//! `production::admissible`), which every writer of production passes.
//! [`at_move`] is a plain `fn` that hands back a boxed future: every move of
//! production (a promoting publish's included) awaits it inside its own
//! transaction, and awaited as an `async fn` it grew those movers' poll frames
//! in a debug build past a test thread's 2 MiB stack.

use chrono::{DateTime, Utc};
use entity::{app_build_checks, app_builds, apps};
use futures::future::BoxFuture;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::ConnectionTrait;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::checks_override::{AcceptedOverride, ChecksAsk};
use super::error::{Blocker, CHANGES_NOT_SEEN, CHECKS_NOT_PASSED, CHECKS_RUNNING, PromoteError};
use super::production::ProductionMove;
use crate::custom_apps_build_checks::dto::{self, SUMMARY_KIND};
use crate::custom_apps_build_checks::kind::{CheckKind, CheckStatus};
use crate::custom_apps_build_checks::settings::{self, Settings};
use crate::custom_apps_build_checks::store;
use crate::custom_apps_build_history as history;
use crate::custom_apps_environments::EnvAction;

/// One row of the attempt, as the plan lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckRow {
    pub kind: String,
    pub label: String,
    pub status: String,
    pub summary: Option<String>,
    /// Whether the app requires this kind (it is in `required`).
    pub required: bool,
}

/// What the `changes` check found, and whether a person has to see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Changes {
    pub changed: u64,
    pub compared: u64,
    pub seen: bool,
    pub seen_required: bool,
}

/// The override a request carried and the gate accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Overridden {
    pub reason: String,
}

/// The plan's `checks`: where the build stands against what the app requires.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Checks {
    /// The kinds the app requires.
    pub required: Vec<String>,
    /// The newest staging attempt of the build; `null` when it has none.
    pub attempt: Option<i32>,
    /// That attempt's roll-up: `none`, `running`, `failed` or `passed`.
    pub state: &'static str,
    /// Every row of that attempt, in the list's order.
    pub rows: Vec<CheckRow>,
    /// Required kinds that ended without being satisfied, or never ran.
    pub unmet: Vec<String>,
    /// Required kinds still open.
    pub running: Vec<String>,
    pub changes: Changes,
    /// The attempt's summary note, when it has one.
    pub summary: Option<Value>,
    /// Whether the caller may send `override_checks` (`checks_override`).
    pub overridable: bool,
    /// Set when the request carried an override that was accepted.
    pub overridden: Option<Overridden>,
    /// Set when `seen_required` asks for a comparison that has no answer to
    /// look at: the `changes` row's status, or `not run`.
    #[serde(skip)]
    not_compared: Option<String>,
    /// The kinds whose row nobody judged (`dto::not_checked`): a blocker
    /// names them `not run`, whatever status the wire gives them.
    #[serde(skip)]
    not_run: Vec<String>,
}

/// How a required kind's row answers the requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    Satisfied,
    Open,
    Unmet,
}

/// A skip satisfies only when the check decided it itself: one nobody
/// decided by judging the build (`dto::not_checked`) leaves the kind unmet.
fn standing_of(row: &app_build_checks::Model) -> Standing {
    match CheckStatus::parse(&row.status) {
        Some(CheckStatus::Skipped) if dto::not_checked(row) => Standing::Unmet,
        Some(CheckStatus::Passed | CheckStatus::Skipped) => Standing::Satisfied,
        Some(CheckStatus::Queued | CheckStatus::Running) => Standing::Open,
        Some(CheckStatus::Failed | CheckStatus::Error) | None => Standing::Unmet,
    }
}

/// The summary note of an attempt's `summary` row: its first note, else its
/// detail when that is the note itself (it carries `text`).
fn summary_note(row: &app_build_checks::Model) -> Option<Value> {
    let first = row.notes.as_array().and_then(|notes| notes.first());
    first.cloned().or_else(|| {
        row.detail
            .clone()
            .filter(|detail| detail.get("text").is_some())
    })
}

/// Where `rows` (every staging row of one build, any attempt) stand against
/// `settings`, as of `now`. Pure: the plan and the move read through it.
pub fn evaluate(
    settings: &Settings,
    rows: Vec<app_build_checks::Model>,
    now: DateTime<Utc>,
) -> Checks {
    let attempt = rows.iter().map(|row| row.attempt).max();
    let newest: Vec<_> = rows
        .into_iter()
        .filter(|row| Some(row.attempt) == attempt)
        .collect();
    let state = dto::attempt_state(&newest, now);
    let (answered, _) = dto::answered(newest, now);
    let (summaries, mut checks): (Vec<_>, Vec<_>) = answered
        .into_iter()
        .partition(|row| row.kind == SUMMARY_KIND);
    checks.sort_by(|a, b| CheckKind::list_key(&a.kind).cmp(&CheckKind::list_key(&b.kind)));
    let required: Vec<String> = settings
        .effective_required()
        .into_iter()
        .map(|kind| kind.as_str().to_string())
        .collect();
    let (mut unmet, mut running) = (Vec::new(), Vec::new());
    for kind in &required {
        let row = checks.iter().find(|row| !row.advisory && &row.kind == kind);
        match row.map(standing_of) {
            Some(Standing::Satisfied) => {}
            Some(Standing::Open) => running.push(kind.clone()),
            Some(Standing::Unmet) | None => unmet.push(kind.clone()),
        }
    }
    let (changes, not_compared) = changes_of(settings, &checks, &mut running);
    let not_run = checks.iter().filter(|row| dto::not_checked(row));
    let not_run = not_run.map(|row| row.kind.clone()).collect();
    Checks {
        rows: checks
            .iter()
            .map(|row| CheckRow {
                kind: row.kind.clone(),
                label: CheckKind::label_of(&row.kind),
                status: row.status.clone(),
                summary: row.summary.clone(),
                required: required.contains(&row.kind),
            })
            .collect(),
        required,
        attempt,
        state,
        unmet,
        running,
        changes,
        summary: summaries.first().and_then(summary_note),
        overridable: false,
        overridden: None,
        not_compared,
        not_run,
    }
}

/// What the attempt's `changes` row found, as it is answered. With
/// `seen_required`, a comparison still open joins `running`, and one with no
/// answer to look at is named (the second value): a person cannot have seen
/// what was never compared. One its runner left behind is not open: it is
/// answered `error` like any check a runner owes (`dto::answered`).
fn changes_of(
    settings: &Settings,
    checks: &[app_build_checks::Model],
    running: &mut Vec<String>,
) -> (Changes, Option<String>) {
    let kind = CheckKind::Changes.as_str();
    let row = checks.iter().find(|row| row.kind == kind);
    let detail = row.and_then(|row| row.detail.as_ref());
    let count = |name: &str| {
        detail
            .and_then(|detail| detail.get(name))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let changes = Changes {
        changed: count("changed"),
        compared: count("compared"),
        seen: detail
            .and_then(|detail| detail.get("seen"))
            .is_some_and(|seen| !seen.is_null()),
        seen_required: settings.seen_required,
    };
    if !settings.seen_required {
        return (changes, None);
    }
    let status = row.map(|row| row.status.as_str());
    let not_compared = match status.and_then(CheckStatus::parse) {
        _ if row.is_some_and(dto::not_checked) => Some("not run".to_string()),
        Some(CheckStatus::Passed | CheckStatus::Skipped) => None,
        Some(CheckStatus::Queued | CheckStatus::Running) => {
            running.push(kind.to_string());
            None
        }
        Some(CheckStatus::Failed | CheckStatus::Error) => status.map(str::to_string),
        None => Some("not run".to_string()),
    };
    (changes, not_compared)
}

impl Checks {
    /// Whether `seen_required` holds the move.
    fn changes_not_seen(&self) -> bool {
        let unanswered = self.changes.changed > 0 || self.not_compared.is_some();
        self.changes.seen_required && !self.changes.seen && unanswered
    }

    /// Whether any of the three blockers holds the move.
    pub fn holds(&self) -> bool {
        !self.unmet.is_empty() || !self.running.is_empty() || self.changes_not_seen()
    }

    /// `label (status)` of each kind in `kinds`, as a sentence lists them.
    fn named(&self, kinds: &[String]) -> String {
        let named: Vec<String> = kinds
            .iter()
            .map(|kind| {
                let row = self.rows.iter().find(|row| &row.kind == kind);
                let status = match row {
                    Some(row) if !self.not_run.contains(kind) => row.status.as_str(),
                    _ => "not run",
                };
                format!("{} ({status})", CheckKind::label_of(kind))
            })
            .collect();
        named.join(", ")
    }

    /// The blockers of a promote of build `build_label`, in the order
    /// `checks_not_passed`, `checks_running`, `changes_not_seen`.
    pub fn blockers(&self, build_label: &str) -> Vec<Blocker> {
        let mut blockers = Vec::new();
        if !self.unmet.is_empty() {
            blockers.push(Blocker {
                code: CHECKS_NOT_PASSED,
                message: format!(
                    "build {build_label:?} has not passed the checks this app requires: {}. Fix \
                     it and publish again, run the checks again, or promote with an override and \
                     a reason",
                    self.named(&self.unmet)
                ),
            });
        }
        if !self.running.is_empty() {
            blockers.push(Blocker {
                code: CHECKS_RUNNING,
                message: format!(
                    "the checks this app requires are still running for build {build_label:?}: \
                     {}. Wait for them to finish, or promote with an override and a reason",
                    self.named(&self.running)
                ),
            });
        }
        if self.changes_not_seen() {
            blockers.push(Blocker {
                code: CHANGES_NOT_SEEN,
                message: self.not_seen_message(build_label),
            });
        }
        blockers
    }

    fn not_seen_message(&self, build_label: &str) -> String {
        let asks = "this app requires a person to look at what changed before a promote";
        match &self.not_compared {
            Some(status) => format!(
                "{asks}, and the comparison of build {build_label:?} with production has no \
                 answer to look at ({status}). Run the checks again, or promote with an override \
                 and a reason"
            ),
            None => format!(
                "{asks}: {} of {} screens of build {build_label:?} differ from production and \
                 nobody has marked them seen. Mark them seen in the console's Checks panel, or \
                 promote with an override and a reason",
                self.changes.changed, self.changes.compared
            ),
        }
    }
}

/// Where `build` stands against what `app` requires; `None` where the gate is
/// off for the app's org.
pub async fn read<C: ConnectionTrait>(
    conn: &C,
    app: &apps::Model,
    build: &app_builds::Model,
) -> Result<Option<Checks>, PromoteError> {
    if !settings::enabled_for(app.org_id) {
        return Ok(None);
    }
    let rows = store::rows_of_build(conn, build.id, &AppEnvironment::Staging.name())
        .await
        .map_err(|e| PromoteError::internal("read the build's checks", e))?;
    Ok(Some(evaluate(&Settings::of(app), rows, Utc::now())))
}

/// What the gate says of one production move.
#[derive(Debug, Default)]
pub(super) struct AtMove {
    /// The check blockers that refuse the move.
    pub blockers: Vec<Blocker>,
    /// The override that cleared them, and what it cleared: the move's audit row.
    pub overridden: Option<(AcceptedOverride, Checks)>,
}

/// The gate, for a move of production from `previous` to `build`: nothing for
/// a fixture, a re-stamp, a rollback to a build production served before, and
/// wherever the gate is off; else the blockers, or the override that cleared
/// them. A rollback to a build production never served is a promote by
/// another name, and is held like one.
pub(super) fn at_move<'a, C: ConnectionTrait>(
    conn: &'a C,
    row: &'a apps::Model,
    build: &'a app_builds::Model,
    mv: &'a ProductionMove,
    previous: Option<Uuid>,
) -> BoxFuture<'a, Result<AtMove, PromoteError>> {
    Box::pin(gate_at_move(conn, row, build, mv, previous))
}

async fn gate_at_move<C: ConnectionTrait>(
    conn: &C,
    row: &apps::Model,
    build: &app_builds::Model,
    mv: &ProductionMove,
    previous: Option<Uuid>,
) -> Result<AtMove, PromoteError> {
    let restamp = previous == Some(build.id);
    if matches!(mv.checks, ChecksAsk::Fixture(_)) || restamp || !settings::enabled_for(row.org_id) {
        return Ok(AtMove::default());
    }
    if mv.action == EnvAction::Rollback {
        let production = AppEnvironment::Production;
        let served = history::has_served(conn, build.app_id, &production, build.id)
            .await
            .map_err(|e| PromoteError::internal("read production's history", e))?;
        if served {
            return Ok(AtMove::default());
        }
    }
    let Some(checks) = read(conn, row, build).await? else {
        return Ok(AtMove::default());
    };
    if !checks.holds() {
        return Ok(AtMove::default());
    }
    Ok(match &mv.checks {
        ChecksAsk::Overridden(accepted) => AtMove {
            blockers: Vec::new(),
            overridden: Some((accepted.clone(), checks)),
        },
        ChecksAsk::Gated | ChecksAsk::Fixture(_) => AtMove {
            blockers: checks.blockers(&build.build_id),
            overridden: None,
        },
    })
}

#[cfg(test)]
#[path = "checks_gate_tests.rs"]
mod tests;
