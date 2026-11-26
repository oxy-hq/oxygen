//! A cast to `json` on an app's own type, against a real Postgres (OLTP
//! change stream design §5.2, "Hardening").
//!
//! The capture functions are `SECURITY DEFINER`, owned by the store's owner.
//! `to_jsonb` converts a value of a type that is not built in through that
//! type's cast to `json`, when one exists, calling the cast's function as
//! whoever is running; it does so for every column, excluded ones too,
//! through domains, array elements and composite fields. The writer owns the
//! types its migrations create, so it may create such a cast. Capture
//! therefore builds every image through `oxy_changes.image_<writer>`, a definer owned
//! by a role with no privileges: each test here is that migration, and pins
//! that the cast runs as that role, never as the owner.
//!
//! The hostile function proves it never ran as the owner by what it would
//! have left: a number taken from `oxy_changes.batches`, which no rollback
//! returns, `capturing` switched off, and a forged log row.

use serde_json::json;

use oxy_oltp::changes::{OWNER_SESSION_PIN_SQL, image_function, image_role_for, owner_install_sql};
use oxy_oltp::platform::{CHANGES_LOG_TABLE, CHANGES_TABLES_TABLE};
use oxy_oltp::schema::{WriterRef, database_search_path_sql};
use oxy_oltp::sql::{PgSqlExecutor, TenantSqlExecutor};

use crate::changes_capture::Store;
use crate::render_pg_error;

/// An app's enum, as its migration creates it.
const MOOD: &str = "CREATE TYPE mood AS ENUM ('ok', 'meh')";

/// A cast from `mood` to `json` whose function does what only the store's
/// owner may, then returns a plausible value.
const HOSTILE_CAST: &str = "CREATE FUNCTION mood_json(m mood) RETURNS json LANGUAGE plpgsql AS $f$
BEGIN
  PERFORM pg_catalog.nextval('oxy_changes.batches');
  UPDATE oxy_changes.stream SET capturing = false;
  INSERT INTO oxy_changes.log (generation, writer, table_name, schema_version, op)
    VALUES (1, 'forged', 'forged', 1, 'i');
  RETURN pg_catalog.to_json(m::text);
END
$f$;
CREATE CAST (mood AS json) WITH FUNCTION mood_json(mood)";

/// A cast from `mood` to `json` that says who it ran as.
const WHOAMI_CAST: &str = "CREATE FUNCTION mood_json(mood) RETURNS json LANGUAGE sql \
       AS $f$ SELECT pg_catalog.to_json(current_user::text) $f$; \
     CREATE CAST (mood AS json) WITH FUNCTION mood_json(mood)";

/// `moods`, with `mood` to be excluded: `to_jsonb(r.*)` converts it anyway.
const MOODS: &str = "CREATE TABLE moods (id bigint PRIMARY KEY, mood mood, note text)";

/// What the owner-only side effects left: log rows, the `stream` rows, and
/// whether `oxy_changes.batches` ever handed out a number.
#[derive(Debug)]
struct Effects {
    logged: i64,
    stream: Vec<(i64, bool)>,
    batches_taken: i64,
}

async fn effects(store: &Store) -> Result<Effects, String> {
    Ok(Effects {
        logged: store
            .owner_count(&format!("SELECT count(*) FROM {CHANGES_LOG_TABLE}"))
            .await?,
        stream: store.stream().await?,
        batches_taken: store
            .owner_count("SELECT count(*) FROM oxy_changes.batches WHERE is_called")
            .await?,
    })
}

/// A write refused because the cast, running as the image role, reached for
/// `oxy_changes`, which that role may not use.
fn assert_refused_inside_the_cast(what: &str, result: &Result<(), String>) {
    let err = result.as_ref().expect_err(what);
    assert!(err.starts_with("42501"), "{what}: {err}");
    assert!(
        err.contains("permission denied for schema oxy_changes"),
        "{what}: {err}"
    );
}

