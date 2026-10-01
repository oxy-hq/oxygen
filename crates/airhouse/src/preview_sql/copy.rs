//! What the host does before a rewritten step runs ([`Prelude`]), and how a
//! copy-on-write settles what the preview holds ([`CopyPlan`]).

use super::{Rewrite, ShadowMap, ShadowState};
use sqlparser::ast::Ident;

/// Rows a copy-on-write copies whole; over it the copy starts empty and the
/// table is [`ShadowState::Partial`]. Override with [`COW_MAX_ROWS_VAR`].
pub const DEFAULT_COW_MAX_ROWS: u64 = 5_000_000;
pub const COW_MAX_ROWS_VAR: &str = "OXY_PREVIEW_COW_MAX_ROWS";

/// The copy-on-write cap: `OXY_PREVIEW_COW_MAX_ROWS`, or
/// [`DEFAULT_COW_MAX_ROWS`] when unset.
pub fn cow_max_rows() -> Result<u64, String> {
    parse_cow_max_rows(std::env::var(COW_MAX_ROWS_VAR).ok().as_deref())
}

/// [`cow_max_rows`] for a given value of the variable. A value that is not a
/// row count is an error, not a silent default.
pub fn parse_cow_max_rows(value: Option<&str>) -> Result<u64, String> {
    match value.map(str::trim) {
        None | Some("") => Ok(DEFAULT_COW_MAX_ROWS),
        Some(v) => v
            .parse()
            .map_err(|_| format!("{COW_MAX_ROWS_VAR}={v:?} is not a row count")),
    }
}

/// Work the host does before sending a [`Rewrite::sql`], in order. Names are
/// lowercase `(schema, table)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prelude {
    /// [`Prelude::statement`]: `CREATE SCHEMA IF NOT EXISTS <preview>`.
    EnsureSchema { live: String, preview: String },
    /// Copy the live table into the preview before a write that starts from
    /// it. The host runs [`Prelude::live_table_probe`]; when the table exists,
    /// [`Prelude::live_rows`]; then [`Prelude::copy_plan`] with the cap from
    /// [`cow_max_rows`], runs its statement, and settles the preview with
    /// [`ShadowMap::apply_rewrite`].
    CopyOnWrite {
        live: (String, String),
        preview: (String, String),
    },
}

/// A copy-on-write the host decided on: run `statement`, then the table is
/// `state`. Pass every plan to [`ShadowMap::apply_rewrite`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyPlan {
    pub live: (String, String),
    pub statement: String,
    pub state: ShadowState,
}

impl Prelude {
    /// The statement for an [`Prelude::EnsureSchema`], in `catalog` when the
    /// SQL names one; `None` for a copy (see [`Prelude::copy_plan`]).
    pub fn statement(&self, catalog: Option<&str>) -> Option<String> {
        let Prelude::EnsureSchema { preview, .. } = self else {
            return None;
        };
        let cat = catalog_prefix(catalog);
        Some(format!(
            "CREATE SCHEMA IF NOT EXISTS {cat}{}",
            quote(preview)
        ))
    }

    /// For a copy: a query counting 1 when the live table exists, else 0.
    /// Filtered by catalog only when the workspace names one: over Airhouse's
    /// wire, `current_database()` need not be the DuckLake catalog the tables
    /// live in, so guessing it would find nothing and skip every copy.
    pub fn live_table_probe(&self, catalog: Option<&str>) -> Option<String> {
        let Prelude::CopyOnWrite { live, .. } = self else {
            return None;
        };
        let in_catalog = catalog.map_or(String::new(), |c| {
            format!("table_catalog = {} AND ", literal(c))
        });
        Some(format!(
            "SELECT count(*) FROM information_schema.tables WHERE {in_catalog}\
             lower(table_schema) = {} AND lower(table_name) = {}",
            literal(&live.0),
            literal(&live.1)
        ))
    }

    /// For a copy: `SELECT count(*)` of the live table. Run it only when the
    /// probe found the table.
    pub fn live_rows(&self, catalog: Option<&str>) -> Option<String> {
        let Prelude::CopyOnWrite { live, .. } = self else {
            return None;
        };
        let cat = catalog_prefix(catalog);
        Some(format!(
            "SELECT count(*) FROM {cat}{}.{}",
            quote(&live.0),
            quote(&live.1)
        ))
    }

