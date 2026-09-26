//! Before a release serves prod, judge every live custom-app function against
//! THIS binary's host rules, and refuse to roll out a release that would stop
//! a working one.
//!
//! **Why.** In Sep 2026, #3187 made customer warehouses read-only to apps
//! unless a function declares `customerWarehouseWrites`. The fix for existing
//! apps was "republish before the release"; one app was not republished, and
//! 0.5.147 refused every write it made for a week. The failure was knowable
//! before a single pod rolled: the new binary's rule and the live app's
//! manifest were both sitting in prod's database.
//!
//! **Where.** `oxy migrate`, which the chart runs as a pre-upgrade hook with the
//! NEW image against prod's database: the one place the next binary and prod's
//! real apps meet before users do. It runs after the migrations — they are
//! additive and every migrator tolerates rows it does not know, so a blocked
//! rollout leaves the old pods serving on a schema they can use — and a failed
//! hook stops the rollout with nothing served.
//!
//! **What.** Two rules, both evaluated by the host's own code rather than
//! restated:
//! - every database a function names in `destinations` must pass
//!   [`destination_write_policy`] — the check that refused the warehouse app;
//! - its manifest must still parse. The runtime parses with `.ok()` and falls
//!   back to no capabilities, so a manifest a release can no longer read turns
//!   off every gated `ctx.*` call without an error anyone sees.
//!
//! **Only what the release breaks blocks.** A refusal blocks when it is *new* —
//! absent from the refusals the last rollout recorded ([`ledger`]) — and the
//! function answered successfully in the last [`WORKING_WITHIN_DAYS`] days: it
//! worked, and on this binary it will not. A refusal the running binary already
//! enforces is carried over and reported in the log, never blocking, even when
//! the function answers on its other paths: stopping a platform release cannot
//! fix that app, and one broken app must not hold every release hostage. A
//! rollout the preflight lets through records what it saw; a blocked one
//! records nothing, so its retry blocks again.
//!
//! **The first run records a baseline.** With nothing recorded, every refusal
//! reads as new, so the first run on a deployment — the `oxy migrate` that
//! created the ledger, or a retry of it within [`BASELINE_HOURS`] — never
//! blocks: it records what live apps already carry and reports it. Turning
//! `block` on therefore needs no warm-up rollout in `warn`.
//!
//! **Tightening a host gate?** Add its rule here, or the preflight cannot see
//! the apps your change breaks.
//!
//! `OXY_APP_PREFLIGHT` = `off` | `warn` (default) | `block`. Staging and prod
//! run `block`; dev reports. To roll out a release knowingly, set `warn` for
//! that rollout — it records the refusals, so the next release does not block
//! on them either.

pub mod ledger;
pub mod report;

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{Duration, Utc};
use oxy::config::model::Database;
use oxy::database::client::establish_connection;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use uuid::Uuid;

use super::FunctionManifestEntry;
use super::host::{DestinationKind, WriteSurface, destination_kind, destination_write_policy};

/// A function counts as working when it answered this recently.
pub const WORKING_WITHIN_DAYS: i64 = 7;
/// How long after the ledger's migration a run still counts as the first — long
/// enough for the Job's and Argo's retries of that same rollout.
pub const BASELINE_HOURS: i64 = 6;
/// The migration that creates the ledger; its `applied_at` dates the first run.
const LEDGER_MIGRATION: &str = "m20260925_000001_app_preflight_refusals";
const ENV: &str = "OXY_APP_PREFLIGHT";

/// What the preflight does with what it finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    Warn,
    Block,
}

impl Mode {
    /// The mode, and a note when `value` names none. An unknown value warns
    /// rather than blocks — a typo in a values file must not be what stops a
    /// release — so the note is the only sign the guardrail is off.
    pub fn parse(value: Option<&str>) -> (Self, Option<String>) {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("off") => (Mode::Off, None),
            Some("block") => (Mode::Block, None),
            Some("warn") | Some("") | None => (Mode::Warn, None),
            Some(_) => (
                Mode::Warn,
                Some(format!(
                    "{ENV}={:?} is not off|warn|block; running as warn.",
                    value.unwrap_or_default()
                )),
            ),
        }
    }
}

