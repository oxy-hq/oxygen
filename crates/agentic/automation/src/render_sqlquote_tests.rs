//! `sqlquote` writes a value the way the engine its SQL is sent to reads a
//! literal, and nothing at all when no engine is known and the value would
//! need escaping.
//!
//! Every assertion is on the rendered SQL: no statement here reaches an
//! engine. The reader below is each grammar as its documentation states it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentic_connector::{
    ConnectorError, DatabaseConnector, ExecutionResult, SqlDialect, StringLiteral,
};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::{render_jinja_string, render_sql_string};
use crate::step_executor::run_automation_step;
use crate::workspace::{IntegrationConfig, WorkspaceContext};

/// A backslash, a quote, the two together, and a backslash at the end.
const VALUES: [&str; 4] = ["a\\b", "it's", "x\\' OR 1=1 -- ", "C:\\"];

const RULES: [StringLiteral; 3] = [
    StringLiteral::Standard,
    StringLiteral::Backslash,
    StringLiteral::BackslashOnly,
];

const HEAD: &str = "SELECT * FROM t WHERE name = ";
const TAIL: &str = " AND tail";

fn rendered(literal: Option<StringLiteral>, value: &str) -> Result<String, String> {
    let template = format!("{HEAD}{{{{ v | sqlquote }}}}{TAIL}");
    render_sql_string(&template, &json!({ "v": value }), literal)
}

/// Read one literal the way an engine under `rule` would, returning the
/// value it holds and whatever text follows its closing quote.
fn read(rule: StringLiteral, sql: &str) -> Option<(String, &str)> {
    let body = sql.strip_prefix('\'')?;
    let mut chars = body.char_indices().peekable();
    let mut value = String::new();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' if rule != StringLiteral::Standard => value.push(chars.next()?.1),
            '\'' if rule != StringLiteral::BackslashOnly
                && matches!(chars.peek(), Some((_, '\''))) =>
            {
                chars.next();
                value.push('\'');
            }
            '\'' => return Some((value, &body[at + 1..])),
            c => value.push(c),
        }
    }
    None
}

#[test]
fn a_value_stays_inside_its_literal_on_every_engine() {
    for rule in RULES {
        for value in VALUES {
            let sql = rendered(Some(rule), value).expect("a known engine renders");
            let literal = sql.strip_prefix(HEAD).expect("the template's own SQL");
            let (held, rest) = read(rule, literal)
                .unwrap_or_else(|| panic!("{rule:?}: {value:?} left the literal open: {sql}"));
            assert_eq!(held, value, "{rule:?}: {sql}");
            assert_eq!(rest, TAIL, "{rule:?}: {value:?} ended early: {sql}");
        }
    }
}

#[test]
fn each_engine_gets_the_escapes_it_reads() {
    let written = |rule| {
        VALUES.map(|v| {
            let sql = rendered(Some(rule), v).unwrap();
            sql[HEAD.len()..sql.len() - TAIL.len()].to_string()
        })
    };
    // DuckDB, Postgres: the backslash is an ordinary character.
    assert_eq!(
        written(StringLiteral::Standard),
        ["'a\\b'", "'it''s'", "'x\\'' OR 1=1 -- '", "'C:\\'"]
    );
    // ClickHouse, MySQL, Snowflake, Redshift, Domo.
    assert_eq!(
        written(StringLiteral::Backslash),
        ["'a\\\\b'", "'it''s'", "'x\\\\'' OR 1=1 -- '", "'C:\\\\'"]
    );
    // BigQuery: `''` is not a quote there.
    assert_eq!(
        written(StringLiteral::BackslashOnly),
        ["'a\\\\b'", "'it\\'s'", "'x\\\\\\' OR 1=1 -- '", "'C:\\\\'"]
    );
}

#[test]
fn a_plain_value_is_the_same_bytes_on_every_engine() {
    let engines = RULES.map(Some).into_iter().chain([None]);
    for literal in engines {
        for (value, want) in [
            ("Paris", "'Paris'"),
            ("2024-01-01", "'2024-01-01'"),
            ("", "''"),
        ] {
            assert_eq!(
                rendered(literal, value).unwrap(),
                format!("{HEAD}{want}{TAIL}"),
                "{literal:?}"
            );
        }
        // Not only strings: a number is quoted as its text, as it always was.
        assert_eq!(
            render_sql_string("{{ n | sqlquote }}", &json!({ "n": 42 }), literal).unwrap(),
            "'42'",
            "{literal:?}"
        );
    }
}

