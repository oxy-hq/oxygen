//! Before a release serves, report every working custom-app function it would
//! stop, judged by THIS binary's host rules against the deployment's real apps.
//!
//! **Why.** In Sep 2026, #3187 made customer warehouses read-only to apps
//! unless a function declares `customerWarehouseWrites`. One app was not
//! republished, and 0.5.147 refused its writes for a week. The new rule and the
//! live manifest were both in prod's database before a pod rolled.
//!
//! **Where.** `oxy migrate`, which the chart runs as a pre-upgrade hook with the
//! NEW image against the deployment's database: the one place the next binary
//! meets the real apps before users do. It reports to the custom-app channel
//! and the Job's log. It never fails the hook.
//!
//! **What.** Each database a function names in `destinations` must pass
//! [`destination_write_policy`], the host's own check, and the manifest must
//! still parse, since the runtime falls back to no capabilities when it cannot.
//! Only functions that answered in the last [`WORKING_WITHIN_DAYS`] days are
//! reported: those are the ones a release can stop.
//!
//! **Tightening a host gate?** Add its rule here, or this cannot see the apps
//! your change breaks.

use std::collections::{BTreeMap, HashMap};

use chrono::{Duration, Utc};
use oxy::config::model::Database;
use oxy::database::client::establish_connection;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use uuid::Uuid;

use super::FunctionManifestEntry;
use super::host::{WriteSurface, destination_kind, destination_write_policy};

/// A function counts as working when it answered this recently.
pub const WORKING_WITHIN_DAYS: i64 = 7;
/// Refusals listed in the message before it summarises the rest.
const LISTED: usize = 15;

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

/// A call this binary would refuse that the function's manifest declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub app_id: Uuid,
    pub app: String,
    pub function: String,
    pub reason: String,
}

/// Run the preflight and return what it found, for `oxy migrate` to print.
/// Posts to the custom-app channel when a working function would be stopped.
/// Never fails: a preflight that cannot run says so and the rollout goes on.
pub async fn run() -> String {
    match check().await {
        Ok((refusals, unchecked)) => {
            let text = report(&refusals, unchecked);
            if !refusals.is_empty() {
                post(&text).await;
            }
            text
        }
        Err(e) => format!("custom-app preflight could not run ({e}); continuing"),
    }
}

/// The refusals on working functions, and how many functions could not be
/// checked because their workspace config could not be read.
async fn check() -> Result<(Vec<Refusal>, usize), DbErr> {
    let db = establish_connection()
        .await
        .map_err(|e| DbErr::Custom(e.to_string()))?;
    let functions = live_functions(&db).await?;
    let databases = workspace_databases(&functions).await;
    let (refusals, unchecked) = evaluate(&functions, &databases);
    let mut working = Vec::new();
    for refusal in refusals {
        if worked_recently(&db, refusal.app_id, &refusal.function).await? {
            working.push(refusal);
        }
    }
    Ok((working, unchecked))
}

/// The pure half: every refusal this binary's host rules would make, and how
/// many writing functions were skipped because their workspace is missing
/// from `databases`. The preflight never refuses on data it does not have.
pub fn evaluate(
    functions: &[LiveFunction],
    databases: &HashMap<Uuid, Vec<Database>>,
) -> (Vec<Refusal>, usize) {
    let mut refusals = Vec::new();
    let mut unchecked = 0;
    for f in functions {
        let Some(manifest) = &f.manifest else {
            continue;
        };
        let refuse = |reason: String| Refusal {
            app_id: f.app_id,
            app: f.app.clone(),
            function: f.function.clone(),
            reason,
        };
        let entry: FunctionManifestEntry = match serde_json::from_value(manifest.clone()) {
            Ok(entry) => entry,
            Err(e) => {
                refusals.push(refuse(format!(
                    "its manifest no longer parses on this release ({e}), so the host would run \
                     it with no capabilities and refuse every gated ctx call"
                )));
                continue;
            }
        };
        let destinations = entry.write_destinations();
        if destinations.is_empty() {
            continue;
        }
        let Some(configured) = databases.get(&f.project_id) else {
            unchecked += 1;
            continue;
        };
        let exceptions: BTreeMap<String, String> = entry.customer_warehouse_writes();
        for database in destinations {
            // A name the workspace does not configure is the app's
            // configuration, not something a release changes.
            let Some(db) = configured.iter().find(|d| d.name == database) else {
                continue;
            };
            // `Warehouse` gives the surface-neutral text; only `Transaction`
            // adds a sentence, and the manifest cannot say which surface writes.
            if let Err(reason) = destination_write_policy(
                &database,
                destination_kind(&db.database_type),
                &exceptions,
                WriteSurface::Warehouse,
            ) {
                refusals.push(refuse(reason));
            }
        }
    }
    (refusals, unchecked)
}

