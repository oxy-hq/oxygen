//! Platform step 8, `oxy_changes`, against a real Postgres: the schema is the
//! owner's alone, the step succeeds where the owner may not create event
//! triggers (the local provider, so the fixture itself), it runs again
//! cleanly, and where a superuser applies it the guard exists and runs.

use crate::{Fixture, attempt, render_pg_error};

use oxy_oltp::platform::{
    CHANGES_DROP_TRIGGER, CHANGES_GUARD_TRIGGER, CHANGES_LOG_TABLE, CHANGES_STREAM_TABLE,
    steps_since,
};
use oxy_oltp::sql::{PgSqlExecutor, TenantSqlExecutor};

/// One integer from `sql`, run as the fixture's owner.
async fn owner_scalar(fx: &Fixture, sql: &str) -> Result<i64, String> {
    let client = oxy_oltp::connect::connect(&fx.owner_dsn, "changes step test")
        .await
        .map_err(render_pg_error)?;
    client
        .query_one(sql, &[])
        .await
        .map(|row| row.get::<_, i64>(0))
        .map_err(render_pg_error)
}

/// Step 8's statements alone, for this fixture's database.
pub(crate) fn step_eight(fx: &Fixture) -> Vec<String> {
    steps_since(7, "local", &fx.database)
        .expect("platform sql")
        .into_iter()
        .filter(|step| step.version == 8)
        .flat_map(|step| step.statements)
        .collect()
}

/// The cluster superuser's DSN, naming the fixture's database: the only
/// role here that may create an event trigger.
pub(crate) fn admin_dsn_for(database: &str) -> String {
    let mut dsn = crate::admin_dsn();
    let at = dsn.rfind('/').expect("a DSN with a database");
    dsn.replace_range(at + 1.., database);
    dsn
}

/// How many of the step's two event triggers the fixture's database holds.
async fn event_triggers(fx: &Fixture) -> Result<i64, String> {
    owner_scalar(
        fx,
        &format!(
            "SELECT count(*) FROM pg_event_trigger \
             WHERE evtname IN ('{CHANGES_GUARD_TRIGGER}', '{CHANGES_DROP_TRIGGER}')"
        ),
    )
    .await
}

#[tokio::test]
async fn step_eight_creates_the_schema_and_no_writer_can_read_it() {
    let Some(fx) = Fixture::create("changes_schema").await else {
        return;
    };
    let app = fx.app("ledger");
    let dsn = fx.add_writer(&app).await;

    // Read before asserting, so a failure still drops the database and roles.
    let objects = owner_scalar(
        &fx,
        "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'oxy_changes' \
           AND c.relname IN ('stream', 'seq', 'log', 'log_seq', 'tables', 'shapes')",
    )
    .await;
    let select = attempt(&dsn, &format!("SELECT * FROM {CHANGES_LOG_TABLE}")).await;
    let seed = attempt(
        &dsn,
        &format!("INSERT INTO {CHANGES_STREAM_TABLE} (generation, capturing) VALUES (1, false)"),
    )
    .await;
    fx.cleanup(&[&app]).await;

    assert_eq!(
        objects,
        Ok(6),
        "the stream, the sequence, the log and its index, tables and shapes"
    );
    let err = select.expect_err("a writer must not read the log");
    assert!(err.starts_with("42501"), "{err}");
    let err = seed.expect_err("nor seed the stream, which would switch capture off");
    assert!(err.starts_with("42501"), "{err}");
}

/// The fixture applies every step as the local provider's owner, a
/// `CREATEROLE` role that is no superuser: creating it at all is the step
/// succeeding without the privilege.
#[tokio::test]
async fn step_eight_skips_the_guard_without_privilege() {
    let Some(fx) = Fixture::create("changes_noguard").await else {
        return;
    };
    let superuser = owner_scalar(
        &fx,
        "SELECT count(*) FROM pg_roles WHERE rolname = current_user AND rolsuper",
    )
    .await;
    let triggers = event_triggers(&fx).await;
    let functions = owner_scalar(
        &fx,
        "SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
         WHERE n.nspname = 'oxy_changes' AND p.proname IN ('guard_ddl', 'record_drop')",
    )
    .await;
    fx.cleanup(&[]).await;

    assert_eq!(
        superuser,
        Ok(0),
        "the premise: the local owner is no superuser"
    );
    assert_eq!(
        triggers,
        Ok(0),
        "the owner may not create event triggers here, so the step skipped them"
    );
    assert_eq!(
        functions,
        Ok(2),
        "the functions are still created, ready for a role that may create the triggers"
    );
}