struct EscalationSeen {
    /// `to_jsonb` of a `mood`, as the writer itself.
    premise: Result<(), String>,
    /// An insert, an update and a delete after the cast exists.
    writes: Vec<(&'static str, Result<(), String>)>,
    rows: i64,
    effects: Effects,
}

async fn escalation(store: &Store) -> Result<EscalationSeen, String> {
    store.as_writer(&format!("{MOOD}; {MOODS}")).await?;
    let capture = store.shape("moods", &["mood"]).await?;
    store.install(&capture, 1).await?;
    store
        .as_writer("INSERT INTO moods VALUES (1, 'ok', 'before the cast')")
        .await?;
    // A later migration.
    store.as_writer(HOSTILE_CAST).await?;
    let premise = store
        .as_writer("SELECT pg_catalog.to_jsonb('ok'::mood)")
        .await;
    let mut writes = Vec::new();
    for (what, sql) in [
        ("insert", "INSERT INTO moods VALUES (2, 'meh', 'after')"),
        ("update", "UPDATE moods SET note = 'changed'"),
        ("delete", "DELETE FROM moods"),
    ] {
        writes.push((what, store.as_writer(sql).await));
    }
    Ok(EscalationSeen {
        premise,
        writes,
        rows: store
            .writer_count("SELECT count(*) FROM moods WHERE note = 'before the cast'")
            .await?,
        effects: effects(store).await?,
    })
}

/// The escalation: a migration casts an app type to `json` with a function
/// that writes `oxy_changes`. Inside a capture it runs as the image role, so
/// each write that builds an image fails on `oxy_changes`, and nothing the
/// function tried happened.
#[tokio::test]
async fn an_app_cast_to_json_never_runs_as_the_owner() {
    let Some(store) = Store::create("cast_esc").await else {
        return;
    };
    let out = escalation(&store).await;
    store.cleanup().await;

    let seen = out.expect("install, cast, and write");
    let premise = seen
        .premise
        .expect_err("to_jsonb runs the cast, which the writer itself may not");
    assert!(
        premise.starts_with("42501") && premise.contains("oxy_changes"),
        "{premise}"
    );
    for (what, result) in &seen.writes {
        assert_refused_inside_the_cast(what, result);
    }
    assert_eq!(seen.rows, 1, "every refused write rolled back");
    assert_eq!(seen.effects.logged, 1, "only the write before the cast");
    assert_eq!(
        seen.effects.stream,
        [(1, true)],
        "capture was not switched off"
    );
    assert_eq!(
        seen.effects.batches_taken, 0,
        "the cast's function never ran as the owner"
    );
}

async fn whoami(store: &Store) -> Result<Vec<serde_json::Value>, String> {
    store
        .as_writer(&format!(
            "{MOOD}; CREATE TYPE feeling AS (mood mood, note text); CREATE DOMAIN calm AS mood; \
             CREATE TABLE moods (id bigint PRIMARY KEY, mood mood, moods mood[], \
               feeling feeling, calm calm)"
        ))
        .await?;
    let capture = store.shape("moods", &[]).await?;
    store.install(&capture, 1).await?;
    store.as_writer(WHOAMI_CAST).await?;
    store
        .as_writer(
            "INSERT INTO moods VALUES \
             (1, 'ok', ARRAY['meh']::mood[], ROW('ok', 'n')::feeling, 'ok')",
        )
        .await?;
    Ok(store
        .log()
        .await?
        .into_iter()
        .filter_map(|r| r.row_image)
        .collect())
}

/// A cast still decides how its own type's values appear in history, through
/// a column, an array's elements, a composite's fields and a domain, and it
/// runs as the image role: what it returns is that role's name.
#[tokio::test]
async fn a_cast_runs_as_the_image_role_and_its_output_is_the_image() {
    let Some(store) = Store::create("cast_whoami").await else {
        return;
    };
    let out = whoami(&store).await;
    let role = store.image_role();
    store.cleanup().await;

    let images = out.expect("install, cast, and write");
    assert_eq!(
        images,
        [json!({
            "id": 1,
            "mood": role,
            "moods": [role],
            "feeling": {"mood": role, "note": "n"},
            "calm": role,
        })]
    );
}

struct PrivilegesSeen {
    /// Whether the image role owns the image function, a definer.
    owned: i64,
    /// Whether the image role can neither log in nor do anything a
    /// role attribute grants.
    bare: i64,
    /// Role memberships the image role holds.
    memberships: i64,
    /// Privileges held that should not be, each a `true`.
    held: i64,
    /// Whether the store's owner may execute the image function.
    owner_executes: i64,
    /// The writer calling and replacing the image function.
    call: Result<(), String>,
    replace: Result<(), String>,
    /// A second install, over the image function the first made.
    reinstall: Result<(), String>,
}

async fn privileges(store: &Store) -> Result<PrivilegesSeen, String> {
    store.as_writer(&format!("{MOOD}; {MOODS}")).await?;
    let capture = store.shape("moods", &[]).await?;
    store.install(&capture, 1).await?;
    let role = store.image_role();
    let writer = store.fx.role_name(&store.app);
    let image = image_function(&store.writer);
    let function = format!("'{image}(anyelement)'");
    let owned = store
        .owner_count(&format!(
            "SELECT count(*) FROM pg_catalog.pg_proc p \
              WHERE p.oid = {function}::pg_catalog.regprocedure AND p.prosecdef \
                AND pg_catalog.pg_get_userbyid(p.proowner) = '{role}'"
        ))
        .await?;
    let bare = store
        .owner_count(&format!(
            "SELECT count(*) FROM pg_catalog.pg_roles WHERE rolname = '{role}' \
               AND NOT rolcanlogin AND NOT rolsuper AND NOT rolcreaterole \
               AND NOT rolcreatedb AND NOT rolreplication AND NOT rolbypassrls"
        ))
        .await?;
    let memberships = store
        .owner_count(&format!(
            "SELECT count(*) FROM pg_catalog.pg_auth_members m \
               JOIN pg_catalog.pg_roles r ON r.oid = m.member WHERE r.rolname = '{role}'"
        ))
        .await?;
    let held = store
        .owner_count(&format!(
            "SELECT count(*) FROM (VALUES \
               (has_function_privilege('public', {function}, 'EXECUTE')), \
               (has_function_privilege('{writer}', {function}, 'EXECUTE')), \
               (pg_has_role('{writer}', '{role}', 'MEMBER')), \
               (has_schema_privilege('{role}', 'oxy_changes', 'USAGE')), \
               (has_schema_privilege('{role}', 'oxy_changes', 'CREATE')), \
               (has_table_privilege('{role}', 'oxy_changes.log', 'INSERT')), \
               (has_table_privilege('{role}', 'oxy_changes.stream', 'UPDATE')), \
               (has_table_privilege('{role}', 'oxy_changes.tables', 'SELECT')), \
               (has_sequence_privilege('{role}', 'oxy_changes.batches', 'USAGE')), \
               (has_database_privilege('{role}', current_database(), 'CREATE')) \
             ) v(held) WHERE v.held"
        ))
        .await?;
    let owner_executes = store
        .owner_count(&format!(
            "SELECT count(*) WHERE has_function_privilege(current_user, {function}, 'EXECUTE')"
        ))
        .await?;
    let call = store.as_writer(&format!("SELECT {image}(1)")).await;
    let replace = store
        .as_writer(&format!(
            "CREATE OR REPLACE FUNCTION {image}(anyelement) RETURNS jsonb \
             LANGUAGE sql AS $f$ SELECT '{{}}'::jsonb $f$"
        ))
        .await;
    store
        .as_writer("CREATE TABLE more (id bigint PRIMARY KEY)")
        .await?;
    let more = store.shape("more", &[]).await?;
    let reinstall = store.install(&more, 1).await;
    Ok(PrivilegesSeen {
        owned,
        bare,
        memberships,
        held,
        owner_executes,
        call,
        replace,
        reinstall,
    })
}

/// The image function is the image role's, a definer only the store's owner
/// may execute; the writer can neither call nor replace it; and the image
/// role holds nothing: no attribute, no membership, no privilege on
/// `oxy_changes`.
#[tokio::test]
async fn the_image_function_is_the_image_roles_and_the_role_holds_nothing() {
    let Some(store) = Store::create("cast_privs").await else {
        return;
    };
    let out = privileges(&store).await;
    store.cleanup().await;

    let seen = out.expect("install and read privileges");
    assert_eq!(seen.owned, 1, "owned by the image role, a definer");
    assert_eq!(seen.bare, 1, "NOLOGIN, and no role attribute");
    assert_eq!(seen.memberships, 0, "a member of nothing");
    assert_eq!(seen.held, 0, "no privilege it should not hold");
    assert_eq!(seen.owner_executes, 1, "the store's owner executes it");
    for (what, result) in [("call", &seen.call), ("replace", &seen.replace)] {
        let err = result.as_ref().expect_err(what);
        assert!(err.starts_with("42501"), "{what}: {err}");
    }
    seen.reinstall
        .expect("a second install keeps the image function");
}

struct SnapshotSeen {
    /// Casts from `mood` in session A's snapshot, before and after B's.
    before: i64,
    after: i64,
    write: Result<(), String>,
    effects: Effects,
}

async fn snapshot(store: &Store) -> Result<SnapshotSeen, String> {
    store.as_writer(&format!("{MOOD}; {MOODS}")).await?;
    let capture = store.shape("moods", &[]).await?;
    store.install(&capture, 1).await?;
    let a = oxy_oltp::connect::connect(&store.writer_dsn, "changes casts session a")
        .await
        .map_err(render_pg_error)?;
    let casts = "SELECT count(*) FROM pg_catalog.pg_cast \
                 WHERE castsource = 'mood'::pg_catalog.regtype";
    a.batch_execute("BEGIN ISOLATION LEVEL REPEATABLE READ")
        .await
        .map_err(render_pg_error)?;
    let before: i64 = a
        .query_one(casts, &[])
        .await
        .map_err(render_pg_error)?
        .get(0);
    // Session B, committed after A's snapshot.
    store.as_writer(HOSTILE_CAST).await?;
    let after: i64 = a
        .query_one(casts, &[])
        .await
        .map_err(render_pg_error)?
        .get(0);
    let write = a
        .batch_execute("INSERT INTO moods VALUES (1, 'ok', 'n')")
        .await
        .map_err(render_pg_error);
    let _ = a.batch_execute("ROLLBACK").await;
    Ok(SnapshotSeen {
        before,
        after,
        write,
        effects: effects(store).await?,
    })
}

/// The race a catalog check loses: session A's repeatable-read snapshot does
/// not hold the cast session B commits after it, yet `to_jsonb` finds the
/// cast through the syscache and runs it. It runs as the image role, so A's
/// write fails on `oxy_changes` and nothing the function tried happened.
#[tokio::test]
async fn a_cast_committed_after_the_writers_snapshot_runs_with_no_privileges() {
    let Some(store) = Store::create("cast_rr").await else {
        return;
    };
    let out = snapshot(&store).await;
    store.cleanup().await;

    let seen = out.expect("install, open A, cast in B, write in A");
    assert_eq!(
        (seen.before, seen.after),
        (0, 0),
        "A's snapshot never holds B's cast"
    );
    assert_refused_inside_the_cast("A's write", &seen.write);
    assert_eq!(seen.effects.logged, 0, "{:?}", seen.effects);
    assert_eq!(seen.effects.stream, [(1, true)]);
    assert_eq!(seen.effects.batches_taken, 0);
}

/// Who ran the planted operator, one row per run.
const RAN: &str = "SELECT count(*) FROM ran";

/// The writer plants `<>(oid, integer)`, which records who ran it, and the
/// owner's default path names the writer's schema, as provisioning sets it.
async fn plant(store: &Store) -> Result<(), String> {
    let schema = store.app.schema_name();
    // `<>(oid, integer)` is not in `pg_catalog`, so an exact one anywhere on
    // the path wins over `<>(oid, oid)` with a coercion.
    store
        .as_writer(&format!(
            "CREATE TABLE ran (who text); GRANT INSERT ON ran TO PUBLIC; \
             CREATE FUNCTION planted_ne(oid, integer) RETURNS boolean LANGUAGE sql AS $f$ \
               INSERT INTO {schema}.ran VALUES (current_user::text); \
               SELECT $1 <> $2::pg_catalog.oid $f$; \
             CREATE OPERATOR <> (LEFTARG = oid, RIGHTARG = integer, FUNCTION = planted_ne); \
             CREATE TABLE leases (id bigint PRIMARY KEY, name text)"
        ))
        .await?;
    // The owner's default path names every app's schema, as provisioning
    // sets it.
    let path =
        database_search_path_sql(&store.fx.database, &[schema]).map_err(|e| e.to_string())?;
    PgSqlExecutor
        .execute_batch(&store.fx.owner_dsn, &path)
        .await
        .map_err(|e| e.to_string())
}

/// Who ran the planted operator, in order.
async fn ran_by(store: &Store) -> Result<Vec<String>, String> {
    let client = oxy_oltp::connect::connect(&store.writer_dsn, "changes casts planted")
        .await
        .map_err(render_pg_error)?;
    Ok(client
        .query("SELECT who FROM ran", &[])
        .await
        .map_err(render_pg_error)?
        .iter()
        .map(|r| r.get(0))
        .collect())
}

async fn planted(store: &Store) -> Result<(i64, i64, i64, Vec<String>), String> {
    plant(store).await?;
    // Unpinned, the owner resolves the planted operator.
    store.as_owner("SELECT 0::oid <> 0").await?;
    let unpinned = store.writer_count(RAN).await?;
    // The promote's session pin, then an install's own head statement.
    store
        .as_owner(&format!("{OWNER_SESSION_PIN_SQL}; SELECT 0::oid <> 0"))
        .await?;
    let capture = store.shape("leases", &[]).await?;
    let head = owner_install_sql(
        &oxy_oltp::changes::CaptureSpec {
            writer: store.writer.as_str(),
            capture: &capture,
            schema_version: 1,
        },
        &store.fx.role_name(&store.app),
        &store.image_role(),
    )
    .map_err(|e| e.to_string())?
    .remove(0);
    store
        .as_owner(&format!("BEGIN; {head}; SELECT 0::oid <> 0; COMMIT"))
        .await?;
    let pinned = store.writer_count(RAN).await?;
    // A whole install, and a captured write.
    store.install(&capture, 1).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a')")
        .await?;
    let installed = store.writer_count(RAN).await?;
    Ok((unpinned, pinned, installed, ran_by(store).await?))
}

/// An app can plant an operator on the owner's default path that wins for an
/// expression needing a coercion, as `oid <> 0` does. Pinned to
/// `pg_catalog, pg_temp`, the install's session and its transaction never
/// resolve it: only the unpinned demonstration ran it.
#[tokio::test]
async fn the_owner_install_never_resolves_an_operator_an_app_planted() {
    let Some(store) = Store::create("cast_plant").await else {
        return;
    };
    let out = planted(&store).await;
    let owner = store.fx.owner_role.clone();
    store.cleanup().await;

    let (unpinned, pinned, installed, who) = out.expect("plant, pin, install and write");
    assert_eq!(unpinned, 1, "unpinned, the owner runs the planted operator");
    assert_eq!(pinned, 1, "pinned, it does not");
    assert_eq!(installed, 1, "nor does an install, or a captured write");
    assert_eq!(who, [owner]);
}

/// A cast that writes `mark` over large object `oid`, then says who it ran
/// as.
fn lo_cast(oid: u32, mark: &str) -> String {
    format!(
        "CREATE FUNCTION mood_json(mood) RETURNS json LANGUAGE sql AS $f$ \
           SELECT pg_catalog.lo_put({oid}, 0, '{mark}'); \
           SELECT pg_catalog.to_json(current_user::text) $f$; \
         CREATE CAST (mood AS json) WITH FUNCTION mood_json(mood)"
    )
}

struct TwoAppsSeen {
    /// Each writer's image function, owned by that writer's own image role.
    functions: i64,
    b_write: Result<(), String>,
    a_images: Vec<serde_json::Value>,
    /// What A's large object holds at the end.
    object: i64,
}

/// Both apps create `mood` and `moods`, and each has its capture installed;
/// then the image functions owned by each writer's own role, counted.
async fn install_two(store: &Store, b: &WriterRef, b_dsn: &str) -> Result<i64, String> {
    let WriterRef::App(b_name) = b else {
        return Err(format!("{b} is not an app writer"));
    };
    for dsn in [store.writer_dsn.as_str(), b_dsn] {
        PgSqlExecutor
            .execute_batch(dsn, &[format!("{MOOD}; {MOODS}")])
            .await
            .map_err(|e| e.to_string())?;
    }
    let a_capture = store.shape("moods", &[]).await?;
    store.install(&a_capture, 1).await?;
    let b_capture = store.shape_of(b, "moods", &[]).await?;
    store.install_for(b, b_dsn, &b_capture, 1).await?;
    let owned_by = |writer: &str, role: &str| {
        format!(
            "(p.oid = '{}(anyelement)'::pg_catalog.regprocedure \
              AND pg_catalog.pg_get_userbyid(p.proowner) = '{role}')",
            image_function(writer)
        )
    };
    store
        .owner_count(&format!(
            "SELECT count(*) FROM pg_catalog.pg_proc p WHERE {} OR {}",
            owned_by(&store.writer, &store.image_role()),
            owned_by(b_name, &image_role_for("local", &store.fx.database, b_name)),
        ))
        .await
}

/// A large object of A's, holding `a`, which A lets its own image role
/// write: a privilege that needs no schema to reach. `SELECT` too, since
/// opening a large object to write opens it to read as well.
async fn a_object(store: &Store) -> Result<u32, String> {
    let a = oxy_oltp::connect::connect(&store.writer_dsn, "changes casts app a")
        .await
        .map_err(render_pg_error)?;
    let oid: u32 = a
        .query_one("SELECT pg_catalog.lo_from_bytea(0, 'a')", &[])
        .await
        .map_err(render_pg_error)?
        .get(0);
    a.batch_execute(&format!(
        "GRANT SELECT, UPDATE ON LARGE OBJECT {oid} TO \"{}\"",
        store.image_role()
    ))
    .await
    .map_err(render_pg_error)?;
    Ok(oid)
}

async fn two_apps(store: &Store, b: &WriterRef, b_dsn: &str) -> Result<TwoAppsSeen, String> {
    let functions = install_two(store, b, b_dsn).await?;
    let oid = a_object(store).await?;
    // B's cast reaches for A's object; A's own cast too, by A's choice.
    PgSqlExecutor
        .execute_batch(b_dsn, &[lo_cast(oid, "B")])
        .await
        .map_err(|e| e.to_string())?;
    let b_write = crate::attempt(b_dsn, "INSERT INTO moods VALUES (1, 'ok', 'n')").await;
    store.as_writer(&lo_cast(oid, "A")).await?;
    store
        .as_writer("INSERT INTO moods VALUES (1, 'ok', 'n')")
        .await?;
    let object = store
        .writer_count(&format!(
            "SELECT pg_catalog.get_byte(pg_catalog.lo_get({oid}), 0)::bigint"
        ))
        .await?;
    let a_images = store
        .log()
        .await?
        .into_iter()
        .filter(|r| r.writer == store.writer)
        .filter_map(|r| r.row_image)
        .collect();
    Ok(TwoAppsSeen {
        functions,
        b_write,
        a_images,
        object,
    })
}

/// Two apps in one store each get their own image role and function. App A
/// grants its own image role a privilege, here writing a large object of
/// A's, which needs no schema to reach: A's cast may use it, and B's cast,
/// running as B's image role, may not.
#[tokio::test]
async fn one_apps_grant_to_its_image_role_reaches_no_other_apps_cast() {
    let Some(store) = Store::create("cast_two").await else {
        return;
    };
    let b = store.fx.app("v");
    let b_dsn = store.fx.add_writer(&b).await;
    let out = two_apps(&store, &b, &b_dsn).await;
    let a_role = store.image_role();
    store.fx.cleanup(&[&store.app, &b]).await;

    let seen = out.expect("two apps install, grant, cast and write");
    assert_eq!(
        seen.functions, 2,
        "one function per writer, each its role's"
    );
    let err = seen.b_write.expect_err("B's cast may not write A's object");
    assert!(err.starts_with("42501"), "{err}");
    assert!(err.contains("large object"), "{err}");
    assert_eq!(seen.object, i64::from(b'A'), "only A's cast wrote it");
    assert_eq!(
        seen.a_images,
        [json!({"id": 1, "mood": a_role, "note": "n"})]
    );
}

async fn taken(store: &Store) -> Result<(Result<(), String>, i64), String> {
    let role = store.image_role();
    store
        .as_owner(&format!(
            "CREATE ROLE \"{role}\" NOLOGIN; \
             COMMENT ON ROLE \"{role}\" IS 'oxy_changes image role for someone_else'"
        ))
        .await?;
    store
        .as_writer("CREATE TABLE leases (id bigint PRIMARY KEY, name text)")
        .await?;
    let capture = store.shape("leases", &[]).await?;
    let install = store.install(&capture, 1).await;
    let tables = store
        .owner_count(&format!("SELECT count(*) FROM {CHANGES_TABLES_TABLE}"))
        .await?;
    Ok((install, tables))
}

/// A role that already has a writer's image-role name but is recorded for
/// another writer, as two slugs cut down to one name would be, refuses that
/// writer's install, naming both, rather than sharing the role.
#[tokio::test]
async fn an_image_role_recorded_for_another_writer_refuses_the_install() {
    let Some(store) = Store::create("cast_taken").await else {
        return;
    };
    let out = taken(&store).await;
    store.cleanup().await;

    let (install, tables) = out.expect("plant the role and create the table");
    let err = install.expect_err("a role recorded for another writer refuses the install");
    assert!(
        err.contains(&format!(
            "42710: oxy_changes: writer {} cannot use image role {}",
            store.writer,
            store.image_role()
        )),
        "{err}"
    );
    assert!(
        err.contains("which is recorded as oxy_changes image role for someone_else"),
        "{err}"
    );
    assert_eq!(tables, 0, "nothing of the install is left");
}
