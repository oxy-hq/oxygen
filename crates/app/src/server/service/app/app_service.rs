use super::cache::AppCache;
use super::controls::{control_values, declared_controls};
use super::types::{AppResult, TASKS_KEY};
use crate::agentic_wiring::OxyProjectContext;
use crate::server::api::middlewares::workspace_context::PreaggCacheCtx;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::model::{AppConfig, Task};
use oxy::config::{ConfigManager, DiskSlot, ResolveWorkspaceFile, WorkingCopy};
use oxy::exec_types::{Data, DataContainer, TableData};
use oxy_shared::errors::OxyError;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub struct AppService<S = WorkingCopy> {
    workspace_manager: WorkspaceManager<S>,
    cache: AppCache<S>,
    /// The node's Layer-1 rollup short-circuit, when the caller has one. Only
    /// `run` reads it: a Data App's tasks must resolve rollups the same way
    /// `/semantic-query` does. Default-empty, which resolves no rollup and
    /// compiles every task to warehouse SQL — correct, just slower.
    preagg: PreaggCacheCtx,
}

impl<S: DiskSlot + Send + Sync> AppService<S>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    /// The `OxyProjectContext` this used to take was stored only for the
    /// preagg fields `run` needs, which now arrive on their own via
    /// [`Self::with_preagg`] — carrying a whole context pinned the service to
    /// `WorkingCopy`, so a route whose entire job is to serve a cached answer
    /// when the ide is down required a disk to say so.
    ///
    /// A `branch_hint` went the same way. The manager already carries the
    /// `Origin` the middleware resolved — branch gate included — so a second
    /// copy of the branch could only agree with it or drift from it.
    pub fn new(workspace_manager: WorkspaceManager<S>) -> Self {
        let config_manager = workspace_manager.config_manager.clone();
        let workspace_id = workspace_manager.workspace_id;
        Self {
            workspace_manager,
            cache: AppCache::new(config_manager, workspace_id),
            preagg: PreaggCacheCtx::default(),
        }
    }

    /// Attach the request's rollup short-circuit, so tasks this service runs
    /// answer from a rollup wherever `/semantic-query` would. Callers that
    /// only read cached data need not bother.
    pub fn with_preagg(mut self, preagg: PreaggCacheCtx) -> Self {
        self.preagg = preagg;
        self
    }

    pub async fn get_config(&self, app_path: &PathBuf) -> AppResult<AppConfig> {
        Ok(self
            .workspace_manager
            .config_manager
            .resolve_app(app_path)
            .await?)
    }

    pub async fn get_tasks(&self, app_path: &PathBuf) -> AppResult<Vec<Task>> {
        let path_str = app_path.to_string_lossy().to_string();
        // `app_definition` owns the compiled-vs-disk choice AND the fallback,
        // so an `Err` is "no source", not "not compiled". `unwrap_or(None)`
        // swallowed a DB fault into a miss and fell through to
        // `read_yaml_file` — which on a replica reads a workspace directory
        // that is not on the node. Same defect `list_monitors` documents.
        if let Some(definition) = self
            .workspace_manager
            .config_manager
            .app_definition(&path_str)
            .await
            .map_err(|e| OxyError::RuntimeError(format!("{path_str}: {e}")))?
            && let Some(tasks_value) = definition.as_object().and_then(|m| m.get(TASKS_KEY))
        {
            return serde_json::from_value(tasks_value.clone()).map_err(|e| {
                OxyError::ConfigurationError(format!("Failed to parse compiled tasks: {e}")).into()
            });
        }

        let yaml_content = self.read_yaml_file(app_path).await?;
        let root_map = self.parse_yaml_to_mapping(&yaml_content)?;

        let tasks_value = root_map
            .get(serde_yaml::Value::String(TASKS_KEY.to_string()))
            .ok_or_else(|| {
                OxyError::ConfigurationError("No tasks found in app config".to_string())
            })?;

        serde_yaml::from_value(tasks_value.clone())
            .map_err(|e| OxyError::ConfigurationError(format!("Failed to parse tasks: {e}")))
    }

    pub async fn try_load_cached_data(
        &self,
        app_path: &PathBuf,
        tasks: &[Task],
    ) -> Option<DataContainer> {
        self.cache.try_load_data(app_path, tasks).await
    }

    pub async fn read_yaml_file(&self, path: &PathBuf) -> AppResult<String> {
        read_app_yaml_file(&self.workspace_manager, path).await
    }

    fn parse_yaml_to_mapping(&self, yaml_content: &str) -> AppResult<serde_yaml::Mapping> {
        let yaml_value: serde_yaml::Value = serde_yaml::from_str(yaml_content)
            .map_err(|e| OxyError::ConfigurationError(format!("Failed to parse YAML: {e}")))?;

        match yaml_value {
            serde_yaml::Value::Mapping(map) => Ok(map),
            _ => Err(OxyError::ConfigurationError(
                "Expected YAML object at root".to_string(),
            )),
        }
    }
}