/// With no engine named there is no spelling every engine reads as the
/// value, so nothing is written. `render_jinja_string` is that case: it
/// renders paths, prompts and HTTP parts, none of them sent to a database.
#[test]
fn an_unknown_engine_refuses_a_value_that_needs_escaping() {
    for value in VALUES {
        let refused = rendered(None, value).expect_err(value);
        assert!(refused.contains("is not known"), "{value:?}: {refused}");
        let refused =
            render_jinja_string("{{ v | sqlquote }}", &json!({ "v": value })).expect_err(value);
        assert!(refused.contains("is not known"), "{value:?}: {refused}");
    }
}

#[test]
fn the_other_filters_do_not_change_with_the_engine() {
    let ctx = json!({ "x": [1, 2, 3], "y": "a'b\\", "id": "abc", "secret": "s3cr3t" });
    let engines = RULES.map(Some).into_iter().chain([None]);
    for literal in engines {
        let render = |template: &str| render_sql_string(template, &ctx, literal).unwrap();
        assert_eq!(render("{{ x | tojson }}"), "[1,2,3]", "{literal:?}");
        assert_eq!(render("{{ y | tojson }}"), "\"a'b\\\\\"", "{literal:?}");
        assert_eq!(render("{{ y }}"), "a'b\\", "{literal:?}");
        assert_eq!(
            render("{{ (id ~ ':' ~ secret) | b64encode }}"),
            "YWJjOnMzY3IzdA==",
            "{literal:?}"
        );
    }
}

/// A condition is rendered with no engine named. One whose `sqlquote` is
/// refused is the step's error: read as "false" it took the next branch and
/// ran the wrong tasks without a word.
#[test]
fn a_refused_sqlquote_fails_the_condition_instead_of_reading_as_false() {
    let env = super::automation_env();
    let holds = |condition: &str, x: &str| {
        let ctx = crate::step_orchestrator::build_minijinja_context(&json!({ "x": x }));
        super::condition_holds(&env, condition, &ctx)
    };
    let quoted = "(x | sqlquote) == \"'Paris'\"";
    assert_eq!(holds(quoted, "Paris"), Ok(true));
    assert_eq!(holds(quoted, "Rome"), Ok(false));
    let refused = holds(quoted, "it's").expect_err("a value sqlquote cannot write here");
    assert!(refused.contains("is not known"), "{refused}");
    assert!(
        refused.contains("(x | sqlquote)"),
        "names the condition: {refused}"
    );

    // Any other failed render is false, as it was before.
    assert_eq!(holds("x | nosuchfilter", "Paris"), Ok(false));
}

// ── The step path: the task's `database` picks the rule ─────────────────────

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

/// A host with one database per rule, named for an engine that reads it,
/// and nothing to say about any other name.
struct Host(Arc<Mutex<Vec<String>>>);