    /// For a copy, what to run given the probe (`live_exists`), the live
    /// table's `live_rows` and the `cap`: nothing when the live table does
    /// not exist (the write meets the missing table, or creates it, as in
    /// prod); a whole copy, [`ShadowState::Shadow`], up to the cap; above it
    /// an empty copy of its columns, [`ShadowState::Partial`], so nothing
    /// later in the step reads every live row.
    pub fn copy_plan(
        &self,
        catalog: Option<&str>,
        live_exists: bool,
        live_rows: u64,
        cap: u64,
    ) -> Option<CopyPlan> {
        let Prelude::CopyOnWrite { live, preview } = self else {
            return None;
        };
        if !live_exists {
            return None;
        }
        let whole = live_rows <= cap;
        let cat = catalog_prefix(catalog);
        let statement = format!(
            "CREATE OR REPLACE TABLE {cat}{}.{} AS SELECT * FROM {cat}{}.{}{}",
            quote(&preview.0),
            quote(&preview.1),
            quote(&live.0),
            quote(&live.1),
            if whole { "" } else { " LIMIT 0" }
        );
        Some(CopyPlan {
            live: live.clone(),
            statement,
            state: if whole {
                ShadowState::Shadow
            } else {
                ShadowState::Partial
            },
        })
    }
}

impl CopyPlan {
    /// The same copy, refusing to replace a table already there (`CREATE
    /// TABLE`, not `CREATE OR REPLACE TABLE`). For a copy planned again
    /// because a listing did not show the preview's table: a listing that lags
    /// behind the Writer must fail the step, never overwrite what the preview
    /// has written since.
    pub fn strict(mut self) -> Self {
        if let Some(rest) = self.statement.strip_prefix("CREATE OR REPLACE TABLE ") {
            self.statement = format!("CREATE TABLE {rest}");
        }
        self
    }
}

impl ShadowMap {
    /// Settle the preview after a rewritten step ran: each copy the host made
    /// (`copies`), then each of [`Rewrite::shadow_updates`] in order, where
    /// an update that depends on a copy (a table renamed after its copy, or
    /// `CREATE TABLE IF NOT EXISTS` over one) takes the copy's state when
    /// there was a copy and its own otherwise.
    pub fn apply_rewrite(&mut self, rewrite: &Rewrite, copies: &[CopyPlan]) {
        let copied = |live: &(String, String)| {
            copies
                .iter()
                .find(|plan| plan.live == *live)
                .map(|plan| plan.state)
        };
        for plan in copies {
            self.0.insert(plan.live.clone(), plan.state);
        }
        for (i, (live, state)) in rewrite.shadow_updates.iter().enumerate() {
            let settled = rewrite
                .from_copy
                .iter()
                .find(|(at, _)| *at == i)
                .and_then(|(_, source)| copied(source))
                .unwrap_or(*state);
            self.0.insert(live.clone(), settled);
        }
    }
}

fn quote(ident: &str) -> String {
    Ident::with_quote('"', ident).to_string()
}

fn catalog_prefix(catalog: Option<&str>) -> String {
    catalog.map_or(String::new(), |c| format!("{}.", quote(c)))
}

fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::super::{PreviewNamespace, RewriteOptions, rewrite};
    use super::*;

    fn pair(schema: &str, table: &str) -> (String, String) {
        (schema.into(), table.into())
    }

    fn copy() -> Prelude {
        Prelude::CopyOnWrite {
            live: pair("toast_pos", "orders"),
            preview: pair("preview_k_000000__toast_pos", "orders"),
        }
    }

    #[test]
    fn the_cap_defaults_and_refuses_a_value_that_is_not_a_count() {
        assert_eq!(parse_cow_max_rows(None), Ok(DEFAULT_COW_MAX_ROWS));
        assert_eq!(parse_cow_max_rows(Some(" ")), Ok(DEFAULT_COW_MAX_ROWS));
        assert_eq!(parse_cow_max_rows(Some("250")), Ok(250));
        assert!(parse_cow_max_rows(Some("5M")).is_err());
        assert!(parse_cow_max_rows(Some("-1")).is_err());
    }

    #[test]
    fn a_copy_is_whole_up_to_the_cap_empty_over_it_and_absent_without_a_live_table() {
        assert_eq!(copy().copy_plan(None, false, 0, 10), None);
        let whole = copy().copy_plan(Some("lake"), true, 10, 10).unwrap();
        assert_eq!(whole.state, ShadowState::Shadow);
        assert_eq!(
            whole.statement,
            "CREATE OR REPLACE TABLE \"lake\".\"preview_k_000000__toast_pos\".\"orders\" AS \
             SELECT * FROM \"lake\".\"toast_pos\".\"orders\""
        );
        let empty = copy().copy_plan(None, true, 11, 10).unwrap();
        assert_eq!(empty.state, ShadowState::Partial);
        assert!(empty.statement.ends_with(" LIMIT 0"), "{}", empty.statement);
        let ensure = Prelude::EnsureSchema {
            live: "toast_pos".into(),
            preview: "preview_k_000000__toast_pos".into(),
        };
        assert_eq!(ensure.copy_plan(None, true, 1, 10), None);
        assert_eq!(copy().statement(None), None);
    }

    /// A strict copy fails on a table already there instead of replacing it.
    #[test]
    fn a_strict_copy_never_replaces_the_previews_table() {
        let plan = copy().copy_plan(None, true, 3, 10).unwrap();
        assert!(plan.statement.starts_with("CREATE OR REPLACE TABLE "));
        let strict = plan.clone().strict();
        assert!(
            strict.statement.starts_with("CREATE TABLE "),
            "{}",
            strict.statement
        );
        assert_eq!((strict.live, strict.state), (plan.live, plan.state));

        let db = duckdb::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE SCHEMA toast_pos; CREATE TABLE toast_pos.orders AS SELECT 1 AS id; \
             CREATE SCHEMA preview_k_000000__toast_pos; \
             CREATE TABLE preview_k_000000__toast_pos.orders AS SELECT * FROM range(5) t(id);",
        )
        .unwrap();
        assert!(db.execute_batch(&strict.statement).is_err(), "it replaced");
        let kept: i64 = db
            .query_row(
                "SELECT count(*) FROM preview_k_000000__toast_pos.orders",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 5, "the preview's rows are kept");
    }

    /// No catalog named: the probe does not guess one (`current_database()`
    /// over Airhouse's wire need not be the catalog the table is in). DuckDB,
    /// with the live table in an attached catalog that is not the current
    /// one, finds it only without the guess.
    #[test]
    fn the_probe_filters_by_catalog_only_when_one_is_named() {
        let probe = copy().live_table_probe(None).unwrap();
        assert!(!probe.contains("table_catalog"), "{probe}");
        let named = copy().live_table_probe(Some("lake")).unwrap();
        assert!(named.contains("table_catalog = 'lake' AND "), "{named}");

        let db = duckdb::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "ATTACH ':memory:' AS lake; CREATE SCHEMA lake.toast_pos; \
             CREATE TABLE lake.toast_pos.orders (id INTEGER);",
        )
        .unwrap();
        let count = |sql: &str| db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count(&probe), 1);
        assert_eq!(count(&named), 1);
        let guessed = probe.replace("WHERE ", "WHERE table_catalog = current_database() AND ");
        assert_eq!(count(&guessed), 0, "control: the guess finds nothing");
    }

    #[test]
    fn create_if_not_exists_over_a_copy_keeps_the_copys_state_and_creates_without_one() {
        let ns = PreviewNamespace::from_key("feat_000000").unwrap();
        let sql = "CREATE TABLE IF NOT EXISTS toast_pos.orders (id INT)";
        let out = rewrite(sql, &ns, &ShadowMap::default(), &RewriteOptions::default()).unwrap();
        let Some(prelude @ Prelude::CopyOnWrite { .. }) = out.preludes.get(1) else {
            panic!("expected a copy: {:?}", out.preludes);
        };
        let over_cap = prelude.copy_plan(None, true, 11, 10).unwrap();
        let mut copied = ShadowMap::default();
        copied.apply_rewrite(&out, &[over_cap]);
        assert_eq!(
            copied.state(&pair("toast_pos", "orders")),
            Some(ShadowState::Partial)
        );
        let mut created = ShadowMap::default();
        created.apply_rewrite(&out, &[]);
        assert_eq!(
            created.state(&pair("toast_pos", "orders")),
            Some(ShadowState::Shadow)
        );
    }
}
