//! Capture against a real Postgres (OLTP change stream design §5.2): what a
//! declared table's four triggers write to `oxy_changes.log`, what they
//! refuse, and what the DDL guard holds.
//!
//! The fixture applies every platform step as the local provider's owner, who
//! may not create event triggers, so most stores here have no guard: capture
//! must work without one. The guard's tests create it as the cluster
//! superuser ([`Store::guarded`]), standing in for Neon's owner, who may
//! through `neon_superuser`.
//!
//! Every install runs each step on the connection production runs it on: the
//! owner writes the functions and the bookkeeping, the writer, which owns the
//! table, attaches the triggers, and the owner commits it.

use serde_json::{Value, json};
use uuid::Uuid;

use oxy_oltp::changes::{
    CaptureShape, CaptureSpec, Column, DeclaredTable, INSTALLED_SQL, SHAPE_SQL, ShapeRow,
    image_role_for, next_schema_version, owner_commit_sql, owner_drop_sql, owner_install_sql,
    owner_uninstall_sql, shape_from_rows, validate, writer_install_sql, writer_uninstall_sql,
};
use oxy_oltp::platform::{
    CHANGES_ACTOR_SETTING, CHANGES_DROP_TRIGGER, CHANGES_GUARD_TRIGGER, CHANGES_INVOCATION_SETTING,
    CHANGES_LOG_TABLE, CHANGES_PROVENANCE_SETTING, CHANGES_SHAPES_TABLE, CHANGES_STREAM_TABLE,
    CHANGES_TABLES_TABLE,
};
use oxy_oltp::schema::WriterRef;
use oxy_oltp::sql::{PgSqlExecutor, TenantSqlExecutor};

use crate::changes_step::{admin_dsn_for, step_eight};
use crate::{Fixture, attempt, render_pg_error};

/// The table the tests declare, created by the writer as its migration would.
const LEASES: &str = "CREATE TABLE leases (id bigint PRIMARY KEY, name text, contact_email text)";

/// A tenant store at platform step 8 with one app writer, as a publish finds
/// it after the app's migrations.
pub(crate) struct Store {
    pub(crate) fx: Fixture,
    pub(crate) app: WriterRef,
    /// The writer's bare name, which `tables.writer` and each log row record.
    pub(crate) writer: String,
    pub(crate) writer_dsn: String,
}

impl Store {
    /// `None` when the suite's cluster is absent, as for [`Fixture::create`].
    pub(crate) async fn create(name: &str) -> Option<Self> {
        let fx = Fixture::create(name).await?;
        let app = fx.app("w");
        let WriterRef::App(writer) = &app else {
            unreachable!("Fixture::app names an app writer");
        };
        let writer = writer.clone();
        let writer_dsn = fx.add_writer(&app).await;
        Some(Self {
            fx,
            app,
            writer,
            writer_dsn,
        })
    }

    /// [`Self::create`], then step 8 again as the cluster superuser, which
    /// creates the DDL guard the owner could not.
    pub(crate) async fn guarded(name: &str) -> Option<Self> {
        let store = Self::create(name).await?;
        let applied = PgSqlExecutor
            .execute_batch(&admin_dsn_for(&store.fx.database), &step_eight(&store.fx))
            .await
            .map_err(|e| e.to_string());
        let triggers = store
            .owner_count(&format!(
                "SELECT count(*) FROM pg_event_trigger \
                 WHERE evtname IN ('{CHANGES_GUARD_TRIGGER}', '{CHANGES_DROP_TRIGGER}')"
            ))
            .await;
        if applied.is_err() || triggers != Ok(2) {
            store.cleanup().await;
            panic!("the superuser creates the guard: {applied:?}, event triggers {triggers:?}");
        }
        Some(store)
    }

    /// Drop the database and the writer's role: every test does, before it
    /// asserts.
    pub(crate) async fn cleanup(&self) {
        self.fx.cleanup(&[&self.app]).await;
    }

    pub(crate) async fn as_writer(&self, sql: &str) -> Result<(), String> {
        attempt(&self.writer_dsn, sql).await
    }

    pub(crate) async fn as_owner(&self, sql: &str) -> Result<(), String> {
        attempt(&self.fx.owner_dsn, sql).await
    }

    /// One `bigint` from `sql`, as the owner.
    pub(crate) async fn owner_count(&self, sql: &str) -> Result<i64, String> {
        scalar(&self.fx.owner_dsn, sql).await
    }

    /// One `bigint` from `sql`, as the writer.
    pub(crate) async fn writer_count(&self, sql: &str) -> Result<i64, String> {
        scalar(&self.writer_dsn, sql).await
    }

