//! [`super`]'s unit tests: the SQL a baseline's request, locks and chunk
//! read run, mostly for `leases` (`id` the key, `contact_email` excluded), as
//! [`crate::changes::shape`]'s test fixture declares it.

use std::time::Duration;

use super::*;
use crate::changes::shape::leases;

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

/// A key column's operators in `schema`, as [`KEY_TYPES_SQL`] reads them.
fn operators(schema: &str) -> KeyOrder {
    KeyOrder {
        lt: format!("{schema}.<"),
        eq: format!("{schema}.="),
        ge: format!("{schema}.>="),
        gt: format!("{schema}.>"),
    }
}

/// `n` key columns of built-in types: `pg_catalog`'s operators.
fn builtin(n: usize) -> Vec<KeyOrder> {
    vec![operators("pg_catalog"); n]
}

/// `leases`'s chunk read, for writer `w`.
fn leases_chunk(after: bool) -> String {
    let capture = leases();
    let captured: Vec<String> = capture.captured.iter().map(|c| c.name.clone()).collect();
    let key_types = strings(&["pg_catalog.int8"]);
    let key_order = builtin(1);
    let shape = SnapshotShape {
        captured: &captured,
        primary_key: &capture.shape.primary_key,
        key_types: &key_types,
        key_order: &key_order,
    };
    chunk_read_sql("w", "leases", &shape, after).expect("valid names")
}

#[test]
fn chunk_sql_filters_images_as_capture_does() {
    let sql = leases_chunk(true);
    assert_eq!(
        sql,
        r#"SELECT (SELECT jsonb_object_agg(e.key, e.value) FROM jsonb_each(pg_catalog.to_jsonb(r.*)) e WHERE e.key = ANY(ARRAY['id']::text[]))::text AS key,
       (SELECT jsonb_object_agg(e.key, e.value) FROM jsonb_each(pg_catalog.to_jsonb(r.*)) e WHERE e.key = ANY(ARRAY['id','name']::text[]))::text AS image
  FROM "app_w"."leases" r
 WHERE r."id" OPERATOR(pg_catalog.>) (SELECT c."id" FROM jsonb_to_record($2::text::jsonb) AS c("id" pg_catalog.int8))
 ORDER BY r."id" USING OPERATOR(pg_catalog.<) LIMIT $1 FOR SHARE OF r"#
    );
    assert!(
        !sql.contains("contact_email"),
        "an excluded column is named nowhere: {sql}"
    );
}

/// The first chunk reads from the start, and binds only the limit.
#[test]
fn the_first_chunk_has_no_cursor() {
    let sql = leases_chunk(false);
    assert!(!sql.contains("\n WHERE"), "{sql}");
    assert!(!sql.contains("$2"), "{sql}");
    assert!(
        sql.ends_with(
            "  FROM \"app_w\".\"leases\" r\n \
             ORDER BY r.\"id\" USING OPERATOR(pg_catalog.<) LIMIT $1 FOR SHARE OF r"
        ),
        "{sql}"
    );
}

/// A composite key whose operators are all one schema's resumes as one row
/// comparison by that schema's `>`, which the key's index seeks to, and
/// orders by each column's own `<`.
#[test]
fn a_composite_key_orders_and_compares_by_every_column() {
    let captured = strings(&["a", "b", "c"]);
    let primary_key = strings(&["a", "b"]);
    let key_types = strings(&["pg_catalog.int8", "pg_catalog.text"]);
    let key_order = builtin(2);
    let shape = SnapshotShape {
        captured: &captured,
        primary_key: &primary_key,
        key_types: &key_types,
        key_order: &key_order,
    };
    let sql = chunk_read_sql("w", "pairs", &shape, true).expect("valid names");
    assert!(
        sql.contains(
            "\n WHERE (r.\"a\", r.\"b\") OPERATOR(pg_catalog.>) (SELECT c.\"a\", c.\"b\" FROM \
             jsonb_to_record($2::text::jsonb) AS c(\"a\" pg_catalog.int8, \"b\" pg_catalog.text))\n"
        ),
        "{sql}"
    );
    assert!(
        sql.ends_with(
            "ORDER BY r.\"a\" USING OPERATOR(pg_catalog.<), r.\"b\" USING OPERATOR(pg_catalog.<) \
             LIMIT $1 FOR SHARE OF r"
        ),
        "{sql}"
    );
    assert!(
        sql.contains("WHERE e.key = ANY(ARRAY['a','b']::text[]))::text AS key"),
        "{sql}"
    );
}

