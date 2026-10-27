//! The Oxy records platform notices are rendered from, loaded in batches.
//!
//! One query per subject kind on a page: workspaces by id, runs by id (plus
//! one for their failing streaks), Ask runs by id, Ask suspensions by run (one
//! query for the runs, one for their suspensions), anomaly buckets by event,
//! CLI device requests by user code. A billing episode needs no query — its id is the
//! end of the grace period, which is the one fact the notice states — and a
//! test notice needs one only to name the token that sent it.

use std::collections::{HashMap, HashSet};

use agentic_runtime::entity::run;
use base64::Engine;
use chrono::{DateTime, Utc};
use entity::{metric_anomalies, workspaces};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::kinds::{
    SUBJECT_AIRWAY_RUN, SUBJECT_ANALYTICS_RUN, SUBJECT_ANALYTICS_SUSPENSION, SUBJECT_ANOMALY,
    SUBJECT_AUTOMATION_RUN, SUBJECT_BILLING, SUBJECT_CLI_DEVICE_REQUEST, SUBJECT_TEST,
    SUBJECT_WORKSPACE,
};
use super::render::About;
use super::run_streak::Streak;

/// A loaded subject.
#[derive(Debug, Clone, PartialEq)]
pub enum Record {
    Billing {
        grace_ends_at: DateTime<Utc>,
    },
    Workspace {
        id: Uuid,
        name: String,
    },
    Run(RunRecord),
    Anomaly(AnomalyRecord),
    /// An `oxyc … --no-browser` request waiting on the person it named.
    CliDeviceRequest {
        /// Stored form, without the dash. Never worded.
        user_code: String,
        hostname: String,
        /// `login`, `agent` or `sandbox_agent`.
        kind: &'static str,
        /// The ask's `apps`, split on commas, as sent.
        apps: Vec<String>,
        hours: Option<i64>,
        /// `pending`, `approved` or `denied`.
        status: String,
        expires_at: DateTime<Utc>,
    },
    Test(super::self_test::TestRecord),
    /// An Ask run someone asked to hear about.
    Ask(super::ask_finished::AskRecord),
    /// One suspension of an Ask run someone asked to hear about.
    AskWaiting(super::ask_waiting::AskWaitingRecord),
}

/// A failed automation or Airway run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub id: String,
    pub workspace_id: Uuid,
    /// The automation's or pipeline's name.
    pub name: String,
    /// The file it ran, as the web app's route encodes it.
    pub file: Option<String>,
    pub airway: bool,
    /// Started by a schedule rather than a person.
    pub scheduled: bool,
    /// When the run started, so two failures of one automation read apart.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// How long its source had been failing by this run; `None` when not
    /// loaded (see [`load`]) or the run names no file.
    pub streak: Option<Streak>,
}