    /// `table`'s capture as the catalog has it now, `exclude` left out, read
    /// and validated as a publish reads it.
    pub(crate) async fn shape(
        &self,
        table: &str,
        exclude: &[&str],
    ) -> Result<CaptureShape, String> {
        self.shape_of(&self.app, table, exclude).await
    }

    /// [`Self::shape`], for a table of `app`'s.
    pub(crate) async fn shape_of(
        &self,
        app: &WriterRef,
        table: &str,
        exclude: &[&str],
    ) -> Result<CaptureShape, String> {
        let client = connect(&self.fx.owner_dsn).await?;
        let schema = app.schema_name();
        let rows = client
            .query(SHAPE_SQL, &[&schema, &table])
            .await
            .map_err(render_pg_error)?;
        let rows = rows
            .iter()
            .map(ShapeRow::from_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(render_pg_error)?;
        let decl = DeclaredTable {
            name: table.to_string(),
            exclude: exclude.iter().map(|c| c.to_string()).collect(),
        };
        validate(&decl, shape_from_rows(&rows).as_ref()).map_err(|e| e.to_string())
    }

    /// The writer's image role, which builds its captured images.
    pub(crate) fn image_role(&self) -> String {
        image_role_for("local", &self.fx.database, &self.writer)
    }

    /// Install `capture` at `schema_version`: owner, writer, owner.
    pub(crate) async fn install(
        &self,
        capture: &CaptureShape,
        schema_version: i32,
    ) -> Result<(), String> {
        self.install_for(&self.app, &self.writer_dsn, capture, schema_version)
            .await
    }

    /// [`Self::install`], for a table of `app`'s, whose writer connects with
    /// `writer_dsn`.
    pub(crate) async fn install_for(
        &self,
        app: &WriterRef,
        writer_dsn: &str,
        capture: &CaptureShape,
        schema_version: i32,
    ) -> Result<(), String> {
        let WriterRef::App(name) = app else {
            return Err(format!("{app} is not an app writer"));
        };
        let spec = CaptureSpec {
            writer: name,
            capture,
            schema_version,
        };
        let image_role = image_role_for("local", &self.fx.database, name);
        let owner = owner_install_sql(&spec, &self.fx.role_name(app), &image_role)
            .map_err(|e| e.to_string())?;
        let writer = writer_install_sql(&spec).map_err(|e| e.to_string())?;
        // One transaction, as the promote runs it: its head is a `SET LOCAL`.
        run_in_transaction(&self.fx.owner_dsn, &owner).await?;
        run(writer_dsn, &writer).await?;
        run(
            &self.fx.owner_dsn,
            &[owner_commit_sql(capture.shape.relid, schema_version)],
        )
        .await
    }

    /// Remove `capture`, the install backwards: owner, writer, owner.
    pub(crate) async fn uninstall(&self, capture: &CaptureShape) -> Result<(), String> {
        let shape = &capture.shape;
        run(&self.fx.owner_dsn, &[owner_uninstall_sql(shape.relid)]).await?;
        run(
            &self.writer_dsn,
            &writer_uninstall_sql(&shape.schema, &shape.table),
        )
        .await?;
        run(
            &self.fx.owner_dsn,
            &owner_drop_sql(shape.relid, &shape.schema),
        )
        .await
    }

    /// Create `leases` as the writer and install its capture at version 1.
    pub(crate) async fn leases(&self, exclude: &[&str]) -> Result<CaptureShape, String> {
        self.as_writer(LEASES).await?;
        let capture = self.shape("leases", exclude).await?;
        self.install(&capture, 1).await?;
        Ok(capture)
    }

    /// The version an install of `capture` records, from the latest shape
    /// [`INSTALLED_SQL`] reads for the writer, as a publish diffs it.
    pub(crate) async fn next_version(&self, capture: &CaptureShape) -> Result<i32, String> {
        let client = connect(&self.fx.owner_dsn).await?;
        let rows = client
            .query(INSTALLED_SQL, &[&self.writer])
            .await
            .map_err(render_pg_error)?;
        let Some(row) = rows
            .iter()
            .find(|r| r.get::<_, u32>("relid") == capture.shape.relid)
        else {
            return Ok(next_schema_version(None, capture));
        };
        let version: Option<i32> = row.try_get("shape_version").map_err(render_pg_error)?;
        let columns: Option<Value> = row.try_get("columns").map_err(render_pg_error)?;
        let key: Option<Vec<String>> = row.try_get("primary_key").map_err(render_pg_error)?;
        let (Some(version), Some(columns), Some(key)) = (version, columns, key) else {
            return Ok(next_schema_version(None, capture));
        };
        let columns: Vec<Column> = serde_json::from_value(columns).map_err(|e| e.to_string())?;
        Ok(next_schema_version(
            Some((version, columns.as_slice(), key.as_slice())),
            capture,
        ))
    }

    /// Every log row, in `seq` order.
    pub(crate) async fn log(&self) -> Result<Vec<LogRow>, String> {
        let client = connect(&self.fx.owner_dsn).await?;
        let sql = format!(
            "SELECT generation, writer, table_name, schema_version, op::text AS op, key, \
             row_image, actor, invocation, provenance FROM {CHANGES_LOG_TABLE} ORDER BY seq"
        );
        let rows = client
            .query(sql.as_str(), &[])
            .await
            .map_err(render_pg_error)?;
        rows.iter()
            .map(LogRow::from_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(render_pg_error)
    }

    /// Every `stream` row, as `(generation, capturing)`.
    pub(crate) async fn stream(&self) -> Result<Vec<(i64, bool)>, String> {
        let client = connect(&self.fx.owner_dsn).await?;
        let sql = format!("SELECT generation, capturing FROM {CHANGES_STREAM_TABLE}");
        let rows = client
            .query(sql.as_str(), &[])
            .await
            .map_err(render_pg_error)?;
        Ok(rows
            .iter()
            .map(|r| (r.get::<_, i64>(0), r.get::<_, bool>(1)))
            .collect())
    }
}

/// One `oxy_changes.log` row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LogRow {
    pub(crate) generation: i64,
    pub(crate) writer: String,
    pub(crate) table_name: String,
    pub(crate) schema_version: i32,
    pub(crate) op: String,
    pub(crate) key: Option<Value>,
    pub(crate) row_image: Option<Value>,
    pub(crate) actor: Option<String>,
    pub(crate) invocation: Option<Uuid>,
    pub(crate) provenance: Option<Vec<u8>>,
}

impl LogRow {
    fn from_row(row: &tokio_postgres::Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Self {
            generation: row.try_get("generation")?,
            writer: row.try_get("writer")?,
            table_name: row.try_get("table_name")?,
            schema_version: row.try_get("schema_version")?,
            op: row.try_get("op")?,
            key: row.try_get("key")?,
            row_image: row.try_get("row_image")?,
            actor: row.try_get("actor")?,
            invocation: row.try_get("invocation")?,
            provenance: row.try_get("provenance")?,
        })
    }
}

