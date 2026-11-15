//! P2 detection: which automations a branch changed, and which of those are
//! pure-Airhouse transforms a preview builds on its own.
//!
//! * **Changed**: the staging revision's `automation_definitions.definition`
//!   differs from the promoted revision's at the same path (or has none), or a
//!   `sql_file` it names has different `verified_queries.content`. References
//!   are walked from the definition JSON, not `compiled_references`.
//! * **Auto-build**: every task — recursively through `conditional` and
//!   `loop_sequential` — is `execute_sql` on the workspace's managed Airhouse,
//!   `formatter` or `conditional`, and at least one SQL body keyword-matches a
//!   write. The match only detects; the step rewrite is what keeps a build in
//!   the preview. A build runs with no variables, so one that needs a value
//!   nothing provides is manual too ([`super::transform_vars`]).
//! * Everything else changed is **manual**, with the first reason found.
//!
//! An `airhouse` database with credentials of its own is manual: preview
//! schemas live only in the workspace's managed Airhouse tenant, so its writes
//! would be held and the build would compare nothing.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::transform_vars::unset_variables;

/// A write keyword anywhere in a SQL body.
static WRITES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(CREATE|INSERT|DELETE|MERGE|UPDATE|DROP|ALTER|TRUNCATE)\b")
        .expect("static regex")
});

/// One automation the branch changed, and how the preview treats it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransformReport {
    pub name: String,
    pub file_path: String,
    /// `added` | `modified`.
    pub change: String,
    /// `auto` (built in the preview and compared with live) | `manual`.
    pub build: String,
    /// Why it is manual, or why an auto build was not queued.
    pub reason: Option<String>,
    /// The `transform_build` run queued for it.
    pub build_run_id: Option<String>,
}

/// How the preview treats a changed automation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Build {
    Auto,
    Manual(String),
}

/// The staging revision's databases: name → config `type`.
pub type DatabaseTypes = HashMap<String, String>;

/// Whether `definition` is a pure-Airhouse transform. `sql_file` answers a
/// referenced file's body in the staging revision.
pub fn classify(
    definition: &Value,
    databases: &DatabaseTypes,
    sql_file: &dyn Fn(&str) -> Option<String>,
) -> Build {
    let mut bodies = Vec::new();
    let tasks = definition.get("tasks").unwrap_or(&Value::Null);
    if let Err(why) = walk(tasks, databases, sql_file, &mut bodies) {
        return Build::Manual(why);
    }
    let unset = unset_variables(definition, &bodies);
    if !unset.is_empty() {
        return Build::Manual(format!(
            "needs variables: {} (no value a build could use: declared without a default, or \
             referenced by the SQL and never declared; run it by hand with values)",
            unset.join(", ")
        ));
    }
    if bodies.iter().any(|sql| WRITES.is_match(sql)) {
        Build::Auto
    } else {
        Build::Manual("writes nothing: no SQL creates, inserts, updates or drops".into())
    }
}

/// Check every task of `tasks`, collecting the SQL bodies; the first task
/// that is not pure Airhouse is the reason.
fn walk(
    tasks: &Value,
    databases: &DatabaseTypes,
    sql_file: &dyn Fn(&str) -> Option<String>,
    bodies: &mut Vec<String>,
) -> Result<(), String> {
    for task in tasks.as_array().into_iter().flatten() {
        match task.get("type").and_then(Value::as_str).unwrap_or_default() {
            "execute_sql" => bodies.push(airhouse_sql(task, databases, sql_file)?),
            "formatter" => {}
            "conditional" => {
                for branch in task
                    .get("conditions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    walk(
                        branch.get("tasks").unwrap_or(&Value::Null),
                        databases,
                        sql_file,
                        bodies,
                    )?;
                }
                walk(
                    task.get("else").unwrap_or(&Value::Null),
                    databases,
                    sql_file,
                    bodies,
                )?;
            }
            "loop_sequential" => walk(
                task.get("tasks").unwrap_or(&Value::Null),
                databases,
                sql_file,
                bodies,
            )?,
            other => return Err(manual_step(other)),
        }
    }
    Ok(())
}

fn manual_step(task_type: &str) -> String {
    match task_type {
        "airway" => "calls an airway step".into(),
        "agent" => "calls an agent".into(),
        "workflow" => "calls another automation".into(),
        "http_request" => "sends an HTTP request".into(),
        "" => "has a step with no type".into(),
        other => format!("has a `{other}` step"),
    }
}

/// An `execute_sql` task's SQL, when it runs on the managed Airhouse.
fn airhouse_sql(
    task: &Value,
    databases: &DatabaseTypes,
    sql_file: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let database = task
        .get("database")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let sql = task_sql(task, sql_file)?;
    match databases.get(database).map(String::as_str) {
        Some("airhouse_managed") => Ok(sql),
        Some("airhouse") => Err(format!(
            "runs SQL on `{database}`, an `airhouse` database with its own credentials (only \
             the workspace's managed Airhouse has preview schemas)"
        )),
        Some(kind) => Err(format!(
            "{} {}",
            if WRITES.is_match(&sql) {
                "writes"
            } else {
                "reads"
            },
            display_name(kind)
        )),
        None => Err(format!(
            "names database `{database}`, which the branch's config.yml does not have"
        )),
    }
}

