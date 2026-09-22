//! The shape zoo (`fixtures/data-shapes/zoo.json`) and the SQL every loader builds from it.
//!
//! Design: `internal-docs/2026-09-15-custom-app-guard-sentry-and-data-shapes-design.md`, the
//! shape zoo section. The zoo is written by hand from the connectors' own type maps and holds
//! synthetic values only. Each engine's cases become one table with one row:
//!
//! - **Table:** `oxy_shape_zoo_<first 8 hex chars of sha256(zoo.json bytes)>`, so a changed zoo
//!   is a new table and an unchanged one is loaded once.
//! - **Columns:** `c001`, `c002`, … in case order, each typed with the case's `native_type`.
//! - **Row:** `INSERT INTO <t> VALUES (<value_sql>, …)`. Never `PRIMARY KEY`, `UNIQUE` or an
//!   index, so the same DDL loads on DuckLake and Airhouse.
//!
//! The platform canary builds the same SQL in TypeScript
//! (`customer-apps/examples/platform-canary/functions/shape-zoo.ts`). Both test [`SAMPLE`]
//! against the same expected strings, so neither can drift alone.
//!
//! **Comparison.** A function holds every number as a double, so numbers compare as doubles.
//! `{"$error": s}` matches a read that was refused with a message containing `s`.

use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// The fixture, relative to the repo root.
pub(crate) const ZOO_PATH: &str = "fixtures/data-shapes/zoo.json";

/// The value classes of the design's shape key, plus `plain` for an ordinary value.
pub(crate) const CLASSES: &[&str] = &[
    "plain",
    "null",
    "empty",
    "whitespace",
    "non_bmp",
    "len_gt_1k",
    "len_gt_64k",
    "negative",
    "zero",
    "nan_inf",
    "precision_gt_18",
    "date_before_1970",
    "date_after_2100",
    "tz_bearing",
    "nested_json",
    "array",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Engine {
    ClickHouse,
    Postgres,
    DuckDb,
}

impl Engine {
    pub(crate) const ALL: [Engine; 3] = [Engine::ClickHouse, Engine::Postgres, Engine::DuckDb];

    /// The key under `engines` in zoo.json, and the first segment of every case key.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Engine::ClickHouse => "clickhouse",
            Engine::Postgres => "postgres",
            Engine::DuckDb => "duckdb",
        }
    }
}

/// The read path an expectation is for: `expect.warehouse` (`ctx.warehouse.query`) or
/// `expect.oltp` (`ctx.oltp`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Plane {
    Warehouse,
    Oltp,
}