async fn connect(dsn: &str) -> Result<tokio_postgres::Client, String> {
    oxy_oltp::connect::connect(dsn, "changes capture test")
        .await
        .map_err(render_pg_error)
}

async fn scalar(dsn: &str, sql: &str) -> Result<i64, String> {
    let client = connect(dsn).await?;
    let row = client.query_one(sql, &[]).await.map_err(render_pg_error)?;
    Ok(row.get::<_, i64>(0))
}

async fn run(dsn: &str, statements: &[String]) -> Result<(), String> {
    PgSqlExecutor
        .execute_batch(dsn, statements)
        .await
        .map_err(|e| e.to_string())
}

/// `statements` in one transaction, each failure naming its statement.
pub(crate) async fn run_in_transaction(dsn: &str, statements: &[String]) -> Result<(), String> {
    let mut client = connect(dsn).await?;
    let txn = client.transaction().await.map_err(render_pg_error)?;
    for statement in statements {
        txn.batch_execute(statement)
            .await
            .map_err(|e| format!("{statement}: {}", render_pg_error(e)))?;
    }
    txn.commit().await.map_err(render_pg_error)
}

fn ops(log: &[LogRow]) -> Vec<&str> {
    log.iter().map(|r| r.op.as_str()).collect()
}

fn images(log: &[LogRow]) -> Vec<Option<Value>> {
    log.iter().map(|r| r.row_image.clone()).collect()
}

/// A `leases` row as a full image.
fn lease(id: i64, name: &str, email: &str) -> Option<Value> {
    Some(json!({"id": id, "name": name, "contact_email": email}))
}

// ── What capture writes ──────────────────────────────────────────────────────

async fn iud(store: &Store) -> Result<(Vec<(i64, bool)>, Vec<LogRow>), String> {
    store.leases(&[]).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x')")
        .await?;
    store
        .as_writer("UPDATE leases SET name = 'b' WHERE id = 1")
        .await?;
    store.as_writer("DELETE FROM leases WHERE id = 1").await?;
    Ok((store.stream().await?, store.log().await?))
}