/// One published function, as the preflight reads it.
#[derive(Debug, Clone)]
pub struct LiveFunction {
    pub app_id: Uuid,
    /// `<org-slug>/<app-slug>`.
    pub app: String,
    pub project_id: Uuid,
    pub function: String,
    pub manifest: Option<serde_json::Value>,
}

/// `(app_id, function_name)`.
pub type FunctionKey = (Uuid, String);

/// Which rule refused, as a stable name — what the [`ledger`] matches on, so
/// rewording a host message never makes a carried refusal read as new.
pub mod rule {
    pub const CUSTOMER_WAREHOUSE_WRITE: &str = "customer_warehouse_write";
    pub const MANAGED_OLTP_WRITE: &str = "managed_oltp_write";
    pub const MANIFEST_PARSE: &str = "manifest_parse";
}

/// A call this binary would refuse that the function's manifest declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub app_id: Uuid,
    pub app: String,
    pub function: String,
    /// One of [`rule`].
    pub rule: &'static str,
    /// The database refused; empty for a rule about the whole manifest.
    pub database: String,
    /// The host's own message, for people. Not an identity: it may be reworded.
    pub reason: String,
}

/// A refusal before it is tied to its function: rule, database, reason.
type Broken = (&'static str, String, String);

/// What [`evaluate`] could and could not judge.
#[derive(Debug, Default)]
pub struct Evaluation {
    pub refusals: Vec<Refusal>,
    /// Every function judged, refused or not.
    pub checked: HashSet<FunctionKey>,
    /// Functions that write somewhere but whose workspace config could not be
    /// read — not judged, and reported as such.
    pub unchecked: HashSet<FunctionKey>,
}

/// The pure half: judge each function against the host's own rules.
/// `databases` maps a workspace to its configured databases; a workspace
/// missing from it could not be read, and its writing functions land in
/// `unchecked` — the preflight never refuses on data it does not have.
pub fn evaluate(
    functions: &[LiveFunction],
    databases: &HashMap<Uuid, Vec<Database>>,
) -> Evaluation {
    let mut out = Evaluation::default();
    for f in functions {
        let key = (f.app_id, f.function.clone());
        match judge_one(f, databases) {
            Some(broken) => {
                out.refusals
                    .extend(broken.into_iter().map(|broken| refusal(f, broken)));
                out.checked.insert(key);
            }
            None => {
                out.unchecked.insert(key);
            }
        }
    }
    out
}

/// The refusals for one function, or `None` when its workspace could not be read.
fn judge_one(f: &LiveFunction, databases: &HashMap<Uuid, Vec<Database>>) -> Option<Vec<Broken>> {
    let Some(manifest) = &f.manifest else {
        return Some(vec![]);
    };
    let entry: FunctionManifestEntry = match serde_json::from_value(manifest.clone()) {
        Ok(entry) => entry,
        Err(e) => {
            return Some(vec![(
                rule::MANIFEST_PARSE,
                String::new(),
                format!(
                    "its manifest no longer parses on this release ({e}), so the host would run \
                     it with no capabilities and refuse every gated ctx call"
                ),
            )]);
        }
    };
    let destinations = entry.write_destinations();
    if destinations.is_empty() {
        return Some(vec![]);
    }
    let configured = databases.get(&f.project_id)?;
    let exceptions: BTreeMap<String, String> = entry.customer_warehouse_writes();
    let mut broken = Vec::new();
    for database in destinations {
        // A name the workspace does not configure is refused at runtime too,
        // but it is the app's configuration, not something a release changes.
        let Some(db) = configured.iter().find(|d| d.name == database) else {
            continue;
        };
        let kind = destination_kind(&db.database_type);
        // `Warehouse` gives the surface-neutral text: the manifest cannot say
        // whether `ctx.warehouse` or `ctx.tx` makes the write, and only
        // `Transaction` adds a sentence of its own.
        if let Err(reason) =
            destination_write_policy(&database, kind, &exceptions, WriteSurface::Warehouse)
        {
            let rule = match kind {
                DestinationKind::ManagedOltp => rule::MANAGED_OLTP_WRITE,
                // Airhouse is never refused; it is here for exhaustiveness.
                DestinationKind::CustomerWarehouse | DestinationKind::Airhouse => {
                    rule::CUSTOMER_WAREHOUSE_WRITE
                }
            };
            broken.push((rule, database, reason));
        }
    }
    Some(broken)
}

fn refusal(f: &LiveFunction, (rule, database, reason): Broken) -> Refusal {
    Refusal {
        app_id: f.app_id,
        app: f.app.clone(),
        function: f.function.clone(),
        rule,
        database,
        reason,
    }
}

/// A refusal, judged against the ledger and the function's recent history.
#[derive(Debug, Clone)]
pub struct Judged {
    pub refusal: Refusal,
    /// Absent from the refusals the last rollout recorded: this release's doing.
    pub new: bool,
    /// New, and the function answered successfully this week.
    pub breaks_working: bool,
}

/// Everything one preflight run found.
#[derive(Debug, Default)]
pub struct Findings {
    pub judged: Vec<Judged>,
    pub checked: HashSet<FunctionKey>,
    pub unchecked: HashSet<FunctionKey>,
    /// The ledger as this run found it.
    pub known: HashSet<ledger::Entry>,
    /// The first run on this deployment ([`first_run`]): it records, never blocks.
    pub baseline: bool,
}

/// What the preflight concluded, for `oxy migrate` to print — this module
/// only decides.
#[derive(Debug, Default)]
pub struct Outcome {
    /// For the migrate Job's log; empty when the preflight is off.
    pub log: String,
    /// Set when the rollout must stop: why, and how to proceed.
    pub blocked: Option<String>,
}

/// Run the preflight as `OXY_APP_PREFLIGHT` says.
pub async fn run_from_env() -> Outcome {
    let (mode, note) = Mode::parse(std::env::var(ENV).ok().as_deref());
    run(mode, note).await
}

/// `note` names a misspelt mode. It leads the log and always reaches the
/// channel: the hook passes in `warn`, so a green Job's log is otherwise the
/// only place it would appear.
async fn run(mode: Mode, note: Option<String>) -> Outcome {
    if mode == Mode::Off {
        return Outcome::default();
    }
    let lead = |text: String| match &note {
        Some(note) => format!("{note}\n{text}"),
        None => text,
    };
    let (db, findings) = match load().await {
        Ok(found) => found,
        Err(e) => {
            // Never block on the preflight's own failure — a fresh install, a
            // table this binary reads differently. Say so loudly instead.
            tracing::warn!(target: "oxy::app_preflight", error = %e, "preflight could not run");
            let log = lead(format!(
                "custom-app preflight could not run ({e}); continuing"
            ));
            if note.is_some() {
                post(&log).await;
            }
            return Outcome { log, blocked: None };
        }
    };
    let blocking = mode == Mode::Block
        && !findings.baseline
        && findings.judged.iter().any(|j| j.breaks_working);
    let mut log = lead(report::report(&findings, blocking));
    if blocking {
        announce_block(&db, &findings, &mut log).await;
    } else {
        if let Err(e) = ledger::record(&db, &findings).await {
            tracing::warn!(target: "oxy::app_preflight", error = %e, "preflight: ledger not recorded");
            log.push_str(&format!(
                "\nThe refusals above were not recorded ({e}); the next release will report \
                 them as new."
            ));
        }
        // Only news reaches the channel; what is carried over stays in this log.
        if note.is_some() || findings.judged.iter().any(|j| j.new) {
            post(&log).await;
        }
    }
    let blocked = blocking.then(|| {
        format!(
            "custom-app preflight: this release would stop working custom-app functions — \
             rollout blocked ({ENV}=block). Republish the apps named above, or set {ENV}=warn \
             for this rollout to accept it."
        )
    });
    Outcome { log, blocked }
}

/// Tell the channel about a block until a post lands, then stop. A failed hook
/// is retried — by the Job's `backoffLimit`, then by each Argo sync retry,
/// which recreates the Job — and every attempt reruns the preflight; the log
/// counts them.
async fn announce_block(db: &DatabaseConnection, findings: &Findings, log: &mut String) {
    let told = match ledger::note_block(db, findings).await {
        Ok((attempt, told)) => {
            if attempt > 1 {
                log.push_str(&format!(
                    "\nBlocked attempt {attempt} of this release over these breaks; {}.",
                    if told {
                        "the channel has been told"
                    } else {
                        "the channel has not been told yet"
                    }
                ));
            }
            told
        }
        Err(e) => {
            // Cannot tell whether anyone was told: say it again rather than risk silence.
            tracing::warn!(target: "oxy::app_preflight", error = %e, "preflight: block not counted");
            false
        }
    };
    if !told
        && post(log).await
        && let Err(e) = ledger::mark_told(db, findings).await
    {
        tracing::warn!(target: "oxy::app_preflight", error = %e, "preflight: block not marked told");
    }
}

async fn load() -> Result<(DatabaseConnection, Findings), DbErr> {
    let db = establish_connection()
        .await
        .map_err(|e| DbErr::Custom(e.to_string()))?;
    let findings = check(&db).await?;
    Ok((db, findings))
}

/// Load every live function and its workspace's databases, then [`judge`].
pub async fn check(db: &DatabaseConnection) -> Result<Findings, DbErr> {
    let functions = live_functions(db).await?;
    let databases = workspace_databases(&functions).await;
    judge(db, &functions, &databases).await
}

/// Evaluate, then judge each refusal against the ledger and — only when it is
/// new — the function's recent history.
pub async fn judge(
    db: &impl ConnectionTrait,
    functions: &[LiveFunction],
    databases: &HashMap<Uuid, Vec<Database>>,
) -> Result<Findings, DbErr> {
    let evaluation = evaluate(functions, databases);
    let known = ledger::known(db).await?;
    let mut judged = Vec::with_capacity(evaluation.refusals.len());
    for refusal in evaluation.refusals {
        let new = !known.contains(&ledger::entry(&refusal));
        let breaks_working = new && worked_recently(db, refusal.app_id, &refusal.function).await?;
        judged.push(Judged {
            refusal,
            new,
            breaks_working,
        });
    }
    Ok(Findings {
        judged,
        checked: evaluation.checked,
        unchecked: evaluation.unchecked,
        known,
        baseline: first_run(db).await,
    })
}

/// Whether this is the preflight's first run on this deployment: the ledger's
/// migration was applied within the last [`BASELINE_HOURS`] — by this
/// `oxy migrate`, or a retry of the same rollout. With nothing recorded yet,
/// every refusal would read as new and a working app the running binary
/// already refuses would block the rollout that introduces the preflight. A
/// read that fails is not a first run: the preflight behaves as it always does.
pub async fn first_run(db: &impl ConnectionTrait) -> bool {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT applied_at > extract(epoch FROM now())::bigint - $2 AS fresh \
             FROM seaql_migrations WHERE version = $1",
            [LEDGER_MIGRATION.into(), (BASELINE_HOURS * 3600).into()],
        ))
        .await;
    match row {
        Ok(Some(row)) => row.try_get::<bool>("", "fresh").unwrap_or(false),
        Ok(None) => false,
        Err(e) => {
            tracing::warn!(target: "oxy::app_preflight", error = %e, "preflight: first-run check failed");
            false
        }
    }
}