/// A key whose columns' operators live in different schemas has no one `>`
/// to compare its rows by: it resumes lexicographically, each column by its
/// own operators, under a leading `>=` on the first column that keeps the
/// read a range of the key's index.
#[test]
fn a_key_mixing_schemas_resumes_lexicographically() {
    let captured = strings(&["tag", "id"]);
    let primary_key = strings(&["tag", "id"]);
    let key_types = strings(&["ext.citext", "pg_catalog.int8"]);
    let key_order = vec![operators("ext"), operators("pg_catalog")];
    let shape = SnapshotShape {
        captured: &captured,
        primary_key: &primary_key,
        key_types: &key_types,
        key_order: &key_order,
    };
    let sql = chunk_read_sql("w", "tags", &shape, true).expect("valid names");
    let c = |k: &str| {
        format!(
            "(SELECT c.\"{k}\" FROM jsonb_to_record($2::text::jsonb) \
             AS c(\"tag\" ext.citext, \"id\" pg_catalog.int8))"
        )
    };
    let (tag, id) = (c("tag"), c("id"));
    let past = format!(
        "\n WHERE r.\"tag\" OPERATOR(ext.>=) {tag} AND (r.\"tag\" OPERATOR(ext.>) {tag} \
         OR (r.\"tag\" OPERATOR(ext.=) {tag} AND r.\"id\" OPERATOR(pg_catalog.>) {id}))\n"
    );
    assert!(sql.contains(&past), "{sql}");
    assert!(
        sql.ends_with(
            "ORDER BY r.\"tag\" USING OPERATOR(ext.<), r.\"id\" USING OPERATOR(pg_catalog.<) \
             LIMIT $1 FOR SHARE OF r"
        ),
        "{sql}"
    );
}

#[test]
fn a_quote_in_a_key_column_is_escaped() {
    let captured = strings(&["o\"k", "o'brien"]);
    let primary_key = strings(&["o\"k"]);
    let key_types = strings(&["pg_catalog.text"]);
    let key_order = builtin(1);
    let shape = SnapshotShape {
        captured: &captured,
        primary_key: &primary_key,
        key_types: &key_types,
        key_order: &key_order,
    };
    let sql = chunk_read_sql("w", "leases", &shape, true).expect("valid names");
    assert!(
        sql.contains(
            "r.\"o\"\"k\" OPERATOR(pg_catalog.>) (SELECT c.\"o\"\"k\" FROM \
             jsonb_to_record($2::text::jsonb) AS c(\"o\"\"k\" pg_catalog.text))"
        ),
        "{sql}"
    );
    assert!(
        sql.contains("ORDER BY r.\"o\"\"k\" USING OPERATOR(pg_catalog.<) LIMIT"),
        "{sql}"
    );
    assert!(
        sql.contains("ARRAY['o\"k','o''brien']::text[]"),
        "a captured name stays a literal: {sql}"
    );
}

/// An extension's type orders and resumes by its own operators, in its own
/// schema, never by `pg_catalog`'s: `citext`'s order is not text's.
#[test]
fn an_extension_types_own_operators_order_and_resume_the_copy() {
    let name = strings(&["name"]);
    let key_types = strings(&["ext.citext"]);
    for schema in ["ext", "\"my ext\""] {
        let key_order = vec![operators(schema)];
        let shape = SnapshotShape {
            captured: &name,
            primary_key: &name,
            key_types: &key_types,
            key_order: &key_order,
        };
        let sql = chunk_read_sql("w", "tags", &shape, true).expect("valid names");
        assert!(
            sql.contains(&format!(
                "\n WHERE r.\"name\" OPERATOR({schema}.>) (SELECT c.\"name\" FROM \
                 jsonb_to_record($2::text::jsonb) AS c(\"name\" ext.citext))\n"
            )),
            "{sql}"
        );
        assert!(
            sql.contains(&format!(
                "ORDER BY r.\"name\" USING OPERATOR({schema}.<) LIMIT"
            )),
            "{sql}"
        );
        assert!(!sql.contains("pg_catalog.<"), "{sql}");
    }
}

/// A copy orders and resumes by the key: without one, without a type for
/// each of its columns, or without its type's btree operators, there is no
/// statement to write. An operator that is not `<schema>.<op>` never reaches
/// the SQL.
#[test]
fn a_shape_without_a_usable_key_is_refused() {
    let captured = strings(&["id", "name"]);
    let none: [String; 0] = [];
    let id = strings(&["id"]);
    let one_type = strings(&["pg_catalog.int8"]);
    let two_types = strings(&["pg_catalog.int8", "pg_catalog.text"]);
    let refused = |primary_key: &[String], key_types: &[String], key_order: &[KeyOrder]| {
        let shape = SnapshotShape {
            captured: &captured,
            primary_key,
            key_types,
            key_order,
        };
        chunk_read_sql("w", "leases", &shape, true).unwrap_err()
    };
    let invalid = |reason: &'static str| SchemaError::InvalidKey {
        table: "leases".into(),
        reason,
    };
    assert_eq!(refused(&none, &none, &[]), invalid("it has no primary key"));
    for key_types in [&none[..], &two_types[..]] {
        assert_eq!(
            refused(&id, key_types, &builtin(1)),
            invalid("each key column needs exactly one type")
        );
    }
    let no_class = invalid("each key column's type needs a default btree operator class");
    assert_eq!(refused(&id, &one_type, &[]), no_class);
    assert_eq!(refused(&id, &one_type, &[KeyOrder::default()]), no_class);
    for bad in [
        "<",
        "pg_catalog.",
        "pg_catalog.<) OR (true",
        "\"ext.<",
        "Ext.<",
        "a\"b.<",
    ] {
        let mut order = operators("pg_catalog");
        order.gt = bad.to_owned();
        assert_eq!(refused(&id, &one_type, &[order]), no_class, "{bad}");
    }
}