#[tokio::test]
async fn insert_update_delete_each_write_one_row_with_images() {
    let Some(store) = Store::create("cap_iud").await else {
        return;
    };
    let out = iud(&store).await;
    store.cleanup().await;

    let (stream, log) = out.expect("install, write and read the log");
    assert_eq!(stream, [(1, true)], "the first install seeds the stream");
    assert_eq!(ops(&log), ["i", "u", "d"], "{log:#?}");
    assert_eq!(
        images(&log),
        [
            lease(1, "a", "a@x"),
            lease(1, "b", "a@x"),
            lease(1, "b", "a@x")
        ],
        "after-images for i and u, the before-image for d"
    );
    for row in &log {
        assert_eq!(row.key, Some(json!({"id": 1})), "{row:?}");
        assert_eq!(row.generation, 1, "{row:?}");
        assert_eq!(row.writer, store.writer, "the bare writer name: {row:?}");
        assert_eq!(row.table_name, "leases", "{row:?}");
        assert_eq!(row.schema_version, 1, "{row:?}");
        assert_eq!(
            (
                row.actor.as_deref(),
                row.invocation,
                row.provenance.as_deref()
            ),
            (None, None, None),
            "nothing signed these writes: {row:?}"
        );
    }
}

async fn noop_upsert(store: &Store) -> Result<(usize, usize), String> {
    store.leases(&[]).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x')")
        .await?;
    let before = store.log().await?.len();
    store
        .as_writer(
            "INSERT INTO leases VALUES (1, 'a', 'a@x') \
             ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
        )
        .await?;
    store
        .as_writer("UPDATE leases SET name = name WHERE id = 1")
        .await?;
    Ok((before, store.log().await?.len()))
}

/// Postgres fires both statement triggers for an `ON CONFLICT DO UPDATE`
/// whatever it did; an update whose images are equal records nothing.
#[tokio::test]
async fn a_noop_upsert_writes_no_change() {
    let Some(store) = Store::create("cap_noop").await else {
        return;
    };
    let out = noop_upsert(&store).await;
    store.cleanup().await;

    let (before, after) = out.expect("install and write");
    assert_eq!(before, 1, "the insert");
    assert_eq!(
        after, 1,
        "neither the no-op upsert nor the no-op update wrote a row"
    );
}

async fn changing_upsert(store: &Store) -> Result<Vec<LogRow>, String> {
    store.leases(&[]).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x')")
        .await?;
    store
        .as_writer(
            "INSERT INTO leases VALUES (1, 'b', 'a@x') \
             ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
        )
        .await?;
    store.log().await
}

#[tokio::test]
async fn a_changing_upsert_writes_one_update() {
    let Some(store) = Store::create("cap_upsert").await else {
        return;
    };
    let out = changing_upsert(&store).await;
    store.cleanup().await;

    let log = out.expect("install and write");
    assert_eq!(ops(&log), ["i", "u"], "one update, no insert: {log:#?}");
    assert_eq!(log[1].key, Some(json!({"id": 1})));
    assert_eq!(log[1].row_image, lease(1, "b", "a@x"));
}

async fn rekey(store: &Store) -> Result<Vec<LogRow>, String> {
    store.leases(&[]).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x')")
        .await?;
    store
        .as_writer("UPDATE leases SET id = 2 WHERE id = 1")
        .await?;
    store.log().await
}

#[tokio::test]
async fn a_key_change_is_a_delete_and_an_insert() {
    let Some(store) = Store::create("cap_rekey").await else {
        return;
    };
    let out = rekey(&store).await;
    store.cleanup().await;

    let log = out.expect("install and write");
    assert_eq!(
        log.len(),
        3,
        "the insert, then two rows for the update: {log:#?}"
    );
    // One statement's rows share no order of their own.
    let mut update: Vec<(String, Option<Value>, Option<Value>)> = log[1..]
        .iter()
        .map(|r| (r.op.clone(), r.key.clone(), r.row_image.clone()))
        .collect();
    update.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        update,
        [
            (
                "d".to_string(),
                Some(json!({"id": 1})),
                lease(1, "a", "a@x")
            ),
            (
                "i".to_string(),
                Some(json!({"id": 2})),
                lease(2, "a", "a@x")
            ),
        ]
    );
}

async fn truncate(store: &Store) -> Result<Vec<LogRow>, String> {
    store.leases(&[]).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x'), (2, 'b', 'b@x')")
        .await?;
    store.as_writer("TRUNCATE leases").await?;
    store.log().await
}

#[tokio::test]
async fn truncate_writes_a_marker() {
    let Some(store) = Store::create("cap_trunc").await else {
        return;
    };
    let out = truncate(&store).await;
    store.cleanup().await;

    let log = out.expect("install and write");
    assert_eq!(ops(&log), ["i", "i", "t"], "{log:#?}");
    let marker = &log[2];
    assert_eq!((&marker.key, &marker.row_image), (&None, &None));
    assert_eq!(marker.table_name, "leases");
    assert_eq!(marker.writer, store.writer);
    assert_eq!(marker.schema_version, 1);
}

