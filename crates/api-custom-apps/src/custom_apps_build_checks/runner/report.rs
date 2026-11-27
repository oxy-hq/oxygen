//! `POST …/checks/report`: the runner's own verdicts.
//!
//! Five checks are the runner's to judge, and only those are taken: `loads`
//! (Playwright at desk and phone width), `design_system` (the SDK floor and
//! the theme guard), the promote checks gate's two, `smoke` (every screen
//! opens) and `changes` (what differs from production), and a production
//! attempt's one, `production_smoke` (production opens after a move). The
//! platform decides `validation` and `functions` itself, so reporting one is
//! `400 kind_not_reportable`. A kind the attempt was not queued with has no
//! row, and its verdict is written nowhere: `production_smoke` reported for
//! a staging attempt, or `loads` for a production one, changes nothing.
//!
//! **What is stored is what the platform rebuilt, not what was sent**
//! ([`sanitized`]). The verdict must be one of `passed`, `failed`, `error`,
//! `skipped`. The detail must be an object with a `findings` array; findings
//! are rebuilt one by one (a severity, a message and an `at` capped at 400
//! characters, a rule) and cut to 50, the rest counted in
//! `findings_omitted`; the summary is capped; and a detail past
//! [`MAX_DETAIL_BYTES`] once serialized is refused whole (`400
//! detail_too_large`), so a row stays a verdict and never a copy of a page.
//!
//! **What the platform writes into a detail is never taken from a report**
//! ([`without_the_platforms_keys`]): who saw the changed screens (`seen` on
//! `changes`, which can clear a promote's `changes_not_seen`), what followed
//! a failed production check (`move`, `rollback`), and that nobody judged a
//! check (`not_checked`, a claim's to say). A report that sends one has it
//! dropped, so the checker's credential cannot mark screens seen or claim a
//! rollback.
//!
//! **A verdict already written stands.** Every check of a report is checked
//! before any is written, and each write is conditional on the row still
//! being open (`store::finish`): a second report, or one that races a rerun
//! closing the attempt, changes nothing.
//!
//! **A report by hand is audited** ([`ReportedBy`]). The platform checker's
//! verdicts are the platform's own. Anyone else who may call this route (a
//! person with non-production reach, running the checks by hand) can write
//! the verdicts a promote waits for, so each report of theirs that writes
//! one leaves an audit row, [`REPORTED_BY_HAND`], in the transaction that
//! writes the verdicts: who, the app, the build, the attempt, and each kind
//! it wrote with its status. A report that writes nothing (every check
//! already had its verdict) leaves none.
//!
//! **What a report can set off**, each exactly once because the writes run
//! under the attempt's lock (`store::finish_in`):
//!
//! - the report that settles a staging attempt hands it on
//!   (`super::super::settled`);
//! - the report that writes a production attempt's `production_smoke` as
//!   `failed` queues what follows it: the notice, and the automatic rollback
//!   where the app asks for one (`super::super::production::failed`).

