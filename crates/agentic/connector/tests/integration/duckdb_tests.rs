//! Integration tests for the DuckDB connector.
//!
//! Migrated from the removed `agentic-connector-duckdb` crate.
//!
//! Non-file tests use `DuckDbConnector::new(conn)` with data pre-loaded into
//! the connection before handing it to the connector.  File-based tests use a
//! `tempfile` directory and write Parquet/CSV there first.

#[cfg(feature = "duckdb")]
mod duckdb {
    use agentic_connector::{
        ConnectorError, DatabaseConnector, DuckDbConnection, DuckDbConnector, LoadStrategy,
        ResultCap,
    };
    use agentic_core::result::{
        CellValue, ColumnSpec, TypedDataType, TypedRowError, TypedRowStream, TypedValue,
    };
    use futures::StreamExt;

    /// Drain a [`TypedRowStream`] into a flat `Vec<Vec<TypedValue>>`, failing
    /// the test on any per-row error. Used across typed-stream tests.
    async fn collect_typed(stream: TypedRowStream) -> (Vec<ColumnSpec>, Vec<Vec<TypedValue>>) {
        let TypedRowStream {
            columns, mut rows, ..
        } = stream;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await {
            match row {
                Ok(cells) => out.push(cells),
                Err(e) => panic!("row stream error: {e}"),
            }
        }
        (columns, out)
    }

    /// A 1M-row `range()` scan must come back as a bounded *partial* result with
    /// the truncation flag set — the [`ResultCap`] backstop on the eager
    /// `execute_query_full` collection loop.
    #[tokio::test]
    async fn result_cap_truncates_runaway_scan() {
        let conn = DuckDbConnection::open_in_memory().unwrap();
        let connector = DuckDbConnector::new(conn).with_result_cap(ResultCap {
            max_rows: 5,
            max_bytes: u64::MAX,
        });

        let stream = connector
            .execute_query_full("SELECT i FROM range(1000000) t(i)")
            .await
            .expect("cap returns a partial result, not an error");

        let TypedRowStream {
            mut rows,
            truncated,
            ..
        } = stream;
        let mut count = 0usize;
        while let Some(row) = rows.next().await {
            row.expect("row decodes");
            count += 1;
        }

        assert!(
            (1..=5).contains(&count),
            "row cap must bound the runaway scan, got {count}"
        );
        assert!(
            agentic_core::result::truncation_flag_set(&truncated),
            "connector must flag the partial result truncated"
        );
    }

    // ensure TypedRowError is exported / usable for negative-path tests
    #[allow(dead_code)]
    fn _types_export_sanity(_e: TypedRowError) {}

    // ── helpers ───────────────────────────────────────────────────────────────