/// Every function of every app's published build. Raw SQL over columns that
/// have been stable since functions shipped, so a release reading prod's rows
/// does not depend on its own entity shapes matching them.
pub async fn live_functions(db: &DatabaseConnection) -> Result<Vec<LiveFunction>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT a.id AS app_id, o.slug AS org_slug, a.slug AS app_slug, a.project_id, \
                    f.name AS function_name, f.manifest_json \
             FROM apps a \
             JOIN organizations o ON o.id = a.org_id \
             JOIN app_functions f ON f.build_id = a.published_build_id \
             WHERE a.published_build_id IS NOT NULL \
             ORDER BY o.slug, a.slug, f.name",
        ))
        .await?;
    rows.into_iter()
        .map(|row| {
            let org: String = row.try_get("", "org_slug")?;
            let slug: String = row.try_get("", "app_slug")?;
            Ok(LiveFunction {
                app_id: row.try_get("", "app_id")?,
                app: format!("{org}/{slug}"),
                project_id: row.try_get("", "project_id")?,
                function: row.try_get("", "function_name")?,
                manifest: row.try_get("", "manifest_json")?,
            })
        })
        .collect()
}

/// Each workspace's databases, from its compiled config — the promoted
/// revision the serve fleet reads, so no working copy is needed. A workspace
/// that cannot be read is left out, and its functions are reported unchecked.
async fn workspace_databases(functions: &[LiveFunction]) -> HashMap<Uuid, Vec<Database>> {
    let mut out = HashMap::new();
    let mut projects: Vec<Uuid> = functions.iter().map(|f| f.project_id).collect();
    projects.sort();
    projects.dedup();
    for project in projects {
        match crate::server::api::compiled_reader::resolve_workspace_config(project, None).await {
            Ok(Some(config)) => {
                let databases = config
                    .get("databases")
                    .cloned()
                    .unwrap_or(serde_json::Value::Array(vec![]));
                match serde_json::from_value::<Vec<Database>>(databases) {
                    Ok(databases) => {
                        out.insert(project, databases);
                    }
                    Err(e) => tracing::warn!(target: "oxy::app_preflight", %project, error = %e,
                        "preflight: workspace databases do not parse; its apps are unchecked"),
                }
            }
            Ok(None) => tracing::warn!(target: "oxy::app_preflight", %project,
                "preflight: workspace has no compiled config; its apps are unchecked"),
            Err(e) => tracing::warn!(target: "oxy::app_preflight", %project, error = %e,
                "preflight: could not read the workspace config; its apps are unchecked"),
        }
    }
    out
}

