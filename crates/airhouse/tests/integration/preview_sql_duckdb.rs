//! I1, end to end: SQL shaped like the Airhouse procedures a preview runs
//! (seed tables, journal entries built from POS data, daily rollups, a
//! site-selection refresh) goes through `rewrite` then `verify` into an
//! in-process DuckDB standing in for Airhouse, and every live table comes out
//! byte-identical. The SQL is synthetic; only its shape follows the real
//! procedures.

use std::collections::BTreeMap;

use airhouse::preview_sql::{
    CopyPlan, Prelude, PreviewNamespace, RewriteOptions, ShadowMap, ShadowState, rewrite, verify,
};
use duckdb::Connection;
use uuid::Uuid;

/// Stands in for `OXY_PREVIEW_COW_MAX_ROWS`, small enough that
/// `toast_pos.sales_daily_metrics` and `toast_pos.orders` are copied as
/// partials.
const COW_MAX_ROWS: u64 = 100;
/// In-process DuckDB's own catalog, standing in for the workspace's.
const CATALOG: Option<&str> = Some("memory");

const SEED: &str = "
CREATE SCHEMA toast_pos;
CREATE SCHEMA site_selection;
CREATE SCHEMA bookkeeping;
CREATE TABLE toast_pos.orders AS
  SELECT DATE '2026-01-01' + CAST(i % 10 AS INTEGER) AS business_date, i // 10 % 5 AS location_id,
         CAST(10 + i * 7 % 50 AS DOUBLE) AS net_sales, CAST(i * 3 % 7 AS DOUBLE) AS tips
  FROM range(300) AS r(i);
CREATE TABLE toast_pos.sales_daily_metrics AS
  SELECT DATE '2025-11-01' + CAST(d AS INTEGER) AS business_date, l AS location_id,
         d + l AS orders, CAST(d * 10 + l AS DOUBLE) AS net_sales, CAST(NULL AS DOUBLE) AS avg_ticket
  FROM range(40) AS a(d), range(5) AS b(l);
CREATE TABLE toast_pos.location_daily_latest AS
  SELECT l AS location_id, DATE '2025-12-10' AS last_business_date FROM range(3) AS b(l);
CREATE TABLE site_selection.site_hex_cells AS
  SELECT 'hex_' || i AS hex_id, CAST(i * 37 % 1000 AS DOUBLE) AS population,
         CAST(i * 13 % 100 AS DOUBLE) / 100 AS score
  FROM range(120) AS r(i);
CREATE TABLE site_selection.candidate_sites AS
  SELECT hex_id, score FROM site_selection.site_hex_cells WHERE score > 0.9;
CREATE TABLE bookkeeping.qb_je_account_map (category VARCHAR, qb_account VARCHAR, side VARCHAR);
INSERT INTO bookkeeping.qb_je_account_map VALUES
  ('sales', '4000 Sales', 'credit'), ('tips', '2100 Tips Payable', 'credit'), ('cash', '1000 Cash', 'debit');
CREATE TABLE bookkeeping.journal_entry_log AS
  SELECT DATE '2026-01-01' AS business_date, '4000 Sales' AS qb_account, CAST(100 AS DOUBLE) AS amount;
";

/// Shaped like `qb_je_account_map_seed_airhouse`: an idempotent seed that
/// keeps rows it does not own.
const ACCOUNT_MAP_SEED: &str = "
CREATE SCHEMA IF NOT EXISTS bookkeeping;
CREATE TABLE IF NOT EXISTS bookkeeping.qb_je_account_map (category VARCHAR, qb_account VARCHAR, side VARCHAR);
DELETE FROM bookkeeping.qb_je_account_map WHERE category IN ('sales', 'tips');
INSERT INTO bookkeeping.qb_je_account_map VALUES
  ('sales', '4000 Sales', 'credit'), ('tips', '2150 Tips Payable', 'credit');
";

/// Shaped like `compute_toast_journal_entry_airhouse`: a CTE-built table from
/// POS facts joined to the account map, then a window of a log replaced.
const JOURNAL_ENTRY: &str = "
CREATE OR REPLACE TABLE bookkeeping.toast_journal_entry AS
WITH daily AS (
  SELECT business_date, sum(net_sales) AS sales, sum(tips) AS tips
  FROM toast_pos.orders GROUP BY business_date
), lines AS (
  SELECT d.business_date, m.qb_account, m.side,
         CASE m.category WHEN 'sales' THEN d.sales ELSE d.tips END AS amount
  FROM daily AS d JOIN bookkeeping.qb_je_account_map AS m ON m.category IN ('sales', 'tips')
)
SELECT * FROM lines;
DELETE FROM bookkeeping.journal_entry_log WHERE business_date >= DATE '2026-01-01';
INSERT INTO bookkeeping.journal_entry_log
  SELECT business_date, qb_account, amount FROM bookkeeping.toast_journal_entry WHERE side = 'credit';
";