/// Running an app's automation inline needs an `OxyProjectContext`, which is
/// pinned to a working copy — so this one method is not generic. That is the
/// right shape: executing is the thing that genuinely needs the ide, and the
/// reads around it (`get_config`, `get_tasks`, `try_load_cached_data`) stay
/// available on any replica.
impl AppService<WorkingCopy> {
    pub async fn run(
        &mut self,
        app_path: &PathBuf,
        params: HashMap<String, JsonValue>,
    ) -> AppResult<DataContainer> {
        tracing::info!("Running app: {app_path:?}");

        let config = self.get_config(app_path).await?;

        // The author's defaults, overridden by what the viewer sent. A viewer's
        // value is data; only the author's `default:` is a template.
        let controls = control_values(&declared_controls(&config), &params);

        // Reuse the already-parsed config instead of re-reading and re-parsing the YAML file.
        let tasks = config.tasks;

        let has_params = !params.is_empty();
        if !has_params {
            self.cache.clean_up_data(app_path, &tasks).await?;
        }

        // Carry the caller's rollup short-circuit onto the context the
        // automation runs under. A Data App's tasks go through
        // `step_executor::execute_semantic_query`, which asks the workspace
        // context for a `PreaggContext`; a bare `OxyProjectContext::new` has
        // none, so every Data App query compiled to warehouse SQL while the
        // same measure asked over `/semantic-query` came from a rollup. Only
        // the preagg fields travel — no subject/role, so the app's connector
        // identity stays what this path has always used.
        //
        // Held as a `PreaggCacheCtx` rather than as a concrete threshold: its
        // `renewal_threshold_secs` is an `Option`, so "nobody set one" stays
        // expressible and `with_preagg`'s config fallback still fires. A
        // concrete `u64` here would pin this path to the 120s default — the
        // exact desync that fallback exists to prevent.
        let workspace: Arc<dyn agentic_automation::WorkspaceContext> = Arc::new(
            OxyProjectContext::new(self.workspace_manager.clone()).with_preagg(&self.preagg),
        );
        let results = run_tasks(workspace.as_ref(), &tasks, controls).await?;

        // Build the cache-layer `DataContainer` directly. The frontend
        // (`AppPreview` / `registerFromTableData` in
        // `web-app/src/components/AppPreview/Displays/utils.ts`) expects
        // tabular task results in `{file_path, json}` shape with `json`
        // as an array-of-objects string — that's what
        // `Data::Table(TableData { ... })` serialises to.
        //
        // The agentic-automation runner produces tabular results as
        // `{columns, rows}` (the standard step-executor output);
        // `automation_task_to_data` converts that to array-of-objects
        // and wraps in `TableData`. Non-tabular results fall through
        // as `Data::Text(stringified)`.
        //
        // Bypasses the legacy `OutputContainer → to_data` path
        // entirely — that path expects arrow batches to write parquet
        // shards, which we don't have here.
        let mut map: HashMap<String, DataContainer> = HashMap::with_capacity(results.len());
        for task in &tasks {
            if let Some(value) = results.get(&task.name) {
                let data = automation_task_to_data(&task.name, value);
                map.insert(task.name.clone(), DataContainer::Single(data));
            } else {
                // Automation runner dropped this task — typically means it
                // errored mid-run (an inline-automation step exception clears
                // the slot before `results` is assembled) or the task name
                // doesn't match the runner's result key. Log so an operator
                // can correlate with the bundle-side "task missing from app
                // response" error.
                tracing::warn!(
                    app_path = ?app_path,
                    task = %task.name,
                    "task produced no result during inline automation run; \
                     omitting from data map"
                );
            }
        }
        let data_container = DataContainer::Map(map);

        if has_params {
            let data = self
                .cache
                .convert_to_data_container(app_path, &tasks, &params, data_container)
                .await?;
            return Ok(data);
        }

        let data = self
            .cache
            .save_data_container(app_path, &tasks, data_container)
            .await?;
        Ok(data)
    }
}

