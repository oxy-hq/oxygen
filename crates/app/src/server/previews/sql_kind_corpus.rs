//! Corpus check: does the classifier read a real workspace's SQL correctly?
//!
//! Run by hand before enabling preview runs on a workspace — never in CI, and
//! no customer SQL is committed. Point `OXY_PREVIEW_SQL_CORPUS` at a checkout
//! whose `oxy/` directory holds the project (pokehouse-oxy, say):
//!
//! ```sh
//! OXY_PREVIEW_SQL_CORPUS=/path/to/pokehouse-oxy \
//!   cargo nextest run -p oxy-app --lib --run-ignored only -E 'test(sql_kind::corpus)'
//! ```
//!
//! It reads every `execute_sql` step under `oxy/**/*.{procedure,automation}.yml`
//! (inline `sql_query` or `sql_file`), classifies it with the dialect of the
//! step's `database` from `oxy/config.yml`, and fails on:
//!
//! * **ClickHouse**: a statement that does not classify, or a read-shaped
//!   statement (`SELECT`/`WITH`/`SHOW`/`DESCRIBE`/`EXPLAIN`) that does not
//!   classify as `Read` — either would hold a read in a preview. A genuine
//!   write classifies as `Write` and is held, which is the point.
//! * **Airhouse**: a statement that does not classify. Phase 2a holds every
//!   Airhouse write rather than rewriting it (the rewrite is phase 2b), so what
//!   matters here is that each held write is reported with its verb and
//!   targets rather than as "unclassified".
//!
//! Jinja is neutralised before parsing: `{{ … }}` becomes the bare word
//! `jinja_value` (inside quotes it is just string content), and `{% … %}` /
//! `{# … #}` tags are dropped. A step whose `sql_file` path is templated is
//! skipped and counted. The report names files and step names only — never
//! the SQL itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::*;

#[test]
#[ignore = "reads a workspace's SQL from $OXY_PREVIEW_SQL_CORPUS; run by hand"]
fn every_corpus_statement_classifies() {
    let root = std::env::var_os("OXY_PREVIEW_SQL_CORPUS")
        .map(PathBuf::from)
        .expect("set OXY_PREVIEW_SQL_CORPUS to a checkout holding an `oxy/` project");
    let project = root.join("oxy");
    let databases = database_types(&project.join("config.yml"));
    let mut report = Report::default();
    for file in procedure_files(&project) {
        let doc: serde_yaml::Value = match std::fs::read_to_string(&file)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_yaml::from_str(&s).map_err(|e| e.to_string()))
        {
            Ok(doc) => doc,
            Err(e) => {
                report.problem(&file, "<file>", &format!("unreadable: {e}"));
                continue;
            }
        };
        let mut steps = Vec::new();
        collect_execute_sql(&doc, &mut steps);
        for step in steps {
            check_step(&project, &file, step, &databases, &mut report);
        }
    }
    println!("{}", report.summary());
    assert!(
        report.problems.is_empty(),
        "{} problem(s):\n{}",
        report.problems.len(),
        report.problems.join("\n")
    );
}

#[derive(Default)]
struct Report {
    counts: BTreeMap<String, usize>,
    problems: Vec<String>,
}

impl Report {
    fn count(&mut self, key: impl Into<String>) {
        *self.counts.entry(key.into()).or_default() += 1;
    }
    fn problem(&mut self, file: &Path, step: &str, what: &str) {
        self.problems
            .push(format!("{}: step `{step}`: {what}", file.display()));
    }
    fn summary(&self) -> String {
        let lines: Vec<String> = self
            .counts
            .iter()
            .map(|(k, v)| format!("  {k}: {v}"))
            .collect();
        format!("corpus:\n{}", lines.join("\n"))
    }
}

fn check_step(
    project: &Path,
    file: &Path,
    step: &serde_yaml::Mapping,
    databases: &BTreeMap<String, String>,
    report: &mut Report,
) {
    let name = yaml_str(step, "name").unwrap_or("<unnamed>").to_string();
    let database = yaml_str(step, "database").unwrap_or_default();
    let Some(kind) = databases.get(database) else {
        report.problem(
            file,
            &name,
            &format!("database `{database}` is not in config.yml"),
        );
        return;
    };
    let sql = match step_sql(project, step) {
        Ok(Some(sql)) => sql,
        Ok(None) => return report.count("skipped: templated sql_file path"),
        Err(e) => return report.problem(file, &name, &e),
    };
    let dialect = dialect_for(kind);
    let kinds = classify(dialect, &neutralise_jinja(&sql));
    let family = if dialect == SqlDialect::CLICKHOUSE {
        "clickhouse"
    } else {
        kind.as_str()
    };
    for statement in &kinds {
        match statement {
            StatementKind::Read => report.count(format!("{family}: read")),
            StatementKind::Write { verb, .. } => report.count(format!("{family}: write ({verb})")),
            StatementKind::Unclassified(reason) => {
                report.count(format!("{family}: unclassified"));
                report.problem(
                    file,
                    &name,
                    &format!("{family} statement does not classify: {reason}"),
                );
            }
        }
    }
    if dialect == SqlDialect::CLICKHOUSE && read_shaped(&sql) && !is_all_read(&kinds) {
        report.problem(file, &name, "a read-shaped ClickHouse step would be held");
    }
}

