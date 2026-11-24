//! `host.rs`'s unit tests, in a file of their own so the host's grows only
//! with the host.

use super::*;
use serde_json::json;

// ── ctx.fetch request bodies ──────────────────────────────────────────

#[test]
fn fetch_body_absent_stays_absent_and_defaults_to_utf8() {
    assert_eq!(fetch_body_bytes(&json!({})).unwrap(), None);
    assert_eq!(
        fetch_body_bytes(&json!({ "body": "hello" })).unwrap(),
        Some(b"hello".to_vec())
    );
    // An explicit empty/whitespace bodyEncoding means "unspecified", the
    // same as every other encoding field (see `encoding_or`).
    assert_eq!(
        fetch_body_bytes(&json!({ "body": "hello", "bodyEncoding": "  " })).unwrap(),
        Some(b"hello".to_vec())
    );
}

#[test]
fn fetch_body_base64_survives_bytes_that_utf8_would_corrupt() {
    // 0xFF is not valid UTF-8. Carried as a JS string it becomes U+FFFD and
    // the far end receives a corrupt file — the exact failure this option
    // exists to prevent. Note the assertion is that the bytes round-trip
    // EXACTLY, not merely that decoding succeeds.
    let raw = vec![0x50u8, 0x4B, 0x03, 0x04, 0xFF, 0x00, 0x80];
    let b64 = encode_base64(&raw);
    assert_eq!(
        fetch_body_bytes(&json!({ "body": b64, "bodyEncoding": "base64" })).unwrap(),
        Some(raw.clone())
    );
    // And the same string WITHOUT the flag is what corruption looks like:
    // the utf8 path hands on the base64 text itself, not the bytes.
    assert_ne!(
        fetch_body_bytes(&json!({ "body": encode_base64(&raw) })).unwrap(),
        Some(raw)
    );
}

#[test]
fn fetch_body_rejects_bad_base64_and_unknown_encodings() {
    let err =
        fetch_body_bytes(&json!({ "body": "not!base64", "bodyEncoding": "base64" })).unwrap_err();
    assert!(err.contains("not valid base64"), "{err}");
    let err = fetch_body_bytes(&json!({ "body": "x", "bodyEncoding": "hex" })).unwrap_err();
    assert!(err.contains("unknown bodyEncoding"), "{err}");
}

// ── ctx.storage capability gate ─────────────────────────────────────────

fn storage_caps(read: bool, write: bool) -> FunctionCapabilities {
    FunctionCapabilities {
        storage_read: read,
        storage_write: write,
        ..Default::default()
    }
}

#[test]
fn storage_delete_and_copy_are_refused_without_write() {
    // A read-only function must not be able to destroy or overwrite what
    // is in its app's silo. Both ops used to pass on `storage.read` alone.
    let read_only = storage_caps(true, false);
    for op in [HostOp::StorageDelete, HostOp::StorageCopy] {
        let err = read_only
            .refuse(op)
            .expect_err(&format!("`{op:?}` must be refused without storage.write"));
        assert!(err.starts_with("StorageCapabilityMissing"), "{op:?}: {err}");
        assert!(err.contains("`storage.write`"), "{op:?}: {err}");
    }
}

#[test]
fn storage_copy_is_refused_without_read() {
    // `copy` reads its source as well as writing its destination, so write
    // alone is not enough. `delete` reads nothing, so write alone is.
    let write_only = storage_caps(false, true);
    let err = write_only
        .refuse(HostOp::StorageCopy)
        .expect_err("`copy` must be refused without storage.read");
    assert!(err.contains("`storage.read`"), "{err}");
    for op in [
        HostOp::StorageDelete,
        HostOp::StoragePut,
        HostOp::StorageGetUploadUrl,
    ] {
        write_only
            .refuse(op)
            .unwrap_or_else(|e| panic!("`{op:?}` needs only storage.write: {e}"));
    }
    // Holding both is what a copy needs.
    storage_caps(true, true)
        .refuse(HostOp::StorageCopy)
        .expect("read+write copies");
}