impl RunRecord {
    /// Where the run is shown in the web app.
    pub fn href(&self, org_slug: &str) -> String {
        let ws = format!("/{org_slug}/workspaces/{}", self.workspace_id);
        // The web app's routes carry the file path base64-encoded
        // (`encodeBase64` in `web-app/src/libs/encoding.ts`: standard
        // alphabet over the UTF-8 bytes).
        let Some(file) = &self.file else {
            return ws;
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(file.as_bytes());
        match self.airway {
            true => format!("{ws}/pipelines/{b64}/runs/{}", self.id),
            false => format!("{ws}/automations/{b64}?run={}", self.id),
        }
    }
}

/// The most severe bucket of an anomaly event.
#[derive(Debug, Clone, PartialEq)]
pub struct AnomalyRecord {
    pub workspace_id: Uuid,
    /// The monitor's label, or the measure it watches.
    pub name: String,
    pub granularity: String,
    pub period_start: DateTime<Utc>,
    pub observed: f64,
    pub expected: f64,
}

impl AnomalyRecord {
    /// One line: what was seen against what was expected, and when.
    pub fn summary(&self) -> String {
        format!(
            "{} against {} expected for the {} of {}.",
            number(self.observed),
            number(self.expected),
            self.granularity,
            self.period_start.format("%B %-d, %Y")
        )
    }
}

/// A value as a person reads it: whole numbers plain, others to two places.
fn number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// Every subject in `abouts` that can still be found, keyed by (kind, id).
pub async fn load(
    db: &DatabaseConnection,
    abouts: &HashSet<About>,
) -> Result<HashMap<(&'static str, String), Record>, DbErr> {
    let ids = |kind: &str| -> Vec<String> {
        abouts
            .iter()
            .filter(|a| a.subject_kind == kind)
            .map(|a| a.subject_id.clone())
            .collect()
    };
    let mut out = HashMap::new();
    for id in ids(SUBJECT_BILLING) {
        if let Ok(at) = DateTime::parse_from_rfc3339(&id) {
            out.insert(
                (SUBJECT_BILLING, id),
                Record::Billing {
                    grace_ends_at: at.with_timezone(&Utc),
                },
            );
        }
    }
    for (id, record) in workspaces_by_id(db, ids(SUBJECT_WORKSPACE)).await? {
        out.insert((SUBJECT_WORKSPACE, id), record);
    }
    let run_ids: Vec<String> = ids(SUBJECT_AUTOMATION_RUN)
        .into_iter()
        .chain(ids(SUBJECT_AIRWAY_RUN))
        .collect();
    for record in runs_by_id(db, run_ids).await? {
        let kind = match record.airway {
            true => SUBJECT_AIRWAY_RUN,
            false => SUBJECT_AUTOMATION_RUN,
        };
        out.insert((kind, record.id.clone()), Record::Run(record));
    }
    for (id, record) in anomalies_by_event(db, ids(SUBJECT_ANOMALY)).await? {
        out.insert((SUBJECT_ANOMALY, id), Record::Anomaly(record));
    }
    for (id, record) in super::cli_approval::load(db, ids(SUBJECT_CLI_DEVICE_REQUEST)).await? {
        out.insert((SUBJECT_CLI_DEVICE_REQUEST, id), record);
    }
    for (id, record) in super::self_test::load(db, ids(SUBJECT_TEST)).await? {
        out.insert((SUBJECT_TEST, id), Record::Test(record));
    }
    for (id, record) in super::ask_finished::load(db, ids(SUBJECT_ANALYTICS_RUN)).await? {
        out.insert((SUBJECT_ANALYTICS_RUN, id), record);
    }
    for (id, record) in super::ask_waiting::load(db, ids(SUBJECT_ANALYTICS_SUSPENSION)).await? {
        out.insert((SUBJECT_ANALYTICS_SUSPENSION, id), record);
    }
    Ok(out)
}

fn uuids(ids: &[String]) -> Vec<Uuid> {
    ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect()
}

async fn workspaces_by_id(
    db: &DatabaseConnection,
    ids: Vec<String>,
) -> Result<Vec<(String, Record)>, DbErr> {
    let ids = uuids(&ids);
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(workspaces::Entity::find()
        .filter(workspaces::Column::Id.is_in(ids))
        .all(db)
        .await?
        .into_iter()
        .map(|w| {
            (
                w.id.to_string(),
                Record::Workspace {
                    id: w.id,
                    name: w.name,
                },
            )
        })
        .collect())
}

/// The runs, each with its streak: a second query for the page, not one per
/// run.
async fn runs_by_id(db: &DatabaseConnection, ids: Vec<String>) -> Result<Vec<RunRecord>, DbErr> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let streaks = super::run_streak::through(db, &ids).await?;
    Ok(run::Entity::find()
        .filter(run::Column::Id.is_in(ids))
        .all(db)
        .await?
        .iter()
        .filter_map(run_record)
        .map(|mut r| {
            r.streak = streaks.get(&r.id).copied();
            r
        })
        .collect())
}

/// What a notice shows of a run row. `None` for a run that is neither an
/// automation nor an Airway pipeline.
pub fn run_record(row: &run::Model) -> Option<RunRecord> {
    let airway = match row.source_type.as_deref() {
        Some(agentic_pipeline::AIRWAY_SOURCE_TYPE) => true,
        Some(agentic_pipeline::WORKFLOW_SOURCE_TYPE) => false,
        _ => return None,
    };
    let meta = row.metadata.as_ref();
    let text = |key: &str| meta.and_then(|m| m.get(key)).and_then(|v| v.as_str());
    let file = text(if airway {
        "pipeline_ref"
    } else {
        "workflow_ref"
    })
    .map(str::to_string);
    let name = text("pipeline_name")
        .filter(|_| airway)
        .map(str::to_string)
        .or_else(|| file.as_deref().map(file_stem))
        .unwrap_or_else(|| {
            if airway {
                "A pipeline"
            } else {
                "An automation"
            }
            .to_string()
        });
    Some(RunRecord {
        id: row.id.clone(),
        workspace_id: row.workspace_id,
        name,
        file,
        airway,
        scheduled: row.schedule_id.is_some(),
        started_at: row.created_at.with_timezone(&chrono::Utc),
        streak: None,
    })
}