/// Run an app's tasks, with `controls` as the `controls.*` values their
/// templates read. Returns each task's result by task name.
///
/// The tasks are the templates and `controls` is their data: a control value
/// is substituted into task SQL as the characters it holds and is never
/// itself rendered.
pub(super) async fn run_tasks(
    workspace: &dyn agentic_automation::WorkspaceContext,
    tasks: &[Task],
    controls: HashMap<String, JsonValue>,
) -> AppResult<HashMap<String, JsonValue>> {
    // Data Apps run a free-form `Vec<oxy::Task>` (from `.app.yml`)
    // synchronously and need the final results back to render charts /
    // tables on the same request. We feed the tasks through
    // `agentic_pipeline::automation_run::run_inline_automation`, which drives
    // the automation decider in-process without the coordinator queue —
    // round-trip the tasks through JSON to land on
    // `agentic_automation::AutomationConfig`. Both `Task` types use
    // `#[serde(tag = "type")]` so the shapes line up.
    let automation_value = serde_json::json!({
        "name": "app-tasks-inline",
        "tasks": serde_json::to_value(tasks).map_err(|e| {
            OxyError::RuntimeError(format!("serialize app tasks: {e}"))
        })?,
    });
    let automation_config: agentic_automation::AutomationConfig =
        serde_json::from_value(automation_value).map_err(|e| {
            OxyError::RuntimeError(format!("convert app tasks → AutomationConfig: {e}"))
        })?;

    // Seed the automation runner's render context with `controls.*` so
    // task SQL templates like `{{ controls.store }}` resolve. The
    // `variables` parameter lands in `state.variables` only — NOT
    // in `state.render_context`, which is what `merge_sql_variables`
    // and `render_jinja_string` actually read. Without the
    // dedicated render-context entry point, Jinja substituted
    // empty strings and produced invalid SQL like
    // `WHERE Store = GROUP BY ...`. Surfaced while wiring the
    // customer-apps data-products endpoint, where param-driven
    // dashboards depend on controls hydrating from query-string
    // params.
    let render_context = if controls.is_empty() {
        None
    } else {
        Some(serde_json::json!({ "controls": controls }))
    };

    // `OxyInlineAgentRunner` was removed on main; agent fan-out
    // tasks inside a Data App automation now surface a clear error
    // from the automation runner instead of executing inline. The
    // common case (`execute_sql` tasks driven by Data App
    // controls) doesn't depend on inline-agent execution, so
    // a `None` runner is correct for current customer-apps usage.
    agentic_pipeline::automation_run::run_inline_automation_with_render_context(
        workspace,
        automation_config,
        None,
        render_context,
        None,
    )
    .await
    .map_err(|e| OxyError::RuntimeError(format!("app inline automation: {e}")))
}

pub async fn read_app_yaml_file<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    path: &PathBuf,
) -> AppResult<String>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    let config_manager = &workspace_manager.config_manager;
    let path_ref = path.to_string_lossy();
    let full_path = config_manager
        .try_resolve_file(&path_ref)
        .await
        .map_err(|e| {
            tracing::debug!("Failed to resolve file: {:?} {}", path, e);
            OxyError::ConfigurationError(format!("Failed to resolve file: {e}"))
        })?;

    std::fs::read_to_string(&full_path).map_err(|e| {
        tracing::info!("Failed to read file: {:?}", e);
        OxyError::ConfigurationError(format!("Failed to read file: {e}"))
    })
}

/// Convert an inline-automation task result into a [`Data`] suitable for
/// the frontend's `AppPreview` consumer.
///
/// Two shapes coming out of the agentic-automation runner:
/// - **Tabular** — `{columns: [...], rows: [[...]]}` from `execute_sql`,
///   `semantic_query`, `omni_query`. Reshape to array-of-objects JSON
///   and wrap in [`Data::Table`] with a synthetic `file_path`. The
///   frontend reads `tableData.json` directly into DuckDB-WASM; the
///   path is only used as a stable view name.
/// - **Anything else** — stringify and wrap in [`Data::Text`]. Covers
///   formatter / agent outputs.
fn automation_task_to_data(task_name: &str, value: &JsonValue) -> Data {
    if let Some(records) = tabular_to_records(value) {
        let json = serde_json::to_string(&records).ok();
        let file_path = PathBuf::from(format!("app_inline/{task_name}.parquet"));
        return Data::Table(TableData { file_path, json });
    }
    match value {
        JsonValue::String(s) => Data::Text(s.clone()),
        other => Data::Text(other.to_string()),
    }
}