    /// Build a connector with a simple `sales` table pre-populated.
    ///
    /// Schema: id INTEGER, name TEXT, amount DOUBLE, tag TEXT
    ///   (row 3 has tag = NULL)
    fn make_sales_connector() -> DuckDbConnector {
        let conn = DuckDbConnection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sales (id INTEGER, name TEXT, amount DOUBLE, tag TEXT);
             INSERT INTO sales VALUES
                 (1, 'alpha', 10.0, 'a'),
                 (2, 'beta',  20.0, 'b'),
                 (3, 'gamma', 30.0, NULL);",
        )
        .unwrap();
        DuckDbConnector::new(conn)
    }

    // ── execute_query ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn basic_query() {
        let c = make_sales_connector();
        let res = c
            .execute_query("SELECT * FROM sales ORDER BY id", 100)
            .await
            .unwrap();

        assert_eq!(res.result.total_row_count, 3);
        assert_eq!(res.result.rows.len(), 3);
        assert_eq!(res.result.columns, ["id", "name", "amount", "tag"]);
        assert!(!res.result.truncated);

        let row0 = &res.result.rows[0].0;
        assert_eq!(row0[0], CellValue::Number(1.0));
        assert_eq!(row0[1], CellValue::Text("alpha".into()));
        assert_eq!(row0[2], CellValue::Number(10.0));
        assert_eq!(row0[3], CellValue::Text("a".into()));
    }

    #[tokio::test]
    async fn truncation() {
        let c = make_sales_connector();
        let res = c.execute_query("SELECT * FROM sales", 2).await.unwrap();

        assert_eq!(res.result.total_row_count, 3);
        assert_eq!(res.result.rows.len(), 2);
        assert!(res.result.truncated);
    }

    #[tokio::test]
    async fn empty_result() {
        let c = make_sales_connector();
        let res = c
            .execute_query("SELECT * FROM sales WHERE 1 = 0", 100)
            .await
            .unwrap();

        assert_eq!(res.result.total_row_count, 0);
        assert!(res.result.rows.is_empty());
        assert!(!res.result.truncated);
        assert_eq!(res.summary.row_count, 0);
    }

    /// A multi-statement DDL/DML script ending in a returning SELECT — the
    /// shape of an automation `execute_sql` setup file
    /// (`CREATE TABLE …; CREATE INDEX …; INSERT …; SELECT 'ready'`).
    /// Regression test: previously the connector wrapped the whole script
    /// in `CREATE TEMP TABLE _t AS (…)` and DuckDB rejected it with
    /// `Parser Error: syntax error at or near "CREATE"`.
    #[tokio::test]
    async fn multi_statement_script_runs_and_samples_final_select() {
        let conn = DuckDbConnection::open_in_memory().unwrap();
        let c = DuckDbConnector::new(conn);

        let script = "CREATE OR REPLACE TABLE video_activity (\
                          video_id VARCHAR NOT NULL,\
                          d DATE NOT NULL,\
                          views BIGINT,\
                          PRIMARY KEY (video_id, d)\
                      );\n\
                      CREATE INDEX IF NOT EXISTS va_d_idx ON video_activity (d);\n\
                      CREATE INDEX IF NOT EXISTS va_v_idx ON video_activity (views);\n\
                      INSERT INTO video_activity VALUES ('v1', DATE '2026-01-01', 100);\n\
                      -- workaround comment with ; semicolon\n\
                      SELECT 'video_activity ready' AS status;";

        let res = c.execute_query(script, 100).await.unwrap();

        assert_eq!(res.result.columns, ["status"]);
        assert_eq!(res.result.total_row_count, 1);
        assert_eq!(res.result.rows.len(), 1);
        assert_eq!(
            res.result.rows[0].0[0],
            CellValue::Text("video_activity ready".into())
        );

        // The prefix statements actually ran: the table and its row exist.
        let check = c
            .execute_query("SELECT views FROM video_activity WHERE video_id = 'v1'", 10)
            .await
            .unwrap();
        assert_eq!(check.result.rows[0].0[0], CellValue::Number(100.0));
    }

    /// A multi-statement script whose final statement is non-returning
    /// DDL/DML: the side effects must apply and the call returns a
    /// well-formed empty result instead of a parser error.
    #[tokio::test]
    async fn multi_statement_script_non_returning_final_is_empty_result() {
        let conn = DuckDbConnection::open_in_memory().unwrap();
        let c = DuckDbConnector::new(conn);

        let res = c
            .execute_query(
                "CREATE TABLE t (a INT);\nINSERT INTO t VALUES (1);\nINSERT INTO t VALUES (2);",
                100,
            )
            .await
            .unwrap();

        assert!(res.result.columns.is_empty());
        assert!(res.result.rows.is_empty());
        assert_eq!(res.result.total_row_count, 0);
        assert!(!res.result.truncated);

        // Both inserts applied.
        let check = c.execute_query("SELECT COUNT(*) FROM t", 10).await.unwrap();
        assert_eq!(check.result.rows[0].0[0], CellValue::Number(2.0));
    }

    /// A single non-returning statement (no semicolons) is also handled:
    /// run for side effects, empty result.
    /// Regression: `date_diff` with a bare string-literal part, projected
    /// non-aggregated over a *populated* `GROUP BY`, only binds when the
    /// DuckDB ICU extension is loaded. The connector must load ICU on
    /// every connection (matching oxy-core); without it this fails with
    /// `No function matches … (STRING_LITERAL, DATE, DATE)` and an empty
    /// candidate list. Reproduces the gallant-demo velocity automation bug.
    #[tokio::test]
    async fn date_diff_string_literal_over_group_by_binds_with_icu() {
        let conn = DuckDbConnection::open_in_memory().unwrap();
        let c = DuckDbConnector::new(conn);

        c.execute_query(
            "CREATE TABLE va (id VARCHAR, publish_date TIMESTAMP, d DATE, views BIGINT);\n\
             INSERT INTO va VALUES \
                ('v1', TIMESTAMP '2024-01-01 00:00:00', DATE '2024-01-03', 10),\
                ('v1', TIMESTAMP '2024-01-01 00:00:00', DATE '2024-01-20', 20),\
                ('v2', TIMESTAMP '2024-02-01 00:00:00', DATE '2024-02-02', 30);",
            100,
        )
        .await
        .unwrap();

        // Non-aggregated date_diff over a GROUP BY key (the days_live shape)
        // plus an aggregated date_diff inside SUM(CASE …) (the window shape).
        let res = c
            .execute_query(
                "SELECT id, \
                    date_diff('day', publish_date::DATE, CURRENT_DATE::DATE) AS days_live, \
                    SUM(CASE WHEN date_diff('day', publish_date::DATE, d::DATE) < 7 \
                             THEN views ELSE 0 END) AS v7 \
                 FROM va GROUP BY id, publish_date ORDER BY id",
                100,
            )
            .await
            .unwrap();

        assert_eq!(res.result.columns, ["id", "days_live", "v7"]);
        assert_eq!(res.result.total_row_count, 2);
        // v1: only the 2024-01-03 row is within 7 days of publish → 10.
        assert_eq!(res.result.rows[0].0[0], CellValue::Text("v1".into()));
        assert_eq!(res.result.rows[0].0[2], CellValue::Number(10.0));
    }

    /// A single non-returning statement (no semicolons) is also handled:
    /// run for side effects, empty result.
    #[tokio::test]
    async fn single_non_returning_statement_is_empty_result() {
        let conn = DuckDbConnection::open_in_memory().unwrap();
        let c = DuckDbConnector::new(conn);

        let res = c
            .execute_query("CREATE TABLE solo (a INT)", 100)
            .await
            .unwrap();
        assert!(res.result.columns.is_empty());
        assert_eq!(res.result.total_row_count, 0);

        // Table exists and is queryable afterwards.
        c.execute_query("SELECT * FROM solo", 10).await.unwrap();
    }

    #[tokio::test]
    async fn stats_correctness() {
        let c = make_sales_connector();
        let res = c.execute_query("SELECT * FROM sales", 100).await.unwrap();

        let amount = res
            .summary
            .columns
            .iter()
            .find(|s| s.name == "amount")
            .unwrap();
        assert_eq!(amount.null_count, 0);
        assert_eq!(amount.distinct_count, Some(3));
        assert_eq!(amount.min, Some(CellValue::Number(10.0)));
        assert_eq!(amount.max, Some(CellValue::Number(30.0)));
        assert!((amount.mean.unwrap() - 20.0).abs() < 1e-9);
        let expected_std = f64::sqrt(((10f64 - 20.0).powi(2) + 0.0 + (30f64 - 20.0).powi(2)) / 3.0);
        assert!((amount.std_dev.unwrap() - expected_std).abs() < 1e-6);

        let tag = res
            .summary
            .columns
            .iter()
            .find(|s| s.name == "tag")
            .unwrap();
        assert_eq!(tag.null_count, 1);
        assert_eq!(tag.distinct_count, Some(2));
        assert!(tag.mean.is_none());
        assert!(tag.std_dev.is_none());
    }

    #[tokio::test]
    async fn sql_error() {
        let c = DuckDbConnector::in_memory().unwrap();
        let err = c
            .execute_query("SELECT * FROM nonexistent_table_xyz", 100)
            .await
            .unwrap_err();
        assert!(matches!(err, ConnectorError::QueryFailed(_)));
    }

    #[tokio::test]
    async fn dialect_is_duckdb() {
        use agentic_connector::SqlDialect;
        let c = DuckDbConnector::in_memory().unwrap();
        assert_eq!(c.dialect(), SqlDialect::DuckDb);
        assert_eq!(c.dialect().as_str(), "DuckDB");
    }

    // ── file loading ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn from_directory_parquet() {
        let dir = tempfile::tempdir().unwrap();
        let parquet_path = dir.path().join("orders.parquet");

        let wconn = DuckDbConnection::open_in_memory().unwrap();
        wconn
            .execute_batch(&format!(
                "COPY (SELECT 1 AS order_id, 100.0 AS total \
                       UNION ALL SELECT 2 AS order_id, 200.0 AS total) \
                 TO '{}' (FORMAT PARQUET)",
                parquet_path.display()
            ))
            .unwrap();

        let c = DuckDbConnector::from_directory(dir.path(), LoadStrategy::View).unwrap();
        let res = c
            .execute_query("SELECT * FROM orders ORDER BY order_id", 100)
            .await
            .unwrap();

        assert_eq!(res.result.total_row_count, 2);
        assert_eq!(res.result.columns, ["order_id", "total"]);
    }

    #[tokio::test]
    async fn from_directory_csv() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("products.csv"),
            "product_id,price\n1,9.99\n2,19.99\n3,29.99\n",
        )
        .unwrap();

        let c = DuckDbConnector::from_directory(dir.path(), LoadStrategy::View).unwrap();
        let res = c
            .execute_query("SELECT * FROM products ORDER BY product_id", 100)
            .await
            .unwrap();

        assert_eq!(res.result.total_row_count, 3);
        assert_eq!(res.result.columns, ["product_id", "price"]);
    }

    /// Semantic views declare `table: "oxymart.csv"` (with extension), so the
    /// connector must expose the file under its full filename too, not just
    /// the stem.
    #[tokio::test]
    async fn csv_accessible_by_full_filename() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("oxymart.csv"),
            "Date,Store\n2024-01-01,1\n2024-01-08,1\n2024-01-15,2\n",
        )
        .unwrap();

        let c = DuckDbConnector::from_directory(dir.path(), LoadStrategy::View).unwrap();

        // Stem name still works.
        let by_stem = c
            .execute_query(
                "SELECT DISTINCT Date FROM oxymart WHERE Date IS NOT NULL",
                100,
            )
            .await
            .unwrap();
        assert_eq!(by_stem.result.total_row_count, 3);

        // Full filename (quoted) also resolves.
        let by_full = c
            .execute_query(
                r#"SELECT DISTINCT Date FROM "oxymart.csv" WHERE Date IS NOT NULL"#,
                100,
            )
            .await
            .unwrap();
        assert_eq!(by_full.result.total_row_count, 3);

        // Exactly matches the failing sample_columns query from the user report:
        // semantic-model `year` dimension computed as EXTRACT(YEAR FROM CAST(...)).
        let by_expr = c
            .execute_query(
                r#"SELECT DISTINCT EXTRACT(YEAR FROM CAST(Date AS DATE)) FROM "oxymart.csv" WHERE EXTRACT(YEAR FROM CAST(Date AS DATE)) IS NOT NULL LIMIT 20"#,
                20,
            )
            .await
            .unwrap();
        assert_eq!(by_expr.result.total_row_count, 1);
    }

    /// The semantic-model compiler renders `table: orders.parquet` as an
    /// UNQUOTED `FROM orders.parquet`, which DuckDB parses as schema.table. The
    /// single-identifier `"orders.parquet"` alias only matches the quoted form,
    /// so the unquoted reference must resolve via a registered `"orders"."parquet"`
    /// catalog view rather than the environment-sensitive file-replacement scan
    /// (which silently returns nothing for Parquet under `oxy serve`).
    #[tokio::test]
    async fn parquet_accessible_by_unquoted_schema_table() {
        let dir = tempfile::tempdir().unwrap();
        let parquet_path = dir.path().join("orders.parquet");
        let wconn = DuckDbConnection::open_in_memory().unwrap();
        wconn
            .execute_batch(&format!(
                "COPY (SELECT 1 AS order_id UNION ALL SELECT 2 UNION ALL SELECT 3) \
                 TO '{}' (FORMAT PARQUET)",
                parquet_path.display()
            ))
            .unwrap();

        let c = DuckDbConnector::from_directory(dir.path(), LoadStrategy::View).unwrap();

        // Unquoted schema.table form — exactly what airlayer emits for `table:`.
        let unquoted = c
            .execute_query("SELECT count(*) AS n FROM orders.parquet", 10)
            .await
            .unwrap();
        assert_eq!(unquoted.result.total_row_count, 1);

        // Fully-quoted "<stem>"."<ext>" resolves ONLY through the catalog view
        // (the file-replacement scan never fires for a quoted identifier), so
        // this proves the schema-qualified alias exists.
        let qualified = c
            .execute_query(
                r#"SELECT order_id FROM "orders"."parquet" ORDER BY order_id"#,
                10,
            )
            .await
            .unwrap();
        assert_eq!(qualified.result.total_row_count, 3);
    }

    #[tokio::test]
    async fn view_vs_materialized() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("items.csv"), "id,val\n1,10\n2,20\n").unwrap();

        let sql = "SELECT * FROM items ORDER BY id";
        let view_res = DuckDbConnector::from_directory(dir.path(), LoadStrategy::View)
            .unwrap()
            .execute_query(sql, 100)
            .await
            .unwrap();
        let mat_res = DuckDbConnector::from_directory(dir.path(), LoadStrategy::Materialized)
            .unwrap()
            .execute_query(sql, 100)
            .await
            .unwrap();

        assert_eq!(
            view_res.result.total_row_count,
            mat_res.result.total_row_count
        );
        assert_eq!(view_res.result.columns, mat_res.result.columns);
        assert_eq!(view_res.result.rows.len(), mat_res.result.rows.len());
    }

    #[tokio::test]
    async fn loaded_tables_metadata() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("events.csv"),
            "event_id,name\n1,click\n2,view\n",
        )
        .unwrap();

        let c = DuckDbConnector::from_directory(dir.path(), LoadStrategy::View).unwrap();
        let tables = c.loaded_tables();

        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].name, "events");
        assert!(tables[0].columns.iter().any(|(n, _)| n == "event_id"));
        assert!(tables[0].columns.iter().any(|(n, _)| n == "name"));
    }

    #[tokio::test]
    async fn parquet_preferred_over_csv_on_collision() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(dir.path().join("data.csv"), "id,src\n1,csv\n").unwrap();
        let wconn = DuckDbConnection::open_in_memory().unwrap();
        wconn
            .execute_batch(&format!(
                "COPY (SELECT 2 AS id, 'parquet' AS src) TO '{}' (FORMAT PARQUET)",
                dir.path().join("data.parquet").display()
            ))
            .unwrap();

        let c = DuckDbConnector::from_directory(dir.path(), LoadStrategy::View).unwrap();
        assert_eq!(
            c.loaded_tables().len(),
            1,
            "collision must yield exactly one table"
        );

        let res = c.execute_query("SELECT src FROM data", 10).await.unwrap();
        assert_eq!(
            res.result.rows[0].0[0],
            CellValue::Text("parquet".into()),
            "Parquet must win over CSV on stem collision"
        );
    }

    #[tokio::test]
    async fn from_files_per_file_strategy() {
        let dir = tempfile::tempdir().unwrap();
        let csv_path = dir.path().join("small.csv");
        std::fs::write(&csv_path, "x\n1\n2\n3\n").unwrap();

        let c = DuckDbConnector::from_files(&[(&csv_path, LoadStrategy::Materialized)]).unwrap();
        let res = c
            .execute_query("SELECT * FROM small ORDER BY x", 100)
            .await
            .unwrap();

        assert_eq!(res.result.total_row_count, 3);
        assert_eq!(res.result.columns, ["x"]);
    }

    // ── introspect_schema ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn introspect_schema_finds_tables_and_columns() {
        let c = make_sales_connector();
        let info = c
            .introspect_schema()
            .expect("introspect_schema must succeed");

        let names: Vec<&str> = info.tables.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"sales"),
            "sales table must appear: {names:?}"
        );

        let sales = info.tables.iter().find(|t| t.name == "sales").unwrap();
        let col_names: Vec<&str> = sales.columns.iter().map(|c| c.name.as_str()).collect();
        assert!(col_names.contains(&"id"), "{col_names:?}");
        assert!(col_names.contains(&"name"), "{col_names:?}");
        assert!(col_names.contains(&"amount"), "{col_names:?}");
        assert!(col_names.contains(&"tag"), "{col_names:?}");
    }

    #[tokio::test]
    async fn introspect_schema_computes_min_max_for_numeric_column() {
        let c = make_sales_connector();
        let info = c.introspect_schema().unwrap();

        let sales = info.tables.iter().find(|t| t.name == "sales").unwrap();
        let amount_col = sales.columns.iter().find(|c| c.name == "amount").unwrap();

        assert_eq!(amount_col.min, Some(CellValue::Number(10.0)));
        assert_eq!(amount_col.max, Some(CellValue::Number(30.0)));
    }

    #[tokio::test]
    async fn introspect_schema_collects_sample_values() {
        let c = make_sales_connector();
        let info = c.introspect_schema().unwrap();

        let sales = info.tables.iter().find(|t| t.name == "sales").unwrap();
        let name_col = sales.columns.iter().find(|c| c.name == "name").unwrap();

        assert!(
            !name_col.sample_values.is_empty(),
            "name must have sample values"
        );
        let texts: Vec<_> = name_col
            .sample_values
            .iter()
            .filter_map(|v| {
                if let CellValue::Text(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(
            texts.iter().any(|s| ["alpha", "beta", "gamma"].contains(s)),
            "expected alpha/beta/gamma in samples: {texts:?}"
        );
    }

    #[tokio::test]
    async fn introspect_schema_sample_values_exclude_null() {
        let c = make_sales_connector();
        let info = c.introspect_schema().unwrap();

        let sales = info.tables.iter().find(|t| t.name == "sales").unwrap();
        let tag_col = sales.columns.iter().find(|c| c.name == "tag").unwrap();

        assert!(
            tag_col
                .sample_values
                .iter()
                .all(|v| !matches!(v, CellValue::Null)),
            "sample_values must not contain Null entries"
        );
    }

    #[tokio::test]
    async fn introspect_schema_detects_join_key_via_shared_id_column() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("orders.csv"),
            "order_id,customer_id,total\n1,10,50.0\n2,20,100.0\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("customers.csv"),
            "customer_id,region\n10,North\n20,South\n",
        )
        .unwrap();

        let c = DuckDbConnector::from_directory(dir.path(), LoadStrategy::View).unwrap();
        let info = c
            .introspect_schema()
            .expect("introspect_schema must succeed");

        let join = info
            .join_keys
            .iter()
            .find(|(_, _, col)| col == "customer_id");
        assert!(
            join.is_some(),
            "customer_id shared between tables must be detected as a join key"
        );
    }

    #[tokio::test]
    async fn introspect_schema_skips_system_tables() {
        let c = make_sales_connector();
        let info = c.introspect_schema().unwrap();

        for table in &info.tables {
            assert!(
                !table.name.starts_with("_agentic_"),
                "system table {} must be excluded",
                table.name
            );
            assert!(
                !table.name.to_lowercase().starts_with("information_schema"),
                "information_schema must not appear: {}",
                table.name
            );
        }
    }

    #[tokio::test]
    async fn introspect_schema_empty_db_returns_empty_schema_info() {
        let c = DuckDbConnector::in_memory().unwrap();
        let info = c.introspect_schema().expect("empty DB must not error");

        assert!(
            info.tables.is_empty(),
            "no tables in empty in-memory DuckDB"
        );
        assert!(
            info.join_keys.is_empty(),
            "no join keys in empty in-memory DuckDB"
        );
    }

    // ── execute_query_full ────────────────────────────────────────────────────

    #[tokio::test]
    async fn execute_query_full_preserves_int_and_float_types() {
        let c = make_sales_connector();
        let stream = c
            .execute_query_full("SELECT id, amount FROM sales ORDER BY id")
            .await
            .unwrap();
        let (columns, rows) = collect_typed(stream).await;

        assert_eq!(columns.len(), 2);
        assert_eq!(columns[0].name, "id");
        assert_eq!(columns[0].data_type, TypedDataType::Int32);
        assert_eq!(columns[1].name, "amount");
        assert_eq!(columns[1].data_type, TypedDataType::Float64);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0][0], TypedValue::Int32(1));
        assert_eq!(rows[0][1], TypedValue::Float64(10.0));
        assert_eq!(rows[2][0], TypedValue::Int32(3));
    }

    #[tokio::test]
    async fn execute_query_full_preserves_nulls() {
        let c = make_sales_connector();
        let stream = c
            .execute_query_full("SELECT tag FROM sales ORDER BY id")
            .await
            .unwrap();
        let (columns, rows) = collect_typed(stream).await;

        assert_eq!(columns[0].data_type, TypedDataType::Text);
        assert_eq!(rows[0][0], TypedValue::Text("a".into()));
        assert_eq!(rows[2][0], TypedValue::Null);
    }

    #[tokio::test]
    async fn execute_query_full_preserves_date_and_timestamp() {
        let conn = DuckDbConnection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t (d DATE, ts TIMESTAMP);
             INSERT INTO t VALUES
                ('2026-04-22', '2026-04-22 12:34:56'),
                (NULL, NULL);",
        )
        .unwrap();
        let c = DuckDbConnector::new(conn);

        let stream = c
            .execute_query_full("SELECT d, ts FROM t ORDER BY d NULLS LAST")
            .await
            .unwrap();
        let (columns, rows) = collect_typed(stream).await;

        assert_eq!(columns[0].data_type, TypedDataType::Date);
        assert_eq!(columns[1].data_type, TypedDataType::Timestamp);

        // 2026-04-22 = 2026 - 1970 = 56 * 365 + leaps. Verify by contract only:
        // the value must be a Date (i32) and Timestamp (i64) — exact values
        // are implementation-dependent.
        assert!(matches!(rows[0][0], TypedValue::Date(_)));
        assert!(matches!(rows[0][1], TypedValue::Timestamp(_)));

        assert_eq!(rows[1][0], TypedValue::Null);
        assert_eq!(rows[1][1], TypedValue::Null);
    }

    #[tokio::test]
    async fn execute_query_full_no_truncation() {
        let conn = DuckDbConnection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE big AS SELECT generate_series AS n FROM generate_series(1, 5000);",
        )
        .unwrap();
        let c = DuckDbConnector::new(conn);

        let stream = c.execute_query_full("SELECT n FROM big").await.unwrap();
        let (_, rows) = collect_typed(stream).await;

        assert_eq!(rows.len(), 5000, "execute_query_full must not truncate");
    }

    #[tokio::test]
    async fn execute_query_full_reports_query_errors() {
        let c = make_sales_connector();
        let err = c
            .execute_query_full("SELECT * FROM does_not_exist")
            .await
            .expect_err("unknown table must error");
        assert!(matches!(err, ConnectorError::QueryFailed(_)));
    }

    #[tokio::test]
    async fn execute_query_full_handles_decimal_precision() {
        let conn = DuckDbConnection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE money (amt DECIMAL(10,2));
             INSERT INTO money VALUES (123.45), (0.01), (-1000.00);",
        )
        .unwrap();
        let c = DuckDbConnector::new(conn);

        let stream = c
            .execute_query_full("SELECT amt FROM money ORDER BY amt")
            .await
            .unwrap();
        let (columns, rows) = collect_typed(stream).await;

        assert_eq!(
            columns[0].data_type,
            TypedDataType::Decimal {
                precision: 10,
                scale: 2
            }
        );
        assert_eq!(rows.len(), 3);
        for row in &rows {
            assert!(matches!(row[0], TypedValue::Decimal(_)));
        }
    }

    // ── Per-column stats with complex types ───────────────────────────────────
    //
    // Smoke test: queries that return columns of complex DuckDB types (MAP,
    // LIST, STRUCT, UNION, BLOB) must not break stats gathering. The current
    // bundled DuckDB version aggregates all of these gracefully, but the
    // connector still wraps each column's stats query in a per-column match
    // (mirroring the BigQuery connector) so a future type or extension that
    // rejects MIN/MAX/COUNT(DISTINCT) at bind time degrades just that one
    // column's stats to None instead of failing the whole execute_query.

    #[tokio::test]
    async fn stats_handle_complex_column_types() {
        let conn = DuckDbConnection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE mixed (
                 id INTEGER,
                 m MAP(VARCHAR, INTEGER),
                 lst INTEGER[],
                 stct STRUCT(a INTEGER, b VARCHAR)
             );
             INSERT INTO mixed VALUES
                 (1, MAP {'a': 1}, [1, 2, 3], {'a': 1, 'b': 'x'}),
                 (2, MAP {'b': 2}, [4, 5],    {'a': 2, 'b': 'y'});",
        )
        .unwrap();
        let c = DuckDbConnector::new(conn);

        let res = c
            .execute_query("SELECT * FROM mixed ORDER BY id", 100)
            .await
            .expect("execute_query must not fail when columns include complex types");

        assert_eq!(res.result.total_row_count, 2);
        assert_eq!(res.result.columns, ["id", "m", "lst", "stct"]);
        assert_eq!(
            res.summary.columns.len(),
            4,
            "every input column must produce a stats entry, even if degraded"
        );

        let id = res
            .summary
            .columns
            .iter()
            .find(|s| s.name == "id")
            .expect("id stats must be present");
        assert_eq!(id.null_count, 0);
        assert_eq!(id.distinct_count, Some(2));
        assert_eq!(id.min, Some(CellValue::Number(1.0)));
        assert_eq!(id.max, Some(CellValue::Number(2.0)));
        assert!(id.mean.is_some(), "INTEGER column must have a mean");
    }

    // ── AsArrowConnector (feature = "arrow") ────────────────────────────────

    #[cfg(feature = "arrow")]
    mod arrow_ext {
        use super::*;
        use agentic_connector::AsArrowConnector;
        use futures::StreamExt;

        #[tokio::test]
        async fn as_arrow_returns_some_for_duckdb() {
            let c = make_sales_connector();
            let conn: &dyn DatabaseConnector = &c;
            assert!(
                conn.as_arrow().is_some(),
                "DuckDB should expose an AsArrowConnector"
            );
        }

        #[tokio::test]
        async fn execute_query_arrow_round_trip() {
            let c = make_sales_connector();
            let stream = c
                .execute_query_arrow("SELECT id, name FROM sales ORDER BY id")
                .await
                .unwrap();

            assert_eq!(stream.schema.fields().len(), 2);
            assert_eq!(stream.schema.field(0).name(), "id");

            let batches: Vec<_> = stream.batches.collect().await;
            let total_rows: usize = batches
                .iter()
                .map(|b| b.as_ref().map(|rb| rb.num_rows()).unwrap_or(0))
                .sum();
            assert_eq!(total_rows, 3);
        }
    }

    /// A failed init statement of a `DuckDbRaw` connector (how a DuckLake is
    /// attached) reaches the caller, and through it the HTTP response, with
    /// its credentials scrubbed from both the echoed SQL and DuckDB's message.
    /// The second statement is the pre-fix ATTACH, password inline; its
    /// parser error quotes it. Parser errors, so no extension is downloaded.
    #[test]
    fn a_failed_init_statement_error_carries_no_secret() {
        for stmt in [
            "CREATE OR REPLACE SECRET (TYPE postgres PASSWORD 'hunter2-x')",
            "ATTACH 'ducklake:postgres:password=hunter2-x host=127.0.0.1' 'x' AS ducklake",
        ] {
            let cfg =
                agentic_connector::ConnectorConfig::DuckDbRaw(agentic_connector::DuckDbRawConfig {
                    init_statements: vec![stmt.to_string()],
                    secret_values: vec!["hunter2-x".to_string()],
                });

            let err = match agentic_connector::build_connector(cfg) {
                Ok(_) => panic!("{stmt} should not parse"),
                Err(e) => e,
            };
            let ConnectorError::QueryFailed(details) = &err else {
                panic!("expected QueryFailed, got {err:?}");
            };
            let shown = err.to_string();

            assert!(!shown.contains("hunter2"), "{shown}");
            assert!(details.sql.contains("***"), "{}", details.sql);
            assert!(
                details.message.contains("***"),
                "the message quoted it: {}",
                details.message
            );
        }
    }

    /// A failed ATTACH whose caller did not list the password in
    /// `secret_values` still comes back without it: the error scrubs anything
    /// shaped like a credential (libpq `password=`, a `PASSWORD '…'` literal).
    /// This is the shape Workspace Health once stored and showed. Parser
    /// errors, so no extension is downloaded.
    #[test]
    fn a_failed_attach_error_carries_no_password_it_was_not_told_about() {
        for stmt in [
            "ATTACH 'ducklake:postgres:user=ducklake password=synthetic%2FPa55 \
             host=db.internal dbname=ducklake_catalog sslmode=require' 'x' AS ducklake",
            "CREATE OR REPLACE SECRET (TYPE postgres PASSWORD 'synthetic-Pa55')",
        ] {
            let cfg =
                agentic_connector::ConnectorConfig::DuckDbRaw(agentic_connector::DuckDbRawConfig {
                    init_statements: vec![stmt.to_string()],
                    secret_values: Vec::new(),
                });

            let err = match agentic_connector::build_connector(cfg) {
                Ok(_) => panic!("{stmt} should not parse"),
                Err(e) => e,
            };
            let ConnectorError::QueryFailed(details) = &err else {
                panic!("expected QueryFailed, got {err:?}");
            };

            for shown in [
                err.to_string(),
                format!("{err:?}"),
                details.sql.clone(),
                details.message.clone(),
            ] {
                assert!(!shown.contains("Pa55"), "{shown}");
            }
            assert!(details.sql.contains("***"), "{}", details.sql);
        }
    }

    /// A `DuckDbUrl` open failure (MotherDuck's token rides in the URL) and the
    /// config's `Debug` both come out without the token.
    #[test]
    fn a_failed_url_open_error_carries_no_token() {
        let url_cfg = agentic_connector::DuckDbUrlConfig {
            url: "/nonexistent-dir/x.duckdb?motherduck_token=tok-12345".to_string(),
            read_only: false,
        };
        assert!(!format!("{url_cfg:?}").contains("tok-12345"));

        let err = match agentic_connector::build_connector(
            agentic_connector::ConnectorConfig::DuckDbUrl(url_cfg),
        ) {
            Ok(_) => panic!("a file in a missing directory should not open"),
            Err(e) => e.to_string(),
        };

        assert!(!err.contains("tok-12345"), "{err}");
    }
}