fn task_sql(task: &Value, sql_file: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    if let Some(sql) = task.get("sql_query").and_then(Value::as_str) {
        return Ok(sql.to_string());
    }
    let Some(path) = task.get("sql_file").and_then(Value::as_str) else {
        return Err("has an execute_sql step with no SQL".into());
    };
    if path.contains("{{") {
        return Err(format!(
            "its sql_file `{path}` is templated, so what it runs is known only at run time"
        ));
    }
    sql_file(path).ok_or_else(|| format!("its sql_file `{path}` is not in the revision"))
}

fn display_name(kind: &str) -> String {
    match kind {
        "clickhouse" => "ClickHouse",
        "bigquery" => "BigQuery",
        "snowflake" => "Snowflake",
        "postgres" => "Postgres",
        "postgres_managed" => "the managed Postgres",
        "redshift" => "Redshift",
        "mysql" => "MySQL",
        "duckdb" => "DuckDB",
        "motherduck" => "MotherDuck",
        "domo" => "DOMO",
        other => other,
    }
    .to_string()
}

/// Every `sql_file` a definition names, at any depth.
pub fn sql_files(definition: &Value) -> Vec<String> {
    fn collect(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(path)) = map.get("sql_file") {
                    out.push(path.clone());
                }
                map.values().for_each(|v| collect(v, out));
            }
            Value::Array(items) => items.iter().for_each(|v| collect(v, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    collect(definition, &mut out);
    out.sort();
    out.dedup();
    out
}

#[derive(Debug, FromQueryResult)]
struct AutomationRow {
    name: String,
    file_path: String,
    branch_def: Value,
    live_def: Option<Value>,
}

const AUTOMATIONS_SQL: &str = "\
    SELECT s.name, s.file_path, s.definition AS branch_def, m.definition AS live_def \
    FROM automation_definitions s \
    LEFT JOIN automation_definitions m ON m.revision_id = $2 AND m.file_path = s.file_path \
    WHERE s.revision_id = $1 ORDER BY s.file_path";

/// `.sql` files whose content the branch changed (or added).
const CHANGED_SQL_FILES: &str = "\
    SELECT s.file_path FROM verified_queries s \
    LEFT JOIN verified_queries m ON m.revision_id = $2 AND m.file_path = s.file_path \
    WHERE s.revision_id = $1 AND (m.file_path IS NULL OR m.content IS DISTINCT FROM s.content)";

/// Every automation `staging` changed against `promoted`, classified.
pub async fn detect<C: ConnectionTrait>(
    db: &C,
    staging: Uuid,
    promoted: Option<Uuid>,
    databases: &DatabaseTypes,
) -> Result<Vec<TransformReport>, DbErr> {
    let values = || vec![staging.into(), promoted.into()];
    let automations = AutomationRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        AUTOMATIONS_SQL,
        values(),
    ))
    .all(db)
    .await?;
    let changed_files: HashSet<String> = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            CHANGED_SQL_FILES,
            values(),
        ))
        .await?
        .iter()
        .map(|r| r.try_get("", "file_path"))
        .collect::<Result<_, _>>()?;
    let mut reports = Vec::new();
    for row in automations {
        let files = sql_files(&row.branch_def);
        let edited = row.live_def.as_ref() != Some(&row.branch_def)
            || files.iter().any(|f| changed_files.contains(f));
        if !edited {
            continue;
        }
        let bodies = sql_bodies(db, staging, &files).await?;
        let build = classify(&row.branch_def, databases, &|p| bodies.get(p).cloned());
        reports.push(report(&row, build));
    }
    Ok(reports)
}

async fn sql_bodies<C: ConnectionTrait>(
    db: &C,
    revision: Uuid,
    files: &[String],
) -> Result<HashMap<String, String>, DbErr> {
    let mut bodies = HashMap::new();
    for file in files.iter().filter(|f| !f.contains("{{")) {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT content FROM verified_queries WHERE revision_id = $1 AND file_path = $2",
                [revision.into(), file.clone().into()],
            ))
            .await?;
        if let Some(row) = row {
            bodies.insert(file.clone(), row.try_get("", "content")?);
        }
    }
    Ok(bodies)
}

fn report(row: &AutomationRow, build: Build) -> TransformReport {
    let (build, reason) = match build {
        Build::Auto => ("auto", None),
        Build::Manual(why) => ("manual", Some(why)),
    };
    TransformReport {
        name: row.name.clone(),
        file_path: row.file_path.clone(),
        change: if row.live_def.is_some() {
            "modified"
        } else {
            "added"
        }
        .into(),
        build: build.into(),
        reason,
        build_run_id: None,
    }
}

/// Name → `type` of a revision's compiled `databases` (the
/// `workspace_compiled_configs.databases` array).
pub fn database_types(databases: &Value) -> DatabaseTypes {
    databases
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|db| {
            Some((
                db.get("name")?.as_str()?.to_string(),
                db.get("type")?.as_str()?.to_string(),
            ))
        })
        .collect()
}