/// `automations/nightly.automation.yml` → `nightly`.
fn file_stem(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    base.split('.').next().unwrap_or(base).to_string()
}

async fn anomalies_by_event(
    db: &DatabaseConnection,
    ids: Vec<String>,
) -> Result<Vec<(String, AnomalyRecord)>, DbErr> {
    let ids = uuids(&ids);
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = metric_anomalies::Entity::find()
        .filter(metric_anomalies::Column::EventId.is_in(ids))
        .all(db)
        .await?;
    let mut worst: HashMap<Uuid, metric_anomalies::Model> = HashMap::new();
    for row in rows {
        let Some(event) = row.event_id else { continue };
        let replace = worst.get(&event).is_none_or(|w| rank(&row) > rank(w));
        if replace {
            worst.insert(event, row);
        }
    }
    Ok(worst
        .into_iter()
        .map(|(event, a)| {
            (
                event.to_string(),
                AnomalyRecord {
                    workspace_id: a.workspace_id,
                    name: a.label.clone().unwrap_or_else(|| a.measure.clone()),
                    granularity: a.granularity.clone(),
                    period_start: a.period_start.with_timezone(&Utc),
                    observed: a.observed,
                    expected: a.expected,
                },
            )
        })
        .collect())
}

/// Most severe first, then the latest period.
fn rank(a: &metric_anomalies::Model) -> (u8, DateTime<Utc>) {
    (
        oxy_metric_monitoring::detect::severity_rank(&a.severity),
        a.period_start.with_timezone(&Utc),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(source: &str, metadata: serde_json::Value, schedule: Option<&str>) -> run::Model {
        let now = Utc::now().fixed_offset();
        run::Model {
            id: "r-1".into(),
            question: String::new(),
            answer: None,
            error_message: None,
            thread_id: None,
            source_type: Some(source.into()),
            metadata: Some(metadata),
            parent_run_id: None,
            schedule_id: schedule.map(str::to_string),
            task_status: Some("failed".into()),
            task_metadata: None,
            attempt: 0,
            recovery_requested_at: None,
            driver_id: None,
            driver_heartbeat_at: None,
            cancel_requested_at: None,
            workspace_id: Uuid::nil(),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn an_automation_run_links_to_its_file_and_run_the_way_the_web_app_does() {
        let r = run_record(&row(
            agentic_pipeline::WORKFLOW_SOURCE_TYPE,
            serde_json::json!({ "workflow_ref": "automations/nightly.automation.yml" }),
            Some("s-1"),
        ))
        .unwrap();
        assert_eq!(r.name, "nightly");
        assert!(r.scheduled && !r.airway);
        // btoa("automations/nightly.automation.yml")
        assert_eq!(
            r.href("acme"),
            format!(
                "/acme/workspaces/{}/automations/YXV0b21hdGlvbnMvbmlnaHRseS5hdXRvbWF0aW9uLnltbA==?run=r-1",
                Uuid::nil()
            )
        );
    }

    #[test]
    fn an_airway_run_is_named_by_its_pipeline_and_other_runs_are_not_notices() {
        let r = run_record(&row(
            agentic_pipeline::AIRWAY_SOURCE_TYPE,
            serde_json::json!({ "pipeline_ref": "toast.airway.yml", "pipeline_name": "Toast" }),
            None,
        ))
        .unwrap();
        assert_eq!(r.name, "Toast");
        assert!(
            r.href("acme")
                .ends_with("/pipelines/dG9hc3QuYWlyd2F5LnltbA==/runs/r-1")
        );
        assert!(run_record(&row("analytics", serde_json::json!({}), None)).is_none());
    }

    #[test]
    fn numbers_read_plainly() {
        assert_eq!(number(1200.0), "1200");
        assert_eq!(number(12.3456), "12.35");
    }
}