/// `Ok(None)` for a `sql_file` whose path is itself templated.
fn step_sql(project: &Path, step: &serde_yaml::Mapping) -> Result<Option<String>, String> {
    if let Some(sql) = yaml_str(step, "sql_query") {
        return Ok(Some(sql.to_string()));
    }
    let path = yaml_str(step, "sql_file").ok_or("neither sql_query nor sql_file")?;
    if path.contains("{{") || path.contains("{%") {
        return Ok(None);
    }
    std::fs::read_to_string(project.join(path))
        .map(Some)
        .map_err(|e| format!("sql_file `{path}`: {e}"))
}

fn dialect_for(database_type: &str) -> SqlDialect {
    match database_type {
        "clickhouse" => SqlDialect::CLICKHOUSE,
        "airhouse" | "airhouse_managed" | "duckdb" | "motherduck" => SqlDialect::DuckDb,
        "postgres" | "postgres_managed" | "redshift" => SqlDialect::Postgres,
        "snowflake" => SqlDialect::Snowflake,
        "bigquery" => SqlDialect::BigQuery,
        _ => SqlDialect::Other("corpus"),
    }
}

/// Lexically a read: the first keyword. Only used to catch a read the
/// classifier would hold; it decides nothing in production.
fn read_shaped(sql: &str) -> bool {
    let first = neutralise_jinja(sql)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("--"))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(
        first.trim_start_matches('('),
        "SELECT" | "WITH" | "SHOW" | "DESCRIBE" | "DESC" | "EXPLAIN"
    )
}

fn neutralise_jinja(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(start) = rest.find(['{']) {
        let (open, close, replacement) = match rest[start..].get(..2) {
            Some("{{") => ("{{", "}}", "jinja_value"),
            Some("{%") => ("{%", "%}", ""),
            Some("{#") => ("{#", "#}", ""),
            _ => {
                out.push_str(&rest[..=start]);
                rest = &rest[start + 1..];
                continue;
            }
        };
        out.push_str(&rest[..start]);
        match rest[start + open.len()..].find(close) {
            Some(end) => {
                out.push_str(replacement);
                rest = &rest[start + open.len() + end + close.len()..];
            }
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn procedure_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.')
            || ["target", "node_modules", "dist", "build"].contains(&name.as_str())
        {
            continue;
        }
        if path.is_dir() {
            out.extend(procedure_files(&path));
        } else if name.ends_with(".procedure.yml") || name.ends_with(".automation.yml") {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn collect_execute_sql<'a>(value: &'a serde_yaml::Value, out: &mut Vec<&'a serde_yaml::Mapping>) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            if yaml_str(map, "type") == Some("execute_sql") {
                out.push(map);
            }
            for v in map.values() {
                collect_execute_sql(v, out);
            }
        }
        serde_yaml::Value::Sequence(items) => {
            for v in items {
                collect_execute_sql(v, out);
            }
        }
        _ => {}
    }
}

fn database_types(config: &Path) -> BTreeMap<String, String> {
    let text =
        std::fs::read_to_string(config).unwrap_or_else(|e| panic!("{}: {e}", config.display()));
    let doc: serde_yaml::Value = serde_yaml::from_str(&text).expect("config.yml parses");
    doc.get("databases")
        .and_then(|d| d.as_sequence())
        .into_iter()
        .flatten()
        .filter_map(|db| {
            let map = db.as_mapping()?;
            Some((
                yaml_str(map, "name")?.to_string(),
                yaml_str(map, "type")?.to_string(),
            ))
        })
        .collect()
}

fn yaml_str<'a>(map: &'a serde_yaml::Mapping, key: &str) -> Option<&'a str> {
    map.get(serde_yaml::Value::String(key.to_string()))
        .and_then(|v| v.as_str())
}

#[test]
fn jinja_is_neutralised_without_touching_the_sql_around_it() {
    assert_eq!(
        neutralise_jinja(
            "SELECT * FROM t WHERE d = '{{ date }}' {% if x %}AND y{% endif %} {# c #}"
        ),
        "SELECT * FROM t WHERE d = 'jinja_value' AND y "
    );
    assert_eq!(neutralise_jinja("SELECT '{' || x"), "SELECT '{' || x");
    assert!(read_shaped(
        "-- note\n  with x as (select 1) select * from x"
    ));
    assert!(!read_shaped("ALTER TABLE t DELETE WHERE 1"));
}