async fn excluded(store: &Store) -> Result<(Vec<LogRow>, i64), String> {
    let capture = store.leases(&["contact_email"]).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x')")
        .await?;
    store
        .as_writer("UPDATE leases SET contact_email = 'b@x'")
        .await?;
    store.as_writer("UPDATE leases SET name = 'b'").await?;
    store.as_writer("DELETE FROM leases").await?;
    let shape = store
        .owner_count(&format!(
            r#"SELECT count(*) FROM {CHANGES_SHAPES_TABLE}
                WHERE relid = {}::oid AND excluded = ARRAY['contact_email']
                  AND NOT columns @> '[{{"name": "contact_email"}}]'"#,
            capture.shape.relid
        ))
        .await?;
    Ok((store.log().await?, shape))
}

#[tokio::test]
async fn excluded_columns_never_reach_the_log() {
    let Some(store) = Store::create("cap_excl").await else {
        return;
    };
    let out = excluded(&store).await;
    store.cleanup().await;

    let (log, shape) = out.expect("install and write");
    assert_eq!(
        ops(&log),
        ["i", "u", "d"],
        "an update of only the excluded column changes no captured image: {log:#?}"
    );
    assert_eq!(
        images(&log),
        [
            Some(json!({"id": 1, "name": "a"})),
            Some(json!({"id": 1, "name": "b"})),
            Some(json!({"id": 1, "name": "b"})),
        ]
    );
    assert_eq!(
        shape, 1,
        "the shape records the exclusion and omits the column"
    );
}

async fn added_column(store: &Store) -> Result<(Vec<LogRow>, i32, i64), String> {
    store.leases(&[]).await?;
    // One migration file: the column, then a write that fills it.
    store
        .as_writer(
            "ALTER TABLE leases ADD COLUMN note text; \
             INSERT INTO leases VALUES (1, 'a', 'a@x', 'first')",
        )
        .await?;
    // The install after that migration.
    let reshaped = store.shape("leases", &[]).await?;
    let version = store.next_version(&reshaped).await?;
    store.install(&reshaped, version).await?;
    store
        .as_writer("INSERT INTO leases VALUES (2, 'b', 'b@x', 'second')")
        .await?;
    let shapes = store
        .owner_count(&format!(
            "SELECT count(*) FROM {CHANGES_SHAPES_TABLE} WHERE relid = {}::oid",
            reshaped.shape.relid
        ))
        .await?;
    Ok((store.log().await?, version, shapes))
}

#[tokio::test]
async fn an_added_column_is_captured_only_after_reinstall() {
    let Some(store) = Store::create("cap_added").await else {
        return;
    };
    let out = added_column(&store).await;
    store.cleanup().await;

    let (log, version, shapes) = out.expect("migrate, reinstall and write");
    assert_eq!(version, 2, "a new captured column is a new shape");
    assert_eq!(shapes, 2, "both shapes are kept");
    assert_eq!(ops(&log), ["i", "i"], "{log:#?}");
    assert_eq!(
        (log[0].schema_version, &log[0].row_image),
        (1, &lease(1, "a", "a@x")),
        "written before the install that records it, the column is not captured"
    );
    assert_eq!(
        (log[1].schema_version, &log[1].row_image),
        (
            2,
            &Some(json!({"id": 2, "name": "b", "contact_email": "b@x", "note": "second"}))
        ),
    );
}

async fn dropped_column(store: &Store) -> Result<Vec<LogRow>, String> {
    store.leases(&[]).await?;
    // One migration file: the drop, then writes to the table.
    store
        .as_writer(
            "ALTER TABLE leases DROP COLUMN contact_email; \
             INSERT INTO leases VALUES (1, 'a'); \
             UPDATE leases SET name = 'b'; \
             DELETE FROM leases",
        )
        .await?;
    store.log().await
}

#[tokio::test]
async fn a_dropped_column_does_not_fail_the_write() {
    let Some(store) = Store::create("cap_dropped").await else {
        return;
    };
    let out = dropped_column(&store).await;
    store.cleanup().await;

    let log = out.expect("the writes after the drop succeed");
    assert_eq!(ops(&log), ["i", "u", "d"], "{log:#?}");
    assert_eq!(
        images(&log),
        [
            Some(json!({"id": 1, "name": "a"})),
            Some(json!({"id": 1, "name": "b"})),
            Some(json!({"id": 1, "name": "b"})),
        ],
        "the dropped column simply stops appearing"
    );
    assert!(log.iter().all(|r| r.schema_version == 1), "{log:#?}");
}