/// Re-shape a step result into `[{col: val, ...}, ...]` — the shape the
/// frontend's DuckDB-WASM `read_json_auto` ingest expects.
///
/// Accepts either row shape produced upstream:
/// - Warehouse path (`query_result_to_json`) emits `rows: [[val, val], ...]`
///   (array of arrays, column-positional).
/// - Preagg path (`execute_preagg_sql`) emits `rows: [{col: val, ...}, ...]`
///   (array of objects). Used as-is.
///
/// Returns `None` when the value isn't tabular.
fn tabular_to_records(value: &JsonValue) -> Option<Vec<JsonValue>> {
    let columns = value.get("columns")?.as_array()?;
    let rows = value.get("rows")?.as_array()?;
    let col_names: Vec<&str> = columns.iter().filter_map(|v| v.as_str()).collect();
    let records = rows
        .iter()
        .filter_map(|row| match row {
            JsonValue::Array(cells) => {
                let mut obj = serde_json::Map::with_capacity(col_names.len());
                for (i, name) in col_names.iter().enumerate() {
                    let cell = cells.get(i).cloned().unwrap_or(JsonValue::Null);
                    obj.insert((*name).to_string(), cell);
                }
                Some(JsonValue::Object(obj))
            }
            JsonValue::Object(_) => Some(row.clone()),
            _ => None,
        })
        .collect();
    Some(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tabular_value_becomes_array_of_objects() {
        let v = json!({
            "columns": ["month", "total"],
            "rows": [["2010-02", 42.0], ["2010-03", 31.5]],
            "sql": "select ...",
        });
        let records = tabular_to_records(&v).unwrap();
        assert_eq!(
            records,
            vec![
                json!({"month": "2010-02", "total": 42.0}),
                json!({"month": "2010-03", "total": 31.5}),
            ]
        );
    }

    #[test]
    fn tabular_value_accepts_object_rows() {
        // Reagg / preagg path emits rows as `[{col: val}]` rather than
        // `[[val]]`; we must accept both so preagg-served tasks don't
        // render as empty tables.
        let v = json!({
            "columns": ["a", "b"],
            "rows": [
                {"a": 1, "b": "x"},
                {"a": 2, "b": "y"},
            ],
        });
        let records = tabular_to_records(&v).unwrap();
        assert_eq!(
            records,
            vec![json!({"a": 1, "b": "x"}), json!({"a": 2, "b": "y"})]
        );
    }

    #[test]
    fn missing_rows_or_columns_returns_none() {
        assert!(tabular_to_records(&json!({"rows": []})).is_none());
        assert!(tabular_to_records(&json!({"columns": ["x"]})).is_none());
        assert!(tabular_to_records(&json!("hello")).is_none());
    }

    #[test]
    fn tabular_task_produces_data_table_with_json_string() {
        let v = json!({
            "columns": ["a"],
            "rows": [[1], [2]],
        });
        let data = automation_task_to_data("query", &v);
        let Data::Table(td) = data else {
            panic!("expected Data::Table");
        };
        assert!(td.file_path.to_string_lossy().contains("query"));
        let json: JsonValue = serde_json::from_str(&td.json.unwrap()).unwrap();
        assert_eq!(json, json!([{"a": 1}, {"a": 2}]));
    }

    #[test]
    fn non_tabular_task_produces_data_text() {
        let v = json!({"text": "hello"});
        let data = automation_task_to_data("greet", &v);
        match data {
            Data::Text(s) => assert!(s.contains("hello")),
            other => panic!("expected Data::Text, got {other:?}"),
        }
    }

    /// Cells that have fewer elements than columns pad with null —
    /// keeps DuckDB-WASM `read_json_auto` happy with rectangular input.
    #[test]
    fn short_rows_pad_with_null() {
        let v = json!({"columns": ["a", "b"], "rows": [[1]]});
        let records = tabular_to_records(&v).unwrap();
        assert_eq!(records, vec![json!({"a": 1, "b": null})]);
    }
}