/// The writer's and the table's names reach SQL as identifiers, so each
/// must pass `validate_name` first.
#[test]
fn names_are_checked_before_they_reach_sql() {
    assert_eq!(
        begin_lock_sql("w", "leases").expect("valid names"),
        "LOCK TABLE \"app_w\".\"leases\" IN SHARE MODE"
    );
    assert!(begin_lock_sql("w", "leases\"; DROP TABLE x; --").is_err());
    assert!(begin_lock_sql("W", "leases").is_err());
    let id = strings(&["id"]);
    let types = strings(&["pg_catalog.int8"]);
    let key_order = builtin(1);
    let shape = SnapshotShape {
        captured: &id,
        primary_key: &id,
        key_types: &types,
        key_order: &key_order,
    };
    assert!(matches!(
        chunk_read_sql("w", "Leases", &shape, false),
        Err(SchemaError::InvalidName(_))
    ));
    assert!(request_named_baseline_sql("w", "lea'ses", BaselineReason::Resync).is_err());
    assert!(request_named_baseline_sql("w x", "leases", BaselineReason::Resync).is_err());
}

#[test]
fn requests_name_their_reason() {
    let [lock, abandon, request] = request_baseline_sql(7, BaselineReason::Reshape);
    assert_eq!(
        lock,
        "SELECT 1 FROM oxy_changes.baselines \
         WHERE relid = 7::oid AND state IN ('requested', 'copying') FOR UPDATE",
        "the open rows are locked before anything changes"
    );
    assert_eq!(
        abandon,
        "UPDATE oxy_changes.baselines SET state = 'abandoned' \
         WHERE relid = 7::oid AND state = 'copying'"
    );
    assert!(request.contains("'reshape'"), "{request}");
    assert!(
        request.contains(
            "ON CONFLICT (relid) WHERE state = 'requested' \
             DO UPDATE SET requested_at = oxy_changes.baselines.requested_at, \
             attempts = 0, retry_at = NULL"
        ),
        "a request that finds one requested locks its row and clears its backoff: {request}"
    );
    assert!(request.contains("WHERE t.relid = 7::oid"), "{request}");
    assert!(!request.contains("installed"), "{request}");

    let reasons = [
        (BaselineReason::OptIn, "opt_in"),
        (BaselineReason::Reshape, "reshape"),
        (BaselineReason::Drift, "drift"),
        (BaselineReason::Resync, "resync"),
        (BaselineReason::Bump, "bump"),
        (BaselineReason::Missing, "missing"),
    ];
    for (reason, text) in reasons {
        assert_eq!(reason.as_str(), text);
    }
}

#[test]
fn a_named_request_finds_the_table_by_writer_and_name() {
    let [lock, abandon, request] =
        request_named_baseline_sql("w", "leases", BaselineReason::Drift).expect("valid names");
    let named = "t.writer = 'w' AND t.table_name = 'leases'";
    let rows = format!("WHERE relid IN (SELECT t.relid FROM oxy_changes.tables t WHERE {named})");
    assert!(
        lock.contains(&format!(
            "{rows} AND state IN ('requested', 'copying') FOR UPDATE"
        )),
        "{lock}"
    );
    assert!(
        abandon.contains(&format!("{rows} AND state = 'copying'")),
        "{abandon}"
    );
    assert!(
        request.contains(&format!("WHERE {named} ON CONFLICT")),
        "{request}"
    );
    assert!(request.contains("'drift'"), "{request}");
}

/// A begin waits on its request row, then reads on a fresh snapshot, and a
/// copying baseline of the same table makes it "not now", not a 23505.
#[test]
fn a_begin_locks_its_request_and_never_overlaps_a_copy() {
    assert_eq!(
        LOCK_REQUEST_SQL,
        "SELECT id FROM oxy_changes.baselines WHERE id = $1 AND state = 'requested' FOR UPDATE"
    );
    assert!(
        BEGIN_SQL.contains(
            "AND NOT EXISTS (SELECT 1 FROM oxy_changes.baselines c \
             WHERE c.relid = b.relid AND c.state = 'copying')"
        ),
        "{BEGIN_SQL}"
    );
}