async fn reshaped_mid_session(store: &Store) -> Result<Vec<LogRow>, String> {
    store.leases(&[]).await?;
    // One connection, as consecutive migration files share one: a write
    // caches the capture function's plan over the transition table, the table
    // gains a column, and the next write must still be captured.
    store
        .as_writer(
            "INSERT INTO leases VALUES (1, 'a', 'a@x'); \
             ALTER TABLE leases ADD COLUMN note text; \
             INSERT INTO leases VALUES (2, 'b', 'b@x', 'n')",
        )
        .await?;
    store.log().await
}

#[tokio::test]
async fn a_shape_change_mid_session_still_captures() {
    let Some(store) = Store::create("cap_midsession").await else {
        return;
    };
    let out = reshaped_mid_session(&store).await;
    store.cleanup().await;

    let log = out.expect("both writes succeed on one connection");
    assert_eq!(ops(&log), ["i", "i"], "{log:#?}");
    assert_eq!(
        images(&log),
        [
            Some(json!({"id": 1, "name": "a", "contact_email": "a@x"})),
            Some(json!({"id": 2, "name": "b", "contact_email": "b@x"})),
        ],
        "the added column waits for the next install"
    );
}

// ── What capture refuses ─────────────────────────────────────────────────────

async fn misattached(store: &Store) -> Result<(Result<(), String>, i64), String> {
    let capture = store.leases(&[]).await?;
    let schema = store.app.schema_name();
    let relid = capture.shape.relid;
    store
        .as_writer(&format!(
            "CREATE TABLE other (id bigint PRIMARY KEY); \
             CREATE TRIGGER stolen AFTER INSERT ON other REFERENCING NEW TABLE AS oxy_new \
               FOR EACH STATEMENT EXECUTE FUNCTION {schema}._oxy_capture_{relid}_i()"
        ))
        .await?;
    let insert = store.as_writer("INSERT INTO other VALUES (1)").await;
    let logged = store
        .owner_count(&format!("SELECT count(*) FROM {CHANGES_LOG_TABLE}"))
        .await?;
    Ok((insert, logged))
}

/// The writer holds `EXECUTE` on its capture functions, so nothing but the
/// function itself stops it attaching one to another table to write history
/// in the first table's name. No guard exists here to refuse the trigger.
#[tokio::test]
async fn the_function_refuses_another_table() {
    let Some(store) = Store::create("cap_refuse").await else {
        return;
    };
    let out = misattached(&store).await;
    store.cleanup().await;

    let (insert, logged) = out.expect("install, and attach the function elsewhere");
    let err = insert.expect_err("a capture function on another table fails the write");
    assert!(err.starts_with("42501"), "{err}");
    assert!(
        err.contains("captures only the table it was generated for"),
        "{err}"
    );
    assert_eq!(logged, 0);
}

async fn unseeded(store: &Store) -> Result<(Result<(), String>, i64), String> {
    store.leases(&[]).await?;
    store
        .as_owner(&format!("DELETE FROM {CHANGES_STREAM_TABLE}"))
        .await?;
    let insert = store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x')")
        .await;
    let rows = scalar(&store.writer_dsn, "SELECT count(*) FROM leases").await?;
    Ok((insert, rows))
}

/// A store whose stream was never seeded fails loudly rather than going
/// uncaptured.
#[tokio::test]
async fn an_unseeded_stream_fails_the_write() {
    let Some(store) = Store::create("cap_unseeded").await else {
        return;
    };
    let out = unseeded(&store).await;
    store.cleanup().await;

    let (insert, rows) = out.expect("install, and remove the stream");
    let err = insert.expect_err("no stream row fails the write");
    assert!(err.starts_with("55000"), "{err}");
    assert!(err.contains("never seeded"), "{err}");
    assert_eq!(rows, 0, "the write rolled back with its capture");
}

async fn paused(store: &Store) -> Result<Vec<LogRow>, String> {
    store.leases(&[]).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x')")
        .await?;
    store
        .as_owner(&format!(
            "UPDATE {CHANGES_STREAM_TABLE} SET capturing = false"
        ))
        .await?;
    store
        .as_writer(
            "INSERT INTO leases VALUES (2, 'b', 'b@x'); \
             UPDATE leases SET name = 'z'; \
             DELETE FROM leases WHERE id = 1; \
             TRUNCATE leases",
        )
        .await?;
    store.log().await
}