/// The message for the migrate Job's log and the custom-app channel.
pub fn report(refusals: &[Refusal], unchecked: usize) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let mut out = if refusals.is_empty() {
        format!("custom-app preflight: release {version} stops no working custom-app function.")
    } else {
        format!(
            ":warning: Release {version} would refuse {} call(s) that working custom-app \
             functions make:",
            refusals.len()
        )
    };
    for r in refusals.iter().take(LISTED) {
        let reason: String = r.reason.chars().take(220).collect();
        out.push_str(&format!("\n• `{}` · `{}` — {reason}", r.app, r.function));
    }
    if refusals.len() > LISTED {
        out.push_str(&format!(
            "\n…and {} more in the migrate Job's log.",
            refusals.len() - LISTED
        ));
    }
    if unchecked > 0 {
        out.push_str(&format!(
            "\nNot checked: {unchecked} function(s) whose workspace config could not be read."
        ));
    }
    if !refusals.is_empty() {
        out.push_str(
            "\nFix: republish each app with the manifest change its reason names, before this \
             release reaches users. internal-docs/custom-app-availability-guidelines.md",
        );
    }
    out
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

/// Each workspace's databases, from its compiled config: the promoted revision
/// the serve fleet reads, so no working copy is needed. A workspace that
/// cannot be read is left out, and its functions count as unchecked.
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
/// [`WORKING_WITHIN_DAYS`] days: a finished call the pager did not count as a
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

/// Best effort: a Slack failure is logged, never raised.
async fn post(text: &str) {
    let Some((token, channel)) = super::failure_page::ops_slack_target() else {
        return;
    };
    let client = oxy_slack_client::SlackClient::new();
    let post = client.chat_post_message(&token, &channel, text, None);
    match tokio::time::timeout(std::time::Duration::from_secs(10), post).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            tracing::warn!(target: "oxy::app_preflight", error = %e, "preflight: Slack post failed")
        }
        Err(_) => tracing::warn!(target: "oxy::app_preflight", "preflight: Slack post timed out"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Poke House shape: a customer ClickHouse and a managed Airhouse.
    fn poke_house() -> Vec<Database> {
        serde_yaml::from_str(
            "- name: clickhouse\n  type: clickhouse\n  host_var: CLICKHOUSE_HOST\n  \
             user_var: CLICKHOUSE_USERNAME\n  password_var: CLICKHOUSE_PASSWORD\n  \
             database_var: CLICKHOUSE_DATABASE\n\
             - name: pokehouse\n  type: airhouse_managed\n",
        )
        .expect("database config")
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
        let (refusals, _) = evaluate(&[f], &dbs);
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(
            refusals[0].reason.contains("customerWarehouseWrites"),
            "the reason must name the fix, as the host's does: {}",
            refusals[0].reason
        );
    }

    #[test]
    fn a_declared_reason_airhouse_or_no_destinations_passes() {
        for manifest in [
            serde_json::json!({
                "destinations": ["clickhouse"],
                "customerWarehouseWrites": { "clickhouse": "receiving reports land here" }
            }),
            serde_json::json!({ "destinations": ["pokehouse"] }),
            serde_json::json!({}),
            serde_json::json!({ "destinations": ["not_configured"] }),
        ] {
            let (f, dbs) = function(manifest.clone());
            assert_eq!(evaluate(&[f], &dbs), (vec![], 0), "{manifest}");
        }
    }

    #[test]
    fn a_manifest_this_release_cannot_parse_is_refused() {
        let (f, dbs) = function(serde_json::json!({ "destinations": "clickhouse" }));
        let (refusals, _) = evaluate(&[f], &dbs);
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(refusals[0].reason.contains("no longer parses"));
    }

    #[test]
    fn an_unreadable_workspace_is_counted_not_refused() {
        let (f, _) = function(serde_json::json!({ "destinations": ["clickhouse"] }));
        assert_eq!(evaluate(&[f], &HashMap::new()), (vec![], 1));
    }

    #[test]
    fn the_report_names_the_function_and_the_fix() {
        let (f, dbs) = function(serde_json::json!({ "destinations": ["clickhouse"] }));
        let (refusals, _) = evaluate(&[f], &dbs);
        let text = report(&refusals, 2);
        assert!(
            text.contains("`poke-house/warehouse` · `submit-receiving`"),
            "{text}"
        );
        assert!(text.contains("republish"), "{text}");
        assert!(text.contains("Not checked: 2"), "{text}");
        assert!(report(&[], 0).contains("stops no working custom-app function"));
    }
}