#[test]
fn storage_reads_work_with_read_only_and_writes_do_not() {
    let read_only = storage_caps(true, false);
    for op in [
        HostOp::StorageGet,
        HostOp::StorageList,
        HostOp::StorageHead,
        HostOp::StorageGetDownloadUrl,
    ] {
        read_only
            .refuse(op)
            .unwrap_or_else(|e| panic!("`{op:?}` needs only storage.read: {e}"));
    }
    for op in [HostOp::StoragePut, HostOp::StorageGetUploadUrl] {
        assert!(
            read_only.refuse(op).is_err(),
            "`{op:?}` must be refused without storage.write"
        );
    }
}

/// Nothing declared refuses every gated op, in the host's words — and
/// the registry row, not an arm per op, is what gates it: an op added
/// with a capability is refused here without any check of its own.
#[test]
fn every_gated_op_is_refused_when_nothing_is_declared() {
    let none = FunctionCapabilities::default();
    for op in HostOp::ALL {
        let gated = !matches!(
            op.spec().capability,
            Capability::None | Capability::WarehouseWrite
        );
        match none.refuse(*op) {
            Err(refusal) => {
                assert!(gated, "{op:?} is ungated yet refused: {refusal}");
                assert!(
                    refusal.contains("has not declared the"),
                    "{op:?}: {refusal}"
                );
            }
            Ok(()) => assert!(!gated, "{op:?} must be refused with nothing declared"),
        }
    }
    let everything = FunctionCapabilities::granting_everything();
    for op in HostOp::ALL {
        everything
            .refuse(*op)
            .unwrap_or_else(|e| panic!("{op:?}: {e}"));
    }
}

/// A writer the slug cannot back says so, not "capability missing".
#[test]
fn a_slug_that_backs_no_schema_is_refused_as_that() {
    let caps = FunctionCapabilities {
        oltp: WriterCapability::resolve(true, "9lives"),
        airhouse: WriterCapability::resolve(true, "9lives"),
        ..Default::default()
    };
    for op in [HostOp::OltpExec, HostOp::AirhouseAppend] {
        let err = caps.refuse(op).unwrap_err();
        assert!(err.contains("cannot back an"), "{op:?}: {err}");
        assert!(!err.contains("CapabilityMissing"), "{op:?}: {err}");
    }
}

// ── build_insert_sql / quote_ident / json_value_to_sql_literal ─────────

/// BigQuery and MySQL read a double-quoted word as a string, so the host
/// backquotes the identifiers it builds for them.
#[test]
fn a_host_built_insert_quotes_identifiers_as_its_dialect_does() {
    let payload = json!({ "table": "orders", "rows": [{ "id": 1 }] });
    for dialect in [SqlDialect::BigQuery, SqlDialect::MYSQL] {
        assert_eq!(
            build_insert_sql(&payload, false, dialect, StringLiteral::Standard).unwrap(),
            "INSERT INTO `orders` (`id`) VALUES (1)"
        );
    }
    assert_eq!(
        build_insert_sql(
            &payload,
            false,
            SqlDialect::CLICKHOUSE,
            StringLiteral::Standard
        )
        .unwrap(),
        r#"INSERT INTO "orders" ("id") VALUES (1)"#
    );
    assert_eq!(backquote_ident("we`ird"), "`we``ird`");
}