#[tokio::test]
async fn a_non_capturing_stream_writes_nothing() {
    let Some(store) = Store::create("cap_paused").await else {
        return;
    };
    let out = paused(&store).await;
    store.cleanup().await;

    let log = out.expect("every write succeeds");
    assert_eq!(
        ops(&log),
        ["i"],
        "only the write made while capturing: {log:#?}"
    );
}

const INVOCATION: &str = "6f1c2b9a-3d4e-4f5a-8b6c-7d8e9f0a1b2c";

/// A transaction that sets the three settings as the host signs one, then
/// inserts `values` into `leases`.
fn signed_insert(actor: &str, invocation: &str, provenance: &str, values: &str) -> String {
    format!(
        "BEGIN; \
         SELECT pg_catalog.set_config('{CHANGES_ACTOR_SETTING}', '{actor}', true), \
                pg_catalog.set_config('{CHANGES_INVOCATION_SETTING}', '{invocation}', true), \
                pg_catalog.set_config('{CHANGES_PROVENANCE_SETTING}', '{provenance}', true); \
         INSERT INTO leases VALUES {values}; \
         COMMIT"
    )
}

async fn settings(store: &Store) -> Result<Vec<LogRow>, String> {
    store.leases(&[]).await?;
    let client = connect(&store.writer_dsn).await?;
    for sql in [
        signed_insert(
            "user:alice",
            INVOCATION,
            &"ab".repeat(32),
            "(1, 'a', 'a@x')",
        ),
        // The same session afterwards, where each setting reads '' rather
        // than unset.
        "INSERT INTO leases VALUES (2, 'b', 'b@x')".to_string(),
        // Empty; hyphens the uuid cast would refuse; hex in upper case.
        signed_insert("", &"-".repeat(36), &"AB".repeat(32), "(3, 'c', 'c@x')"),
        // An actor is free text; the rest malformed.
        signed_insert("user:bob", "not-a-uuid", "abc", "(4, 'd', 'd@x')"),
    ] {
        client.batch_execute(&sql).await.map_err(render_pg_error)?;
    }
    store.log().await
}

/// The trigger copies who made the change from the settings the host signs;
/// a malformed one is null and never fails the write (the relay checks the
/// MAC, not this).
#[tokio::test]
async fn settings_are_copied_and_malformed_ones_are_null() {
    let Some(store) = Store::create("cap_settings").await else {
        return;
    };
    let out = settings(&store).await;
    store.cleanup().await;

    let log = out.expect("every write succeeds");
    let signed: Vec<(Option<&str>, Option<Uuid>, Option<&[u8]>)> = log
        .iter()
        .map(|r| (r.actor.as_deref(), r.invocation, r.provenance.as_deref()))
        .collect();
    let invocation = Uuid::parse_str(INVOCATION).expect("a uuid");
    let mac = vec![0xab_u8; 32];
    assert_eq!(
        signed,
        [
            (Some("user:alice"), Some(invocation), Some(mac.as_slice())),
            (None, None, None),
            (None, None, None),
            (Some("user:bob"), None, None),
        ]
    );
}

// ── What the DDL guard holds ─────────────────────────────────────────────────