/// Whether the function answered successfully in the last
/// [`WORKING_WITHIN_DAYS`] days — a finished call the pager did not count as a
/// failure. Uses `idx_app_function_invocations_success`.
pub async fn worked_recently(
    db: &impl ConnectionTrait,
    app_id: Uuid,
    function: &str,
) -> Result<bool, DbErr> {
    let since = (Utc::now() - Duration::days(WORKING_WITHIN_DAYS)).fixed_offset();
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT EXISTS (SELECT 1 FROM app_function_invocations \
               WHERE app_id = $1 AND function_name = $2 AND created_at >= $3 \
                 AND status = 'success' AND failure_fingerprint IS NULL) AS worked",
            [app_id.into(), function.into(), since.into()],
        ))
        .await?;
    Ok(match row {
        Some(row) => row.try_get::<bool>("", "worked")?,
        None => false,
    })
}

/// Best effort: the preflight's verdict never depends on Slack. Says whether
/// the post landed, so a block keeps trying until someone has been told.
async fn post(text: &str) -> bool {
    let Some((token, channel)) = super::failure_page::ops_slack_target() else {
        return false;
    };
    let client = oxy_slack_client::SlackClient::new();
    let post = client.chat_post_message(&token, &channel, text, None);
    match tokio::time::timeout(std::time::Duration::from_secs(10), post).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => {
            tracing::warn!(target: "oxy::app_preflight", error = %e, "preflight: Slack post failed");
            false
        }
        Err(_) => {
            tracing::warn!(target: "oxy::app_preflight", "preflight: Slack post timed out");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn databases(yaml: &str) -> Vec<Database> {
        serde_yaml::from_str(yaml).expect("database config")
    }

    /// The Poke House shape: a customer ClickHouse and a managed Airhouse.
    fn poke_house() -> Vec<Database> {
        databases(
            "- name: clickhouse\n  type: clickhouse\n  host_var: CLICKHOUSE_HOST\n  \
             user_var: CLICKHOUSE_USERNAME\n  password_var: CLICKHOUSE_PASSWORD\n  \
             database_var: CLICKHOUSE_DATABASE\n\
             - name: pokehouse\n  type: airhouse_managed\n",
        )
    }

    fn function(manifest: serde_json::Value) -> (LiveFunction, HashMap<Uuid, Vec<Database>>) {
        let project_id = Uuid::new_v4();
        let f = LiveFunction {
            app_id: Uuid::new_v4(),
            app: "poke-house/warehouse".into(),
            project_id,
            function: "submit-receiving".into(),
            manifest: Some(manifest),
        };
        (f, HashMap::from([(project_id, poke_house())]))
    }

    #[test]
    fn the_incident_a_customer_warehouse_write_without_a_reason_is_refused() {
        let (f, dbs) = function(serde_json::json!({ "destinations": ["clickhouse"] }));
        let refusals = evaluate(&[f], &dbs).refusals;
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(
            refusals[0].reason.contains("customerWarehouseWrites"),
            "the reason must name the fix, as the host's does: {}",
            refusals[0].reason
        );
        assert_eq!(
            (refusals[0].rule, refusals[0].database.as_str()),
            (rule::CUSTOMER_WAREHOUSE_WRITE, "clickhouse"),
            "the ledger's identity is the rule and the database, not the prose"
        );
    }

    #[test]
    fn a_declared_reason_or_airhouse_passes() {
        let (declared, dbs) = function(serde_json::json!({
            "destinations": ["clickhouse"],
            "customerWarehouseWrites": { "clickhouse": "receiving reports land here" }
        }));
        let evaluation = evaluate(&[declared], &dbs);
        assert!(evaluation.refusals.is_empty());
        assert_eq!(evaluation.checked.len(), 1, "passing is being checked");

        let (airhouse, dbs) = function(serde_json::json!({ "destinations": ["pokehouse"] }));
        assert!(evaluate(&[airhouse], &dbs).refusals.is_empty());
    }

    #[test]
    fn a_function_that_writes_nowhere_or_has_no_manifest_is_checked_and_clean() {
        let (none, dbs) = function(serde_json::json!({}));
        let evaluation = evaluate(&[none], &dbs);
        assert!(evaluation.refusals.is_empty() && evaluation.checked.len() == 1);
        let (mut no_manifest, dbs) = function(serde_json::json!({}));
        no_manifest.manifest = None;
        let evaluation = evaluate(&[no_manifest], &dbs);
        assert!(evaluation.refusals.is_empty() && evaluation.checked.len() == 1);
    }

    #[test]
    fn an_unreadable_workspace_is_unchecked_not_refused_or_passed() {
        let (f, _) = function(serde_json::json!({ "destinations": ["clickhouse"] }));
        let evaluation = evaluate(&[f], &HashMap::new());
        assert!(evaluation.refusals.is_empty(), "no data is no refusal");
        assert!(evaluation.checked.is_empty(), "and not a pass either");
        assert_eq!(evaluation.unchecked.len(), 1);

        let (g, dbs) = function(serde_json::json!({ "destinations": ["not_there"] }));
        let evaluation = evaluate(&[g], &dbs);
        assert!(
            evaluation.refusals.is_empty() && evaluation.checked.len() == 1,
            "an unconfigured name is the app's configuration, not the release's doing"
        );
    }

    #[test]
    fn a_manifest_this_release_cannot_parse_is_refused() {
        // `destinations` must be a list; a release that tightened the shape
        // would read this app's manifest as no capabilities at all.
        let (f, dbs) = function(serde_json::json!({ "destinations": "clickhouse" }));
        let refusals = evaluate(&[f], &dbs).refusals;
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(
            refusals[0].reason.contains("no longer parses"),
            "{}",
            refusals[0].reason
        );
        assert_eq!(
            (refusals[0].rule, refusals[0].database.as_str()),
            (rule::MANIFEST_PARSE, "")
        );
    }

    #[test]
    fn mode_defaults_to_warn_and_a_typo_never_blocks_but_says_so() {
        assert_eq!(Mode::parse(None), (Mode::Warn, None));
        assert_eq!(Mode::parse(Some("")), (Mode::Warn, None));
        assert_eq!(Mode::parse(Some(" BLOCK ")), (Mode::Block, None));
        assert_eq!(Mode::parse(Some("off")), (Mode::Off, None));
        let (mode, note) = Mode::parse(Some("blokc"));
        assert_eq!(mode, Mode::Warn);
        assert!(
            note.as_deref()
                .is_some_and(|n| n.contains("\"blokc\"") && n.contains("running as warn")),
            "{note:?}"
        );
    }
}