#[tokio::test]
async fn step_eight_is_idempotent() {
    let Some(fx) = Fixture::create("changes_idem").await else {
        return;
    };
    let statements = step_eight(&fx);
    let second = PgSqlExecutor
        .execute_batch(&fx.owner_dsn, &statements)
        .await;
    let third = PgSqlExecutor
        .execute_batch(&fx.owner_dsn, &statements)
        .await;
    let streams = owner_scalar(&fx, &format!("SELECT count(*) FROM {CHANGES_STREAM_TABLE}")).await;
    fx.cleanup(&[]).await;

    second.expect("step 8 runs a second time");
    third.expect("and a third");
    assert_eq!(streams, Ok(0), "no run of the step seeds the stream");
}

/// Where the role applying the step may create event triggers (Neon's owner,
/// through `neon_superuser`; the cluster superuser here), it does, the guard
/// runs on the owner's DDL and lets an uncaptured table be, refuses a capture
/// function attached to a table it was not generated for once the store
/// captures anything, and the owner can apply the step again with the
/// triggers in place.
#[tokio::test]
async fn step_eight_creates_the_guard_where_permitted() {
    let Some(fx) = Fixture::create("changes_guard").await else {
        return;
    };
    let statements = step_eight(&fx);
    let as_superuser = PgSqlExecutor
        .execute_batch(&admin_dsn_for(&fx.database), &statements)
        .await;
    let triggers = event_triggers(&fx).await;
    let uncaptured = attempt(
        &fx.owner_dsn,
        "CREATE TABLE public.changes_probe (id int PRIMARY KEY); \
         ALTER TABLE public.changes_probe ADD COLUMN note text; \
         DROP TABLE public.changes_probe",
    )
    .await;
    let markers = owner_scalar(&fx, &format!("SELECT count(*) FROM {CHANGES_LOG_TABLE}")).await;
    // The guard checks nothing in a store that captures no table, so register
    // one (not yet installed: the first check skips it) before misattaching.
    let misattached = attempt(
        &fx.owner_dsn,
        "INSERT INTO oxy_changes.tables \
           (relid, schema_name, table_name, writer, schema_version, installed) \
           VALUES (0, 'app_changes_probe', 'registered', 'changes_probe', 1, false); \
         CREATE SCHEMA app_changes_probe; \
         CREATE FUNCTION app_changes_probe._oxy_capture_1_i() RETURNS trigger \
           LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$; \
         CREATE TABLE app_changes_probe.t (id int PRIMARY KEY); \
         CREATE TRIGGER stolen AFTER INSERT ON app_changes_probe.t \
           FOR EACH STATEMENT EXECUTE FUNCTION app_changes_probe._oxy_capture_1_i()",
    )
    .await;
    let again = PgSqlExecutor
        .execute_batch(&fx.owner_dsn, &statements)
        .await;
    fx.cleanup(&[]).await;

    as_superuser.expect("the superuser applies step 8");
    assert_eq!(triggers, Ok(2), "both event triggers exist");
    uncaptured
        .expect("DDL on a table nothing captures passes the guard, and its drop records nothing");
    assert_eq!(markers, Ok(0), "a table nothing captures leaves no marker");
    let err = misattached.expect_err("a capture function on another table is refused");
    assert!(err.starts_with("42501"), "{err}");
    assert!(err.contains("generated for another table"), "{err}");
    again.expect("the owner re-applies the step with the triggers in place");
}

/// Step 9's statements alone, for this fixture's database.
fn step_nine(fx: &Fixture) -> Vec<String> {
    steps_since(8, "local", &fx.database)
        .expect("platform sql")
        .into_iter()
        .filter(|step| step.version == 9)
        .flat_map(|step| step.statements)
        .collect()
}

#[tokio::test]
async fn step_nine_applies_twice() {
    let Some(fx) = Fixture::create("changes_step_nine").await else {
        return;
    };
    let statements = step_nine(&fx);
    let second = PgSqlExecutor
        .execute_batch(&fx.owner_dsn, &statements)
        .await;
    let third = PgSqlExecutor
        .execute_batch(&fx.owner_dsn, &statements)
        .await;
    let progress = owner_scalar(
        &fx,
        "SELECT (to_regclass('oxy_changes.progress') IS NOT NULL)::int::bigint",
    )
    .await;
    fx.cleanup(&[]).await;

    second.expect("step 9 runs a second time");
    third.expect("and a third");
    assert_eq!(progress, Ok(1), "the progress table exists");
}