impl Plane {
    /// The key under `expect` in zoo.json, and the word a failure message names.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Plane::Warehouse => "warehouse",
            Plane::Oltp => "oltp",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Zoo {
    pub(crate) version: u32,
    pub(crate) engines: Engines,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Engines {
    pub(crate) clickhouse: EngineCases,
    pub(crate) postgres: EngineCases,
    pub(crate) duckdb: EngineCases,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EngineCases {
    pub(crate) cases: Vec<Case>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Case {
    pub(crate) key: String,
    pub(crate) native_type: String,
    pub(crate) class: String,
    pub(crate) value_sql: String,
    pub(crate) expect: Expect,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Expect {
    pub(crate) warehouse: Value,
    /// `Some(Value::Null)` for a NULL read through `ctx.oltp`: a plain `Option<Value>` would
    /// turn `"oltp": null` into `None` and lose the expectation.
    #[serde(default, deserialize_with = "present")]
    pub(crate) oltp: Option<Value>,
}

fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

impl Case {
    /// What a read of this case must match on `plane`. Only a Postgres case has an
    /// `expect.oltp` (`shape_zoo_fixture_is_well_formed` pins that), so asking another
    /// engine's case for [`Plane::Oltp`] is a bug in the caller.
    pub(crate) fn expected(&self, plane: Plane) -> &Value {
        match plane {
            Plane::Warehouse => &self.expect.warehouse,
            Plane::Oltp => self
                .expect
                .oltp
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no expect.oltp", self.key)),
        }
    }
}

impl Zoo {
    pub(crate) fn cases(&self, engine: Engine) -> &[Case] {
        match engine {
            Engine::ClickHouse => &self.engines.clickhouse.cases,
            Engine::Postgres => &self.engines.postgres.cases,
            Engine::DuckDb => &self.engines.duckdb.cases,
        }
    }
}

/// The parsed zoo, and the digest of the exact bytes it was parsed from.
pub(crate) struct LoadedZoo {
    pub(crate) zoo: Zoo,
    pub(crate) sha256_hex: String,
}

impl LoadedZoo {
    pub(crate) fn table(&self) -> String {
        table_name(&self.sha256_hex)
    }
}

pub(crate) fn load() -> LoadedZoo {
    parse(crate::common::read_repo_file(ZOO_PATH).as_bytes())
}

pub(crate) fn parse(bytes: &[u8]) -> LoadedZoo {
    let zoo: Zoo =
        serde_json::from_slice(bytes).unwrap_or_else(|e| panic!("zoo.json does not parse: {e}"));
    LoadedZoo {
        zoo,
        sha256_hex: hex::encode(Sha256::digest(bytes)),
    }
}

pub(crate) fn table_name(sha256_hex: &str) -> String {
    format!("oxy_shape_zoo_{}", &sha256_hex[..8])
}

pub(crate) fn column(index: usize) -> String {
    format!("c{:03}", index + 1)
}

pub(crate) fn create_table_sql(engine: Engine, table: &str, cases: &[Case]) -> String {
    let columns = cases
        .iter()
        .enumerate()
        .map(|(i, case)| format!("{} {}", column(i), case.native_type))
        .collect::<Vec<_>>()
        .join(", ");
    match engine {
        Engine::ClickHouse => format!(
            "CREATE TABLE IF NOT EXISTS {table} ({columns}) ENGINE = MergeTree ORDER BY tuple()"
        ),
        Engine::Postgres | Engine::DuckDb => {
            format!("CREATE TABLE IF NOT EXISTS {table} ({columns})")
        }
    }
}

pub(crate) fn insert_sql(table: &str, cases: &[Case]) -> String {
    let values = cases
        .iter()
        .map(|case| case.value_sql.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!("INSERT INTO {table} VALUES ({values})")
}

pub(crate) fn count_sql(table: &str) -> String {
    format!("SELECT count(*) AS n FROM {table}")
}

pub(crate) fn select_column_sql(table: &str, index: usize) -> String {
    format!("SELECT {} FROM {table} LIMIT 1", column(index))
}

/// Whether a read matches its expectation: numbers as doubles, `{"$error": s}` by substring.
pub(crate) fn matches(expected: &Value, actual: &Value) -> bool {
    match error_needle(expected) {
        Some(needle) => error_needle(actual).is_some_and(|message| message.contains(needle)),
        None => same_json(expected, actual),
    }
}

fn error_needle(value: &Value) -> Option<&str> {
    match value {
        Value::Object(map) if map.len() == 1 => map.get("$error").and_then(Value::as_str),
        _ => None,
    }
}

fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_json(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_json(v, w)))
        }
        _ => a == b,
    }
}

/// Every case whose column in `reads` differs from its expectation on `plane`, worded so the
/// key and class lead: `<key> (<class>) on <plane>: expected <json> got <json>`.
pub(crate) fn check_reads(cases: &[Case], plane: Plane, reads: &Map<String, Value>) -> Vec<String> {
    cases
        .iter()
        .enumerate()
        .filter_map(|(i, case)| {
            let expected = case.expected(plane);
            let actual = reads.get(&column(i));
            if actual.is_some_and(|a| matches(expected, a)) {
                return None;
            }
            let got = actual.map_or_else(|| "<no value>".to_string(), Value::to_string);
            Some(format!(
                "{} ({}) on {}: expected {expected} got {got}",
                case.key,
                case.class,
                plane.name()
            ))
        })
        .collect()
}

/// The shared vector. `platform-canary/functions/shape-zoo-sync.test.ts` holds the same bytes
/// and asserts the same strings; change both or neither.
pub(crate) const SAMPLE: &str = concat!(
    r#"{"version":1,"engines":{"clickhouse":{"cases":[{"key":"clickhouse/String/non_bmp","native_type":"String","class":"non_bmp","value_sql":"'𝔘nicode'","expect":{"warehouse":"𝔘nicode"}},{"key":"clickhouse/Nullable(Int32)/null","native_type":"Nullable(Int32)","class":"null","value_sql":"NULL","expect":{"warehouse":null}}]},"postgres":{"cases":[{"key":"postgres/numeric/negative","native_type":"numeric","class":"negative","value_sql":"-1.25","expect":{"warehouse":"-1.25","oltp":{"$error":"which cannot be returned directly"}}},{"key":"postgres/timestamptz/tz_bearing","native_type":"timestamptz","class":"tz_bearing","value_sql":"TIMESTAMPTZ '2024-03-10 12:34:56+05:30'","expect":{"warehouse":"2024-03-10 07:04:56","oltp":"2024-03-10T07:04:56.000000Z"}}]},"duckdb":{"cases":[]}}}"#,
    "\n"
);

#[test]
fn shape_zoo_sql_matches_the_shared_vector() {
    let sample = parse(SAMPLE.as_bytes());
    assert_eq!(
        sample.sha256_hex,
        "a7a68bfb136327ec9d55f27de1be43c71f78694b35c7b6943d3587070854b0c2"
    );
    let table = sample.table();
    assert_eq!(table, "oxy_shape_zoo_a7a68bfb");
    let ch = sample.zoo.cases(Engine::ClickHouse);
    assert_eq!(
        create_table_sql(Engine::ClickHouse, &table, ch),
        "CREATE TABLE IF NOT EXISTS oxy_shape_zoo_a7a68bfb (c001 String, c002 Nullable(Int32)) ENGINE = MergeTree ORDER BY tuple()"
    );
    assert_eq!(
        insert_sql(&table, ch),
        "INSERT INTO oxy_shape_zoo_a7a68bfb VALUES ('𝔘nicode', NULL)"
    );
    let pg = sample.zoo.cases(Engine::Postgres);
    assert_eq!(
        create_table_sql(Engine::Postgres, &table, pg),
        "CREATE TABLE IF NOT EXISTS oxy_shape_zoo_a7a68bfb (c001 numeric, c002 timestamptz)"
    );
    assert_eq!(
        insert_sql(&table, pg),
        "INSERT INTO oxy_shape_zoo_a7a68bfb VALUES (-1.25, TIMESTAMPTZ '2024-03-10 12:34:56+05:30')"
    );
    assert_eq!(
        select_column_sql(&table, 1),
        "SELECT c002 FROM oxy_shape_zoo_a7a68bfb LIMIT 1"
    );
    assert_eq!(
        count_sql(&table),
        "SELECT count(*) AS n FROM oxy_shape_zoo_a7a68bfb"
    );
}

#[test]
fn shape_zoo_compares_numbers_as_doubles_and_errors_by_substring() {
    assert!(matches(
        &json!(9223372036854776000u64),
        &json!(9223372036854775807i64)
    ));
    assert!(matches(&json!(0), &json!(-0.0)));
    assert!(!matches(&json!("1.5"), &json!(1.5)));
    assert!(matches(
        &json!({ "a": [1, null] }),
        &json!({ "a": [1.0, null] })
    ));
    let refused = json!({ "$error": "result column `c001` has Postgres type `numeric`, which cannot be returned directly" });
    assert!(matches(
        &json!({ "$error": "cannot be returned" }),
        &refused
    ));
    assert!(!matches(
        &json!({ "$error": "cannot be returned" }),
        &json!("-1.25")
    ));
    assert!(!matches(&json!(null), &json!({ "$error": "boom" })));

    let sample = parse(SAMPLE.as_bytes());
    let reads = json!({ "c001": "-1.2", "c002": "2024-03-10 07:04:56" });
    assert_eq!(
        check_reads(
            sample.zoo.cases(Engine::Postgres),
            Plane::Warehouse,
            reads.as_object().unwrap()
        ),
        vec![r#"postgres/numeric/negative (negative) on warehouse: expected "-1.25" got "-1.2""#]
    );
}

#[test]
fn shape_zoo_fixture_is_well_formed() {
    let loaded = load();
    assert_eq!(loaded.zoo.version, 1, "zoo.json version");
    let mut keys = BTreeSet::new();
    for engine in Engine::ALL {
        for case in loaded.zoo.cases(engine) {
            let key = format!("{}/{}/{}", engine.name(), case.native_type, case.class);
            assert_eq!(case.key, key, "a key is <engine>/<native_type>/<class>");
            assert!(keys.insert(case.key.clone()), "duplicate key {}", case.key);
            assert!(
                CLASSES.contains(&case.class.as_str()),
                "{}: unknown class",
                case.key
            );
            assert_eq!(
                case.expect.oltp.is_some(),
                engine == Engine::Postgres,
                "{}: every postgres case, and only a postgres case, has expect.oltp",
                case.key
            );
            for expected in [Some(&case.expect.warehouse), case.expect.oltp.as_ref()]
                .into_iter()
                .flatten()
            {
                if expected.get("$error").is_some() {
                    assert!(
                        error_needle(expected).is_some_and(|needle| !needle.is_empty()),
                        "{}: $error must be the object's only key, a non-empty string",
                        case.key
                    );
                }
            }
        }
    }
}