use chrono::Utc;
use entity::{app_builds, apps};
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use sea_orm::{DatabaseConnection, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::environment::CheckEnvironment;
use super::super::error::ChecksError;
use super::super::kind::{
    CheckKind, CheckStatus, Finding, MAX_FINDINGS, NOT_CHECKED_KEY, Outcome, Severity, cap,
};
use super::super::production::failed;
use super::super::store::{AttemptKey, Finished};
use super::super::{dto, settled, store};

/// The largest detail one check may store, serialized.
pub const MAX_DETAIL_BYTES: usize = 256 * 1024;
/// The longest rule id kept on a finding.
const MAX_RULE_CHARS: usize = 100;

#[derive(Debug, Default, Deserialize)]
pub struct ReportRequest {
    /// The attempt; the newest when absent or null.
    #[serde(default)]
    pub attempt: Option<i32>,
    /// `staging` or `production`; staging when absent or null.
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub checks: Vec<ReportedCheck>,
}

#[derive(Debug, Deserialize)]
pub struct ReportedCheck {
    pub kind: String,
    pub status: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub detail: Option<Value>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ReportAnswer {
    /// The attempt's roll-up after the report: `running`, `passed`, `failed`,
    /// or `none` when a check of it was judged by nobody (`dto::attempt_state`).
    pub state: &'static str,
    /// The kinds still without a verdict.
    pub open: Vec<String>,
}

fn invalid_detail() -> ChecksError {
    ChecksError::bad_request(
        "invalid_detail",
        "detail must be an object with a `findings` array of {severity: error|warning|info, \
         message, at, rule}",
    )
}

/// One reported finding, rebuilt: the severity checked, the texts capped.
fn finding_of(reported: &Value) -> Result<Finding, ChecksError> {
    let text = |name: &str| reported.get(name).and_then(Value::as_str);
    let severity = match text("severity") {
        Some("error") => Severity::Error,
        Some("warning") => Severity::Warning,
        Some("info") => Severity::Info,
        _ => return Err(invalid_detail()),
    };
    let mut finding = Finding::new(severity, text("message").ok_or_else(invalid_detail)?);
    if let Some(at) = text("at") {
        finding = finding.at(at);
    }
    if let Some(rule) = text("rule") {
        finding = finding.rule(cap(rule, MAX_RULE_CHARS));
    }
    Ok(finding)
}

/// The reportable kind `raw` names.
fn reportable(raw: &str) -> Result<CheckKind, ChecksError> {
    match CheckKind::parse(raw) {
        Some(
            kind @ (CheckKind::Loads
            | CheckKind::DesignSystem
            | CheckKind::Smoke
            | CheckKind::Changes
            | CheckKind::ProductionSmoke),
        ) => Ok(kind),
        _ => Err(ChecksError::bad_request(
            "kind_not_reportable",
            format!(
                "a runner reports `loads`, `design_system`, `smoke`, `changes` and \
                 `production_smoke`; `{}` is not one of them",
                cap(raw, 40)
            ),
        )),
    }
}

/// The keys of a check's detail the platform writes itself (`changes/seen`,
/// `production::followup`, a claim's `Outcome::not_checked`), which a report
/// may not set.
pub const PLATFORM_KEYS: [&str; 4] = ["seen", "move", "rollback", NOT_CHECKED_KEY];

/// A reported detail without [`PLATFORM_KEYS`]; `changes` keeps `seen` as
/// the `null` it starts with, the shape its readers expect.
fn without_the_platforms_keys(kind: CheckKind, extra: &mut serde_json::Map<String, Value>) {
    for key in PLATFORM_KEYS {
        extra.remove(key);
    }
    if kind == CheckKind::Changes {
        extra.insert("seen".to_string(), Value::Null);
    }
}

/// One reported check as it is stored, or why it is refused.
pub fn sanitized(check: &ReportedCheck) -> Result<(CheckKind, Outcome), ChecksError> {
    let kind = reportable(&check.kind)?;
    let status = CheckStatus::parse(&check.status)
        .filter(|status| status.is_terminal())
        .ok_or_else(|| {
            ChecksError::bad_request(
                "invalid_status",
                "status must be one of passed, failed, error, skipped",
            )
        })?;
    let Some(Value::Object(mut extra)) = check.detail.clone() else {
        return Err(invalid_detail());
    };
    let Some(Value::Array(reported)) = extra.remove("findings") else {
        return Err(invalid_detail());
    };
    without_the_platforms_keys(kind, &mut extra);
    // What the runner left out itself, and what is cut here.
    let left_out = extra
        .remove("findings_omitted")
        .and_then(|n| n.as_u64())
        .unwrap_or(0);
    let cut = reported.len().saturating_sub(MAX_FINDINGS) as u64;
    let findings: Vec<Finding> = reported
        .iter()
        .take(MAX_FINDINGS)
        .map(finding_of)
        .collect::<Result<_, _>>()?;
    let summary = check.summary.clone().unwrap_or_default();
    let mut outcome = Outcome::with(status, summary, &findings, Value::Object(extra));
    if left_out + cut > 0 {
        outcome.detail["findings_omitted"] = json!(left_out + cut);
    }
    let bytes = serde_json::to_vec(&outcome.detail).map_or(usize::MAX, |v| v.len());
    if bytes > MAX_DETAIL_BYTES {
        return Err(ChecksError::bad_request(
            "detail_too_large",
            format!(
                "the detail of `{}` is {bytes} bytes, more than the {MAX_DETAIL_BYTES} one check \
                 stores; report findings and facts, never a page or a response body",
                kind.as_str()
            ),
        ));
    }
    Ok((kind, outcome))
}

/// The audit action of verdicts reported by a caller who is not the platform
/// checker.
pub const REPORTED_BY_HAND: &str = "app.build_checks_reported_by_hand";

/// Who reports.
#[derive(Clone, Copy, Debug)]
pub enum ReportedBy<'a> {
    /// The platform checker: its verdicts are the platform's own.
    Checker,
    /// Anyone else who may call the route, running the checks by hand: the
    /// verdicts they write are audited.
    Hand(&'a RequestActor),
}

/// The audit row of a report by hand that wrote `wrote`: each of those kinds
/// with the status reported for it, in the report's order.
fn by_hand_entry(
    actor: &RequestActor,
    (app, build): (&apps::Model, &app_builds::Model),
    key: &AttemptKey,
    (wrote, verdicts): (&[CheckKind], &[(CheckKind, Outcome)]),
) -> AuditEntry {
    let mut listed: Vec<CheckKind> = Vec::new();
    let mut checks = Vec::new();
    for (kind, outcome) in verdicts {
        // A kind sent twice was written once, as its first verdict.
        if wrote.contains(kind) && !listed.contains(kind) {
            listed.push(*kind);
            checks.push(json!({ "kind": kind.as_str(), "status": outcome.status.as_str() }));
        }
    }
    AuditEntry::for_request(actor, REPORTED_BY_HAND)
        .org(app.org_id)
        .workspace(app.project_id)
        .target("custom_app", app.id.to_string(), app.slug.clone())
        .metadata(json!({ "build": build.build_id, "build_pk": build.id,
            "environment": key.environment, "attempt": key.attempt, "checks": checks }))
}

/// Write `verdicts` under the attempt's lock and, for a report by hand that
/// wrote any, its audit row with them: both land or neither does.
async fn write(
    db: &DatabaseConnection,
    by: ReportedBy<'_>,
    built: (&apps::Model, &app_builds::Model),
    key: &AttemptKey,
    verdicts: &[(CheckKind, Outcome)],
) -> Result<Finished, ChecksError> {
    let txn = db.begin().await?;
    let finished = store::finish_in(&txn, key, verdicts).await?;
    if let ReportedBy::Hand(actor) = by
        && !finished.wrote.is_empty()
    {
        let entry = by_hand_entry(actor, built, key, (&finished.wrote, verdicts));
        audit::record_in_txn(&txn, entry).await?;
    }
    txn.commit().await?;
    Ok(finished)
}

/// Write the verdicts `request` reports for its attempt of `build` in
/// `environment`, each unless its check already has one, and answer where the
/// attempt stands. `by` says whose report it is ([`ReportedBy`]).
pub async fn report(
    db: &DatabaseConnection,
    (app, build): (&apps::Model, &app_builds::Model),
    request: &ReportRequest,
    environment: CheckEnvironment,
    by: ReportedBy<'_>,
) -> Result<ReportAnswer, ChecksError> {
    let verdicts: Vec<(CheckKind, Outcome)> = request
        .checks
        .iter()
        .map(sanitized)
        .collect::<Result<_, _>>()?;
    let key = super::attempt_of(db, build, environment, request.attempt).await?;
    if store::rows_of(db, &key).await?.is_empty() {
        return Err(ChecksError::NotFound);
    }
    let finished = write(db, by, (app, build), &key, &verdicts).await?;
    for (kind, outcome) in &verdicts {
        let written = finished.wrote.contains(kind);
        tracing::info!(build = %build.build_id, attempt = key.attempt, kind = kind.as_str(),
            environment = environment.name(), status = outcome.status.as_str(), written,
            "build checks: verdict reported");
    }
    if finished.settled {
        settled::on_staging_attempt_settled(db, app, build, &key).await;
    }
    failed::after_report(db, (app, build), &key, &finished.wrote, &verdicts).await;
    let rows = store::rows_of(db, &key).await?;
    Ok(ReportAnswer {
        state: dto::attempt_state(&rows, Utc::now()),
        open: super::open_kinds(&rows),
    })
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