/// The cursor and the pacing count bind as the relay holds them: the key as
/// text, never through a float, and the held list possibly null.
#[test]
fn the_cursor_binds_as_text_and_the_backlog_is_capped() {
    assert!(
        CHUNK_WRITE_SQL.contains("SET last_key = $2::text::jsonb,"),
        "{CHUNK_WRITE_SQL}"
    );
    assert!(
        BACKLOG_SQL.contains("NOT (table_name = ANY(coalesce($3::text[], '{}')))"),
        "{BACKLOG_SQL}"
    );
    assert!(
        BACKLOG_SQL.ends_with("LIMIT $4::bigint) x"),
        "{BACKLOG_SQL}"
    );
}

#[test]
fn lock_timeout_is_milliseconds() {
    assert_eq!(
        lock_timeout_sql(Duration::from_secs(1)),
        "SET LOCAL lock_timeout = '1000ms'"
    );
    assert_eq!(
        lock_timeout_sql(Duration::ZERO),
        "SET LOCAL lock_timeout = '1ms'",
        "zero would disable the timeout"
    );
}

#[test]
fn snapshot_rows_are_the_platforms() {
    assert!(
        CHUNK_WRITE_SQL.contains(&format!(
            "'s'::\"char\", r.key::jsonb, r.image::jsonb, '{PLATFORM_ACTOR}', $5"
        )),
        "{CHUNK_WRITE_SQL}"
    );
}

/// Every step's transaction pins its search path to `pg_catalog` and sets
/// both timeouts, in milliseconds and never zero, before any other statement.
#[test]
fn a_steps_session_is_pinned_and_timed() {
    assert_eq!(
        pin_session_sql(Duration::from_secs(1), Duration::from_secs(10)),
        "SET LOCAL search_path = pg_catalog, pg_temp; \
         SET LOCAL lock_timeout = '1000ms'; \
         SET LOCAL statement_timeout = '10000ms'"
    );
    assert!(
        pin_session_sql(Duration::ZERO, Duration::ZERO)
            .ends_with("SET LOCAL statement_timeout = '1ms'"),
        "zero would disable the timeout"
    );
    assert!(matches!(
        CHUNK_READ_ISOLATION,
        IsolationLevel::RepeatableRead
    ));
}

/// A baseline backs off `5 s · 2^attempts`, at most 30 minutes, counting each
/// attempt; a begin or a chunk that commits clears it, and the open read
/// says, by the store's clock, whether it is still backing off.
#[test]
fn a_backoff_doubles_to_its_cap_and_a_step_clears_it() {
    assert!(
        BACKOFF_SQL.contains(
            "SET attempts = attempts + 1, \
             retry_at = now() + least(interval '5 seconds' * power(2, least(attempts, 20)), \
             interval '30 minutes')"
        ),
        "{BACKOFF_SQL}"
    );
    assert!(
        BACKOFF_SQL.contains("WHERE id = $1 AND state IN ('requested', 'copying')"),
        "{BACKOFF_SQL}"
    );
    assert!(
        BACKOFF_SQL.contains("RETURNING attempts, retry_at, "),
        "the relay keeps the mark due at retry_at: {BACKOFF_SQL}"
    );
    for cleared in [BEGIN_SQL, CHUNK_WRITE_SQL] {
        assert!(
            cleared.contains("attempts = 0, retry_at = NULL"),
            "{cleared}"
        );
    }
    assert!(
        OPEN_BASELINES_SQL.contains("coalesce(retry_at > now(), false) AS backing_off"),
        "{OPEN_BASELINES_SQL}"
    );
    assert!(
        OPEN_BASELINES_SQL.contains("extract(epoch FROM retry_at - now())::float8 AS retry_in"),
        "a delay, so no other clock compares with the store's: {OPEN_BASELINES_SQL}"
    );
}

/// A view that could not be made again is flagged on the table's latest
/// complete baseline, listed by writer and table, and cleared once made.
#[test]
fn a_pending_view_is_flagged_listed_and_cleared() {
    assert!(
        VIEW_PENDING_SQL.contains(
            "WHERE writer = $1 AND table_name = $2 AND state = 'complete' \
             ORDER BY id DESC LIMIT 1"
        ),
        "{VIEW_PENDING_SQL}"
    );
    assert!(
        PENDING_VIEWS_SQL.contains("WHERE view_pending ORDER BY writer, table_name"),
        "{PENDING_VIEWS_SQL}"
    );
    assert!(
        VIEW_RENDERED_SQL.contains(
            "SET view_pending = false WHERE writer = $1 AND table_name = $2 AND view_pending"
        ),
        "{VIEW_RENDERED_SQL}"
    );
}