struct GuardSeen {
    refused: Vec<(&'static str, Result<(), String>)>,
    added_column: Result<(), String>,
    enabled_always: i64,
    ops: Vec<String>,
}

async fn guard_refusals(store: &Store) -> Result<GuardSeen, String> {
    let capture = store.leases(&[]).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x')")
        .await?;
    let mut refused = Vec::new();
    for (what, sql) in [
        (
            "disabling a capture trigger",
            "ALTER TABLE leases DISABLE TRIGGER oxy_capture_i",
        ),
        (
            "disabling every trigger",
            "ALTER TABLE leases DISABLE TRIGGER ALL",
        ),
        (
            "enabling a capture trigger for origin sessions only",
            "ALTER TABLE leases ENABLE TRIGGER oxy_capture_u",
        ),
        (
            "dropping a capture trigger",
            "DROP TRIGGER oxy_capture_d ON leases",
        ),
        (
            "renaming the table",
            "ALTER TABLE leases RENAME TO leases_old",
        ),
    ] {
        refused.push((what, store.as_writer(sql).await));
    }
    let added_column = store
        .as_writer("ALTER TABLE leases ADD COLUMN note text")
        .await;
    let enabled_always = store
        .owner_count(&format!(
            "SELECT count(*) FROM pg_trigger WHERE tgrelid = {}::oid AND tgenabled = 'A' \
               AND tgname IN ('oxy_capture_i', 'oxy_capture_u', 'oxy_capture_d', 'oxy_capture_t')",
            capture.shape.relid
        ))
        .await?;
    store
        .as_writer("INSERT INTO leases (id, name) VALUES (2, 'b')")
        .await?;
    let ops = store.log().await?.into_iter().map(|r| r.op).collect();
    Ok(GuardSeen {
        refused,
        added_column,
        enabled_always,
        ops,
    })
}

/// The writer owns the table, so only the guard stops it switching capture
/// off; each refused statement rolls back, and capture carries on.
#[tokio::test]
async fn the_guard_refuses_disabling_dropping_or_renaming() {
    let Some(store) = Store::guarded("cap_guard").await else {
        return;
    };
    let out = guard_refusals(&store).await;
    store.cleanup().await;

    let seen = out.expect("install under the guard, and write");
    for (what, result) in &seen.refused {
        let err = result.as_ref().expect_err(what);
        assert!(err.starts_with("42501"), "{what}: {err}");
        assert!(
            err.contains("remove it from oltpChanges and publish first"),
            "{what}: {err}"
        );
    }
    seen.added_column
        .expect("a column added to a captured table passes the guard");
    assert_eq!(
        seen.enabled_always, 4,
        "every capture trigger is as installed"
    );
    assert_eq!(seen.ops, ["i", "i"], "capture works after every refusal");
}

struct UninstallSeen {
    removal: Result<(), String>,
    renamed: Result<(), String>,
    logged: usize,
    tables: i64,
    shapes: i64,
}

async fn uninstalled(store: &Store) -> Result<UninstallSeen, String> {
    let capture = store.leases(&[]).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x')")
        .await?;
    let removal = store.uninstall(&capture).await;
    let renamed = store
        .as_writer(
            "INSERT INTO leases VALUES (2, 'b', 'b@x'); \
             ALTER TABLE leases RENAME TO leases_old",
        )
        .await;
    let tables = store
        .owner_count(&format!("SELECT count(*) FROM {CHANGES_TABLES_TABLE}"))
        .await?;
    let shapes = store
        .owner_count(&format!(
            "SELECT count(*) FROM {CHANGES_SHAPES_TABLE} WHERE relid = {}::oid",
            capture.shape.relid
        ))
        .await?;
    Ok(UninstallSeen {
        removal,
        renamed,
        logged: store.log().await?.len(),
        tables,
        shapes,
    })
}

/// A removal sets `installed` false first, so the guard lets the triggers go,
/// and after it the table is the app's to change.
#[tokio::test]
async fn the_guard_lets_an_uninstalled_table_go() {
    let Some(store) = Store::guarded("cap_uninst").await else {
        return;
    };
    let out = uninstalled(&store).await;
    store.cleanup().await;

    let seen = out.expect("install under the guard, and write");
    seen.removal
        .expect("the removal passes the guard, triggers before functions");
    seen.renamed
        .expect("an uncaptured table writes and renames freely");
    assert_eq!(seen.logged, 1, "only the write made while captured");
    assert_eq!(seen.tables, 0, "the table is no longer registered");
    assert_eq!(
        seen.shapes, 1,
        "its shape stays, for the history already shipped"
    );
}

async fn dropped_table(
    store: &Store,
) -> Result<(Result<(), String>, Vec<LogRow>, i64, i64), String> {
    let capture = store.leases(&[]).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x')")
        .await?;
    let dropped = store.as_writer("DROP TABLE leases").await;
    let tables = store
        .owner_count(&format!("SELECT count(*) FROM {CHANGES_TABLES_TABLE}"))
        .await?;
    let shapes = store
        .owner_count(&format!(
            "SELECT count(*) FROM {CHANGES_SHAPES_TABLE} WHERE relid = {}::oid",
            capture.shape.relid
        ))
        .await?;
    Ok((dropped, store.log().await?, tables, shapes))
}

/// A drop is recorded, not refused: the table's current state empties at the
/// marker, and its history stays.
#[tokio::test]
async fn dropping_a_declared_table_leaves_a_marker() {
    let Some(store) = Store::guarded("cap_dropmark").await else {
        return;
    };
    let out = dropped_table(&store).await;
    store.cleanup().await;

    let (dropped, log, tables, shapes) = out.expect("install under the guard, and write");
    dropped.expect("the writer may drop its captured table");
    assert_eq!(ops(&log), ["i", "t"], "{log:#?}");
    let marker = &log[1];
    assert_eq!((&marker.key, &marker.row_image), (&None, &None));
    assert_eq!(
        (marker.writer.as_str(), marker.table_name.as_str()),
        (store.writer.as_str(), "leases")
    );
    assert_eq!((marker.generation, marker.schema_version), (1, 1));
    assert_eq!(tables, 0, "the dropped table is no longer registered");
    assert_eq!(shapes, 1, "its shape stays");
}
