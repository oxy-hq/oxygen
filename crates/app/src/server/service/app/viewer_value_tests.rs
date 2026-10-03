//! A viewer's control value is data; only the app author's text is a
//! template.
//!
//! `AppService::run` used to hand the viewer's param to
//! `render_control_default`, so a param whose value held `{{ … }}` was
//! evaluated as a Jinja template on the server before it became
//! `controls.<name>`. Every test here goes from the two things `run` is given
//! — the app's YAML and the request's `params` — to what it computes from
//! them: the control values, and the SQL its tasks send. Nothing reaches a
//! database; the connector below records the statement.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentic_automation::workspace::IntegrationConfig;
use agentic_automation::{WorkspaceContext, WorkspaceReadError};
use agentic_connector::{
    ConnectorError, DatabaseConnector, ExecutionResult, SqlDialect, StringLiteral,
};
use async_trait::async_trait;
use oxy::config::model::AppConfig;
use serde_json::{Value, json};

use super::app_service::run_tasks;
use super::controls::{control_values, declared_controls};

/// One control with no default (`store`), one whose default the author wrote
/// as a template (`since`), and a task that reads both.
const APP: &str = r#"
tasks:
  - name: sales
    type: execute_sql
    database: clickhouse
    sql_query: "SELECT 1 WHERE store = {{ controls.store | sqlquote }} AND day >= {{ controls.since | sqlquote }}"
display:
  - type: control
    name: store
    control_type: select
  - type: control
    name: since
    control_type: date
    default: "{{ now(fmt='%Y') }}-01-01"
"#;

fn app(yaml: &str) -> AppConfig {
    serde_yaml::from_str(yaml).expect("the test app parses")
}

fn params(pairs: &[(&str, &str)]) -> HashMap<String, Value> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), json!(value)))
        .collect()
}

/// What `run` puts in `controls.*` for `yaml` and a request's `params`.
fn controls(yaml: &str, sent: &[(&str, &str)]) -> HashMap<String, Value> {
    control_values(&declared_controls(&app(yaml)), &params(sent))
}

/// Records the SQL that reaches it.
struct Recorder(Arc<Mutex<Vec<String>>>);

#[async_trait]
impl DatabaseConnector for Recorder {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }
    async fn execute_query(&self, sql: &str, _: u64) -> Result<ExecutionResult, ConnectorError> {
        self.0.lock().unwrap().push(sql.to_string());
        Ok(ExecutionResult::empty())
    }
}

/// A host whose one database reads a backslash as an escape.
struct Host(Arc<Mutex<Vec<String>>>);