#[async_trait]
impl WorkspaceContext for Host {
    fn workspace_path(&self) -> Option<&Path> {
        None
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    fn string_literal(&self, database: &str) -> Option<StringLiteral> {
        match database {
            "duckdb" => Some(StringLiteral::Standard),
            "clickhouse" => Some(StringLiteral::Backslash),
            "bigquery" => Some(StringLiteral::BackslashOnly),
            _ => None,
        }
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
    async fn resolve_automation_yaml(&self, _: &str) -> Result<String, crate::WorkspaceReadError> {
        unreachable!("not exercised")
    }
}

/// Run an `execute_sql` step against `database` and return the SQL that
/// reached its connector.
async fn sent_to(database: &str, step: Value, ctx: Value) -> Result<String, String> {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut step = step;
    step["name"] = json!("s");
    step["type"] = json!("execute_sql");
    step["database"] = json!(database);
    run_automation_step(&Host(sent.clone()), step, ctx, json!({})).await?;
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 1, "one statement per step");
    Ok(sent[0].clone())
}

const STEP_SQL: &str = "SELECT 1 WHERE store = {{ controls.store | sqlquote }} AND tail";

/// What a Data App viewer sends as a control value.
fn controls(store: &str) -> Value {
    json!({ "controls": { "store": store } })
}

#[tokio::test]
async fn a_step_quotes_by_the_rule_of_its_database() {
    let breakout = "x\\' OR 1=1 -- ";
    for (database, want) in [
        ("duckdb", "'x\\'' OR 1=1 -- '"),
        ("clickhouse", "'x\\\\'' OR 1=1 -- '"),
        ("bigquery", "'x\\\\\\' OR 1=1 -- '"),
    ] {
        let sql = sent_to(
            database,
            json!({ "sql_query": STEP_SQL }),
            controls(breakout),
        )
        .await
        .expect(database);
        assert_eq!(
            sql,
            format!("SELECT 1 WHERE store = {want} AND tail"),
            "{database}"
        );
    }
}

/// A task's own `variables:` feed its SQL, so they take the same rule.
#[tokio::test]
async fn a_step_variable_is_quoted_by_the_same_rule() {
    let step = json!({
        "sql_query": "SELECT 1 WHERE store = {{ quoted }} AND tail",
        "variables": { "quoted": "{{ controls.store | sqlquote }}" },
    });
    let sql = sent_to("clickhouse", step, controls("C:\\")).await.unwrap();
    assert_eq!(sql, "SELECT 1 WHERE store = 'C:\\\\' AND tail");
}

/// A database the host cannot name an engine for: a plain value runs as it
/// always did, and one that would need escaping reaches no connector.
#[tokio::test]
async fn a_step_on_an_unnamed_engine_sends_nothing_it_cannot_quote() {
    let plain = sent_to(
        "mystery",
        json!({ "sql_query": STEP_SQL }),
        controls("Paris"),
    )
    .await;
    assert_eq!(plain.unwrap(), "SELECT 1 WHERE store = 'Paris' AND tail");

    let sent = Arc::new(Mutex::new(Vec::new()));
    let step = json!({ "name": "s", "type": "execute_sql", "database": "mystery",
                       "sql_query": STEP_SQL });
    let refused = run_automation_step(&Host(sent.clone()), step, controls("C:\\"), json!({}))
        .await
        .expect_err("a backslash on an unnamed engine");
    assert!(refused.contains("is not known"), "{refused}");
    // The cause is the task's `database`, so the error names it.
    assert!(
        refused.contains("names no engine for database `mystery`"),
        "{refused}"
    );
    assert!(sent.lock().unwrap().is_empty(), "nothing was sent");
}

/// The "no engine named" hint is specific to a `sqlquote` refusal. A render
/// error from any other cause, on that very same unnamed engine, must not
/// carry it — the cause has nothing to do with quoting, so blaming the
/// database is misleading.
#[tokio::test]
async fn the_unnamed_engine_hint_is_added_only_for_a_sqlquote_refusal() {
    // A `sqlquote` refusal: the hint names the database.
    let sent = Arc::new(Mutex::new(Vec::new()));
    let step = json!({ "name": "s", "type": "execute_sql", "database": "mystery",
                       "sql_query": STEP_SQL });
    let refused = run_automation_step(&Host(sent.clone()), step, controls("C:\\"), json!({}))
        .await
        .expect_err("a backslash on an unnamed engine");
    assert!(
        refused.contains("names no engine for database `mystery`"),
        "{refused}"
    );

    // An unrelated render error (an unknown filter) on the same unnamed
    // engine must not get the hint.
    let sent = Arc::new(Mutex::new(Vec::new()));
    let step = json!({
        "name": "s", "type": "execute_sql", "database": "mystery",
        "sql_query": "SELECT 1 WHERE store = {{ controls.store | nosuchfilter }} AND tail",
    });
    let err = run_automation_step(&Host(sent), step, controls("Paris"), json!({}))
        .await
        .expect_err("an unknown filter fails to render");
    assert!(
        !err.contains("names no engine for database"),
        "an unrelated render error must not claim this: {err}"
    );
}