/// Shaped like `restaurant_analytics_daily_rollups`: delete a window,
/// re-aggregate it, derive a column, merge a per-location summary.
const DAILY_ROLLUPS: &str = "
BEGIN;
DELETE FROM toast_pos.sales_daily_metrics WHERE business_date >= DATE '2026-01-01';
INSERT INTO toast_pos.sales_daily_metrics (business_date, location_id, orders, net_sales)
  SELECT business_date, location_id, count(*), sum(net_sales)
  FROM toast_pos.orders WHERE business_date >= DATE '2026-01-01' GROUP BY ALL;
UPDATE toast_pos.sales_daily_metrics SET avg_ticket = net_sales / orders WHERE orders > 0;
MERGE INTO toast_pos.location_daily_latest AS t
  USING (SELECT location_id, max(business_date) AS d FROM toast_pos.sales_daily_metrics GROUP BY location_id) AS s
  ON t.location_id = s.location_id
  WHEN MATCHED THEN UPDATE SET last_business_date = s.d
  WHEN NOT MATCHED THEN INSERT VALUES (s.location_id, s.d);
COMMIT;
";

/// Shaped like `site_selection_refresh`: build a replacement, swap it in by
/// drop and rename, rebuild a ranked table and a view over it.
const SITE_SELECTION_REFRESH: &str = "
CREATE SCHEMA IF NOT EXISTS site_selection;
DROP TABLE IF EXISTS site_selection.site_hex_cells_next;
CREATE TABLE site_selection.site_hex_cells_next AS
  SELECT hex_id, population * 1.1 AS population, score FROM site_selection.site_hex_cells;
ALTER TABLE site_selection.site_hex_cells_next ADD COLUMN refreshed_at TIMESTAMP;
UPDATE site_selection.site_hex_cells_next SET refreshed_at = TIMESTAMP '2026-01-02 00:00:00';
DROP TABLE site_selection.site_hex_cells;
ALTER TABLE site_selection.site_hex_cells_next RENAME TO site_hex_cells;
TRUNCATE site_selection.candidate_sites;
INSERT INTO site_selection.candidate_sites
  SELECT hex_id, score FROM site_selection.site_hex_cells WHERE score > 0.5 ORDER BY score DESC LIMIT 10;
CREATE OR REPLACE VIEW site_selection.top_sites AS SELECT * FROM site_selection.candidate_sites;
";

/// A write that reads its own target, on a table over the copy cap: it must
/// read the (empty) copy, not every live row.
const SELF_INSERT: &str = "INSERT INTO toast_pos.orders SELECT * FROM toast_pos.orders;";

/// What a preview host does with a step: rewrite, run the preludes, and send
/// every statement (preludes included) through `verify` first, as the preview
/// connector does.
struct Host {
    conn: Connection,
    ns: PreviewNamespace,
    shadow: ShadowMap,
    opts: RewriteOptions,
}

impl Host {
    fn step(&mut self, sql: &str) -> Vec<(String, String)> {
        let rw = rewrite(sql, &self.ns, &self.shadow, &self.opts)
            .unwrap_or_else(|e| panic!("rewrite refused a preview step: {e}\n{sql}"));
        let copies: Vec<CopyPlan> = rw.preludes.iter().filter_map(|p| self.prelude(p)).collect();
        if !rw.sql.is_empty() {
            self.send(&rw.sql);
        }
        self.shadow.apply_rewrite(&rw, &copies);
        rw.redirected_reads
    }

    /// Ensure a schema, or decide and make a copy.
    fn prelude(&self, prelude: &Prelude) -> Option<CopyPlan> {
        if let Some(sql) = prelude.statement(CATALOG) {
            self.send(&sql);
            return None;
        }
        let exists = self.count(&prelude.live_table_probe(CATALOG)?) > 0;
        let rows = if exists {
            self.count(&prelude.live_rows(CATALOG)?)
        } else {
            0
        };
        let plan = prelude.copy_plan(CATALOG, exists, rows, COW_MAX_ROWS)?;
        self.send(&plan.statement);
        Some(plan)
    }

    fn send(&self, sql: &str) {
        let statements = verify(sql, &self.ns, CATALOG)
            .unwrap_or_else(|e| panic!("the verifier refused what the host sends: {e}\n{sql}"));
        for statement in statements {
            self.conn
                .execute_batch(&statement)
                .unwrap_or_else(|e| panic!("DuckDB refused {statement}: {e}"));
        }
    }

    fn count(&self, sql: &str) -> u64 {
        let sql = verify(sql, &self.ns, CATALOG).unwrap().remove(0);
        let count: i64 = self.conn.query_row(&sql, [], |row| row.get(0)).unwrap();
        u64::try_from(count).unwrap()
    }

    fn strings(&self, sql: &str) -> Vec<String> {
        let mut statement = self.conn.prepare(sql).unwrap();
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap();
        rows.collect::<Result<_, _>>().unwrap()
    }