#[async_trait]
impl WorkspaceContext for Host {
    fn workspace_path(&self) -> Option<&Path> {
        None
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    fn string_literal(&self, _: &str) -> Option<StringLiteral> {
        Some(StringLiteral::Backslash)
    }
    async fn get_connector(&self, _: &str) -> Result<Arc<dyn DatabaseConnector>, String> {
        Ok(Arc::new(Recorder(self.0.clone())))
    }
    async fn get_integration(&self, _: &str) -> Result<IntegrationConfig, String> {
        unreachable!("not exercised")
    }
    async fn list_automation_files(&self) -> Result<Vec<PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(&self, _: &str) -> Result<String, WorkspaceReadError> {
        unreachable!("not exercised")
    }
}

/// The SQL the app's task sends for a request's `params`.
async fn sql_sent(yaml: &str, sent: &[(&str, &str)]) -> String {
    let config = app(yaml);
    let statements = Arc::new(Mutex::new(Vec::new()));
    run_tasks(
        &Host(statements.clone()),
        &config.tasks,
        control_values(&declared_controls(&config), &params(sent)),
    )
    .await
    .expect("the task runs");
    let statements = statements.lock().unwrap();
    assert_eq!(statements.len(), 1, "one statement per task");
    statements[0].clone()
}

/// The report: a viewer's `{{ 7 * 7 }}` came back as `49`, and `{{ now() }}`
/// as the server's clock.
#[test]
fn a_viewers_value_is_not_evaluated() {
    for sent in [
        "{{ 7 * 7 }}",
        "{{ now() }}",
        "{% for i in range(3) %}x{% endfor %}",
        "{{ debug() }}",
        "{{ unclosed",
    ] {
        let values = controls(APP, &[("store", sent)]);
        assert_eq!(values["store"], json!(sent), "{sent:?} was evaluated");
    }
}

/// A value that would render to another template is not rendered at all, so
/// there is no second pass for its output to reach.
#[test]
fn a_value_that_would_render_to_a_template_is_left_as_sent() {
    let sent = "{{ '{{ 7 * 7 }}' }}";
    assert_eq!(controls(APP, &[("store", sent)])["store"], json!(sent));
}

/// The author's `default:` is a template, and still renders — when the
/// viewer sends nothing for that control, or sends the empty string.
#[test]
fn an_authors_default_is_still_rendered() {
    let year = chrono::Local::now().format("%Y").to_string();
    for sent in [vec![], vec![("since", "")]] {
        let values = controls(APP, &sent);
        assert_eq!(values["since"], json!(format!("{year}-01-01")), "{sent:?}");
        assert_eq!(values["store"], Value::Null, "no default, nothing sent");
    }
}

/// A default that renders to template syntax is rendered once: its output is
/// a value like any other.
#[test]
fn an_authors_default_is_rendered_once() {
    let yaml = APP.replace("{{ now(fmt='%Y') }}-01-01", "{{ '{{ 7 * 7 }}' }}");
    assert_eq!(controls(&yaml, &[])["since"], json!("{{ 7 * 7 }}"));
}

/// The viewer's value replaces the author's default rather than being merged
/// into it, and is not rendered on the way.
#[test]
fn a_viewers_value_replaces_a_templated_default_as_data() {
    let values = controls(APP, &[("since", "{{ now(fmt='%Y') }}")]);
    assert_eq!(values["since"], json!("{{ now(fmt='%Y') }}"));
}

/// Inside task SQL the value arrives as the characters the viewer sent, and
/// `sqlquote` still quotes it as data by the task's engine — here one that
/// reads a backslash, so the quote is doubled and the backslash escaped.
#[tokio::test]
async fn a_viewers_template_reaches_task_sql_as_its_characters() {
    let sql = sql_sent(
        APP,
        &[("store", "{{ 7 * 7 }}"), ("since", "{% if x %}'\\{{")],
    )
    .await;
    assert_eq!(
        sql,
        "SELECT 1 WHERE store = '{{ 7 * 7 }}' AND day >= '{% if x %}''\\\\{{'"
    );
}

/// The same task with nothing sent: the author's templated default is what
/// the SQL reads.
#[tokio::test]
async fn an_authors_default_reaches_task_sql_rendered() {
    let year = chrono::Local::now().format("%Y").to_string();
    let sql = sql_sent(APP, &[("store", "Paris")]).await;
    assert_eq!(
        sql,
        format!("SELECT 1 WHERE store = 'Paris' AND day >= '{year}-01-01'")
    );
}

/// A default that renders to template syntax is not rendered again by the
/// task that reads it.
#[tokio::test]
async fn a_rendered_default_is_not_rendered_again_in_task_sql() {
    let yaml = APP.replace("{{ now(fmt='%Y') }}-01-01", "{{ '{{ 7 * 7 }}' }}");
    let sql = sql_sent(&yaml, &[("store", "Paris")]).await;
    assert_eq!(
        sql,
        "SELECT 1 WHERE store = 'Paris' AND day >= '{{ 7 * 7 }}'"
    );
}

/// A task's own `variables:` are the author's templates too. One that quotes
/// a control renders to a value, and the SQL that reads the variable does not
/// render that value again.
#[tokio::test]
async fn a_viewers_template_is_not_rendered_through_a_task_variable() {
    let yaml = APP.replace(
        "    sql_query: \"SELECT 1 WHERE store = {{ controls.store | sqlquote }} AND day >= {{ controls.since | sqlquote }}\"\n",
        "    sql_query: \"SELECT 1 WHERE store = {{ quoted }}\"\n    variables:\n      quoted: \"{{ controls.store | sqlquote }}\"\n",
    );
    assert_ne!(yaml, APP, "the task was rewritten");
    let sql = sql_sent(&yaml, &[("store", "{{ 7 * 7 }}")]).await;
    assert_eq!(sql, "SELECT 1 WHERE store = '{{ 7 * 7 }}'");
}

/// What a template evaluated by `render_control_default` could reach — the
/// impact of the hole, kept as a test so the list stays true. It is what an
/// app author's `default:` can still do.
mod reach {
    use serde_json::json;

    use super::super::controls::render_control_default;

    fn rendered(template: &str) -> String {
        match render_control_default(json!(template)) {
            serde_json::Value::String(s) => s,
            other => panic!("{template:?} rendered to {other}"),
        }
    }

    /// No variable is in scope: not another control, not the workspace
    /// config, not a secret, not the environment.
    #[test]
    fn no_variable_is_in_scope() {
        for name in ["controls", "config", "secrets", "env", "workspace", "self"] {
            assert_eq!(rendered(&format!("[{{{{ {name} }}}}]")), "[]", "{name}");
        }
    }

    /// No template can be loaded: the environment has no loader and no
    /// registered template, so `include` / `import` / `extends` fail, and a
    /// failed render returns the text unrendered.
    #[test]
    fn no_file_or_other_template_is_reachable() {
        for template in [
            "{% include 'config.yml' %}",
            "{% import '/etc/passwd' as f %}{{ f }}",
            "{% extends '.env' %}",
        ] {
            assert_eq!(rendered(template), template);
        }
    }

    /// What is reachable: arithmetic, minijinja's built-in filters and
    /// functions, loops, and the server's clock with its UTC offset.
    #[test]
    fn expressions_builtins_loops_and_the_clock_are_reachable() {
        assert_eq!(rendered("{{ 7 * 7 }}"), "49");
        assert_eq!(
            rendered("{{ 'ab' | upper }}{{ range(3) | list }}"),
            "AB[0, 1, 2]"
        );
        assert_eq!(rendered("{% for i in range(3) %}x{% endfor %}"), "xxx");
        assert_eq!(rendered("{{ {'a': 1} | tojson }}"), r#"{"a":1}"#);
        let offset = chrono::Local::now().format("%:z").to_string();
        assert!(rendered("{{ now() }}").ends_with(&offset));
    }
}