#[test]
fn quote_ident_escapes_embedded_quotes() {
    assert_eq!(quote_ident("orders"), "\"orders\"");
    assert_eq!(quote_ident(r#"weird"col"#), "\"weird\"\"col\"");
}

#[test]
fn oltp_capability_distinguishes_its_two_fail_closed_reasons() {
    // Gate off → Disabled, whatever the slug.
    assert!(matches!(
        WriterCapability::resolve(false, "bookings"),
        WriterCapability::Disabled
    ));
    // Gate on + derivable slug → Enabled, writer derived from the slug
    // (hyphens → underscores), NOT from the manifest.
    match WriterCapability::resolve(true, "oltp-bookings") {
        WriterCapability::Enabled { writer } => assert_eq!(writer, "oltp_bookings"),
        other => panic!("expected Enabled, got {other:?}"),
    }
    // Gate on but the slug can't back a schema → SlugNotDerivable (NOT
    // Disabled), so the author sees "your slug can't back a schema" rather
    // than "you didn't declare the capability" — the exact misdiagnosis this
    // enum exists to prevent. A leading digit, an over-long slug, and an
    // underscore (which `app_writer_name` refuses to keep the derivation
    // injective) all resolve here.
    for bad in [
        "2fa-app",
        "1password-sync",
        "my_app", // an `_` slug — refused so it can't alias `my-app`
        &"a".repeat(oxy_oltp::schema::MAX_NAME_LEN + 1),
    ] {
        match WriterCapability::resolve(true, bad) {
            WriterCapability::SlugNotDerivable { slug } => assert_eq!(slug, bad),
            other => panic!("expected SlugNotDerivable for {bad:?}, got {other:?}"),
        }
    }
}

#[test]
fn json_value_to_sql_literal_covers_all_variants() {
    assert_eq!(
        json_value_to_sql_literal(&json!(null), StringLiteral::Standard),
        "NULL"
    );
    assert_eq!(
        json_value_to_sql_literal(&json!(true), StringLiteral::Standard),
        "true"
    );
    assert_eq!(
        json_value_to_sql_literal(&json!(42), StringLiteral::Standard),
        "42"
    );
    assert_eq!(
        json_value_to_sql_literal(&json!(4.5), StringLiteral::Standard),
        "4.5"
    );
    assert_eq!(
        json_value_to_sql_literal(&json!("plain"), StringLiteral::Standard),
        "'plain'"
    );
    assert_eq!(
        json_value_to_sql_literal(&json!("it's"), StringLiteral::Standard),
        "'it''s'"
    );
    // Arrays/objects round-trip through JSON with single quotes escaped.
    assert_eq!(
        json_value_to_sql_literal(&json!({"a": "b's"}), StringLiteral::Standard),
        "'{\"a\":\"b''s\"}'"
    );
}

/// The four values that separate the rules: a backslash, a quote, the two
/// together, and a backslash at the very end of the value.
const ESCAPES: &[&str] = &["a\\b", "it's", "x\\' OR 1=1 -- ", "C:\\"];

/// A string is written the way the destination's engine reads a literal.
/// With quote-only escaping, the third value ended its literal on every
/// engine that reads a backslash and the fourth swallowed the closing
/// quote, so the rest of the row was read as part of the value.
#[test]
fn a_string_is_escaped_as_its_engine_reads_a_literal() {
    let written = |rule: StringLiteral| -> Vec<String> {
        ESCAPES
            .iter()
            .map(|v| json_value_to_sql_literal(&json!(v), rule))
            .collect()
    };
    assert_eq!(
        written(StringLiteral::Standard),
        ["'a\\b'", "'it''s'", "'x\\'' OR 1=1 -- '", "'C:\\'"],
        "on DuckDB and Postgres a backslash is an ordinary character"
    );
    assert_eq!(
        written(StringLiteral::Backslash),
        ["'a\\\\b'", "'it''s'", "'x\\\\'' OR 1=1 -- '", "'C:\\\\'"],
        "on ClickHouse, MySQL, Snowflake and Redshift the backslash is doubled"
    );
    assert_eq!(
        written(StringLiteral::BackslashOnly),
        ["'a\\\\b'", "'it\\'s'", "'x\\\\\\' OR 1=1 -- '", "'C:\\\\'"],
        "on BigQuery the backslash is doubled and the quote is written with one"
    );
}

/// JSON escapes with backslashes of its own, so an object written for an
/// engine that reads them has to double them or the engine stores
/// different JSON from what was serialized.
#[test]
fn a_json_object_keeps_its_own_escapes_on_a_backslash_engine() {
    let value = json!({ "note": "say \"hi\"", "path": "C:\\" });
    assert_eq!(
        json_value_to_sql_literal(&value, StringLiteral::Standard),
        r#"'{"note":"say \"hi\"","path":"C:\\"}'"#
    );
    assert_eq!(
        json_value_to_sql_literal(&value, StringLiteral::Backslash),
        r#"'{"note":"say \\"hi\\"","path":"C:\\\\"}'"#
    );
}

/// The whole statement, for the engine it is sent to: every value stays in
/// its own literal, and nothing follows the row list (ClickHouse reads
/// anything after `VALUES` as row data).
#[test]
fn an_insert_keeps_each_value_in_its_own_column() {
    let payload = json!({
        "table": "notes",
        "rows": [{ "a": "x\\' OR 1=1 -- ", "b": "C:\\" }],
    });
    assert_eq!(
        build_insert_sql(
            &payload,
            false,
            SqlDialect::Postgres,
            StringLiteral::Standard
        )
        .unwrap(),
        "INSERT INTO \"notes\" (\"a\", \"b\") VALUES ('x\\'' OR 1=1 -- ', 'C:\\')"
    );
    assert_eq!(
        build_insert_sql(
            &payload,
            false,
            SqlDialect::Postgres,
            StringLiteral::Backslash
        )
        .unwrap(),
        "INSERT INTO \"notes\" (\"a\", \"b\") VALUES ('x\\\\'' OR 1=1 -- ', 'C:\\\\')"
    );
    assert_eq!(
        build_insert_sql(
            &payload,
            false,
            SqlDialect::BigQuery,
            StringLiteral::BackslashOnly
        )
        .unwrap(),
        "INSERT INTO `notes` (`a`, `b`) VALUES ('x\\\\\\' OR 1=1 -- ', 'C:\\\\')"
    );
}

/// Production keeps its identifier quoting, but the values follow the
/// engine there too: Redshift answers as Postgres and reads backslashes.
#[test]
fn values_follow_the_engine_in_production_and_identifiers_do_not() {
    let postgres = PostgresConnector::new("h", 5432, "u", "p", "d");
    let redshift = PostgresConnector::new("h", 5432, "u", "p", "d").redshift();
    for is_production in [true, false] {
        assert_eq!(
            engine_spelling(is_production, &postgres),
            (SqlDialect::Postgres, StringLiteral::Standard)
        );
        assert_eq!(
            engine_spelling(is_production, &redshift),
            (SqlDialect::Postgres, StringLiteral::Backslash)
        );
    }
}

/// A row with no quote and no backslash is the same bytes on every engine,
/// so nothing a working app sends today changes.
#[test]
fn a_plain_row_is_written_the_same_for_every_engine() {
    let payload = json!({ "table": "orders", "rows": [{ "id": 1, "name": "ada" }] });
    for rule in [
        StringLiteral::Standard,
        StringLiteral::Backslash,
        StringLiteral::BackslashOnly,
    ] {
        assert_eq!(
            build_insert_sql(&payload, false, SqlDialect::Postgres, rule).unwrap(),
            r#"INSERT INTO "orders" ("id", "name") VALUES (1, 'ada')"#,
            "{rule:?}"
        );
    }
}

#[test]
fn build_insert_sql_basic_insert() {
    let payload = json!({
        "table": "daily_rollup",
        "rows": [
            { "day": "2026-06-13", "store_id": 12, "total": 4821.5 },
        ],
    });
    let sql = build_insert_sql(
        &payload,
        false,
        SqlDialect::Postgres,
        StringLiteral::Standard,
    )
    .unwrap();
    assert_eq!(
        sql,
        r#"INSERT INTO "daily_rollup" ("day", "store_id", "total") VALUES ('2026-06-13', 12, 4821.5)"#
    );
}

#[test]
fn build_insert_sql_multi_row() {
    let payload = json!({
        "table": "t",
        "rows": [
            { "a": 1, "b": "x" },
            { "a": 2, "b": "y" },
        ],
    });
    let sql = build_insert_sql(
        &payload,
        false,
        SqlDialect::Postgres,
        StringLiteral::Standard,
    )
    .unwrap();
    assert_eq!(
        sql,
        r#"INSERT INTO "t" ("a", "b") VALUES (1, 'x'), (2, 'y')"#
    );
}

#[test]
fn build_insert_sql_requires_table() {
    let payload = json!({ "rows": [{ "a": 1 }] });
    let err = build_insert_sql(
        &payload,
        false,
        SqlDialect::Postgres,
        StringLiteral::Standard,
    )
    .unwrap_err();
    assert!(err.contains("`table`"));
}

#[test]
fn build_insert_sql_requires_non_empty_rows() {
    let payload = json!({ "table": "t", "rows": [] });
    let err = build_insert_sql(
        &payload,
        false,
        SqlDialect::Postgres,
        StringLiteral::Standard,
    )
    .unwrap_err();
    assert!(err.contains("`rows`"));
}

#[test]
fn build_insert_sql_requires_consistent_columns() {
    let payload = json!({
        "table": "t",
        "rows": [
            { "a": 1, "b": 2 },
            { "a": 1 },
        ],
    });
    let err = build_insert_sql(
        &payload,
        false,
        SqlDialect::Postgres,
        StringLiteral::Standard,
    )
    .unwrap_err();
    assert!(err.contains("missing column 'b'"));
}

#[test]
fn build_insert_sql_upsert_appends_on_conflict() {
    let payload = json!({
        "table": "daily_rollup",
        "rows": [{ "day": "2026-06-13", "store_id": 12, "total": 4821.5 }],
        "conflictColumns": ["day", "store_id"],
    });
    let sql = build_insert_sql(
        &payload,
        true,
        SqlDialect::Postgres,
        StringLiteral::Standard,
    )
    .unwrap();
    assert_eq!(
        sql,
        r#"INSERT INTO "daily_rollup" ("day", "store_id", "total") VALUES ('2026-06-13', 12, 4821.5) ON CONFLICT ("day", "store_id") DO UPDATE SET "total" = EXCLUDED."total""#
    );
}

#[test]
fn build_insert_sql_upsert_all_columns_in_conflict_does_nothing() {
    let payload = json!({
        "table": "t",
        "rows": [{ "a": 1, "b": 2 }],
        "conflictColumns": ["a", "b"],
    });
    let sql = build_insert_sql(
        &payload,
        true,
        SqlDialect::Postgres,
        StringLiteral::Standard,
    )
    .unwrap();
    assert!(sql.ends_with("DO NOTHING"));
}

#[test]
fn build_insert_sql_upsert_requires_conflict_columns() {
    let payload = json!({
        "table": "t",
        "rows": [{ "a": 1 }],
    });
    let err = build_insert_sql(
        &payload,
        true,
        SqlDialect::Postgres,
        StringLiteral::Standard,
    )
    .unwrap_err();
    assert!(err.contains("conflictColumns"));
}

/// Write a one-column Parquet with `n` rows and return its path (plus the
/// tempdir guard the caller must keep alive).
fn rollup_fixture(n: usize) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tmpdir");
    let path = dir.path().join("rollup.parquet");
    let conn = duckdb::Connection::open_in_memory().unwrap();
    conn.execute_batch(&format!(
        "COPY (SELECT i AS n FROM range({n}) t(i)) TO '{}' (FORMAT PARQUET);",
        path.display()
    ))
    .unwrap();
    (dir, path)
}

#[tokio::test]
async fn read_rollup_shapes_the_result_like_the_warehouse_branch() {
    let (_guard, path) = rollup_fixture(3);
    let sql = format!(
        "SELECT n FROM read_parquet('{}') ORDER BY n",
        path.display()
    );
    let result = read_rollup(
        &sql,
        &agentic_semantic::compile::PreaggSource::Local(path),
        10,
    )
    .await
    .expect("rollup read");
    let rows = result["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 3);
    // Objects keyed by column name — the same shape `query_with_truncation`
    // hands back, so a function cannot tell which tier answered.
    assert_eq!(rows[0]["n"], serde_json::json!(0));
    assert_eq!(result["truncated"], serde_json::json!(false));
}

#[tokio::test]
async fn read_rollup_caps_rows_and_flags_truncation() {
    let (_guard, path) = rollup_fixture(5);
    let sql = format!(
        "SELECT n FROM read_parquet('{}') ORDER BY n",
        path.display()
    );
    let result = read_rollup(
        &sql,
        &agentic_semantic::compile::PreaggSource::Local(path),
        2,
    )
    .await
    .expect("rollup read");
    assert_eq!(result["rows"].as_array().unwrap().len(), 2);
    assert_eq!(result["truncated"], serde_json::json!(true));
}

#[tokio::test]
async fn read_rollup_reports_a_missing_parquet_rather_than_zero_rows() {
    // The caller answers from the warehouse on this error. Returning an
    // empty result instead would be indistinguishable from a rollup that
    // genuinely has no rows.
    let missing = std::path::PathBuf::from("/nonexistent/rollup.parquet");
    let err = read_rollup(
        "SELECT 1",
        &agentic_semantic::compile::PreaggSource::Local(missing),
        10,
    )
    .await
    .expect_err("a missing Parquet must be an error");
    assert!(!err.is_empty());
}