    /// Every live schema, and for each live table or view its columns, row
    /// count, `sum(hash(row))` and a digest of its rows as text.
    fn live_snapshot(&self) -> BTreeMap<String, String> {
        let live = "table_catalog = current_database() AND table_schema NOT LIKE 'preview\\_%' ESCAPE '\\'";
        let mut snapshot = BTreeMap::new();
        let schemas = self.strings(
            "SELECT schema_name FROM information_schema.schemata \
             WHERE catalog_name = current_database() AND schema_name NOT LIKE 'preview\\_%' ESCAPE '\\' \
             ORDER BY 1",
        );
        snapshot.insert("schemas".into(), schemas.join(","));
        let tables = self.strings(&format!(
            "SELECT table_schema || '.' || table_name FROM information_schema.tables WHERE {live} ORDER BY 1"
        ));
        for table in tables {
            let (schema, name) = table.split_once('.').unwrap();
            let quoted = format!("\"{schema}\".\"{name}\"");
            let columns = self.strings(&format!(
                "SELECT string_agg(column_name || ' ' || data_type, ', ' ORDER BY ordinal_position) \
                 FROM information_schema.columns WHERE table_catalog = current_database() \
                 AND table_schema = '{schema}' AND table_name = '{name}'"
            ));
            let rows = self.strings(&format!(
                "SELECT CAST(count(*) AS VARCHAR) || ' ' || CAST(coalesce(sum(hash(t)), 0) AS VARCHAR) \
                 || ' ' || coalesce(md5(string_agg(CAST(t AS VARCHAR), chr(10) ORDER BY CAST(t AS VARCHAR))), '') \
                 FROM {quoted} AS t"
            ));
            snapshot.insert(table, format!("{} | {}", columns.join(""), rows.join("")));
        }
        snapshot
    }
}

#[test]
fn live_tables_are_byte_identical_after_pokehouse_shaped_sequences() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SEED).unwrap();
    let ns = PreviewNamespace::for_branch(Uuid::from_u128(42), "feat/je-v2");
    let prefix = ns.prefix();
    let mut host = Host {
        conn,
        ns,
        shadow: ShadowMap::default(),
        opts: RewriteOptions {
            catalog: CATALOG.map(str::to_string),
            read_live_only: false,
        },
    };
    let before = host.live_snapshot();
    assert!(
        before.len() > 7,
        "the seed made the live tables: {before:?}"
    );

    host.step(ACCOUNT_MAP_SEED);
    let reads = host.step(JOURNAL_ENTRY);
    assert!(reads.contains(&("bookkeeping".into(), "qb_je_account_map".into())));
    host.step(DAILY_ROLLUPS);
    host.step(SITE_SELECTION_REFRESH);
    host.step(SELF_INSERT);

    assert_eq!(host.live_snapshot(), before, "a live table changed");

    // The preview did the work: the copy-on-write kept the row the seed does
    // not own, the journal read the preview's map, the oversized rollup table
    // started empty, and the rename and view landed in the preview.
    let q = |sql: &str| host.strings(&sql.replace("P_", &prefix));
    assert_eq!(
        q(
            "SELECT string_agg(category || ':' || qb_account, ',' ORDER BY category) FROM P_bookkeeping.qb_je_account_map"
        ),
        vec!["cash:1000 Cash,sales:4000 Sales,tips:2150 Tips Payable"]
    );
    assert_eq!(
        q(
            "SELECT CAST(count(*) AS VARCHAR) FROM P_bookkeeping.journal_entry_log WHERE qb_account = '2150 Tips Payable'"
        ),
        vec!["10"]
    );
    assert_eq!(
        q(
            "SELECT CAST(count(*) AS VARCHAR) FROM P_toast_pos.sales_daily_metrics WHERE avg_ticket IS NOT NULL"
        ),
        vec!["50"]
    );
    assert_eq!(
        host.shadow
            .state(&("toast_pos".into(), "sales_daily_metrics".into())),
        Some(ShadowState::Partial)
    );
    assert_eq!(
        q(
            "SELECT CAST(count(*) AS VARCHAR) FROM P_toast_pos.location_daily_latest WHERE last_business_date = DATE '2026-01-10'"
        ),
        vec!["5"]
    );
    assert_eq!(
        q("SELECT CAST(count(refreshed_at) AS VARCHAR) FROM P_site_selection.site_hex_cells"),
        vec!["120"]
    );
    assert_eq!(
        q("SELECT CAST(count(*) AS VARCHAR) FROM P_site_selection.top_sites"),
        vec!["10"]
    );
    assert_eq!(
        host.shadow
            .state(&("site_selection".into(), "site_hex_cells_next".into())),
        Some(ShadowState::Dropped)
    );
    // Over the cap, the copy started empty and the write read the copy.
    assert_eq!(
        q("SELECT CAST(count(*) AS VARCHAR) FROM P_toast_pos.orders"),
        vec!["0"]
    );
    assert_eq!(
        host.shadow.state(&("toast_pos".into(), "orders".into())),
        Some(ShadowState::Partial)
    );
}
