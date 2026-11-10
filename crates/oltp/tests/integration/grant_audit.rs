//! The grant audit, against a real Postgres: a clean tenant reports nothing,
//! and each kind of drift it exists for is found, attributed once, and stops
//! provisioning.

use crate::{Fixture, attempt, render_pg_error};

use oxy_oltp::grant_audit::{
    GRANT_DRIFT_SQLSTATE, GrantScope, assert_role_grants_sql, public_findings_sql,
    role_findings_sql,
};
use oxy_oltp::provider::OltpProvider;
use oxy_oltp::schema::{self, GrantLevel, WriterRef};
use oxy_oltp::sql::{PgSqlExecutor, TenantSqlExecutor};

async fn rows(fx: &Fixture, sql: &str) -> Vec<String> {
    let client = oxy_oltp::connect::connect(&fx.owner_dsn, "grant audit test")
        .await
        .expect("owner connects");
    client
        .query(sql, &[])
        .await
        .map_err(render_pg_error)
        .expect("audit query runs")
        .iter()
        .map(|r| r.get::<_, String>(0))
        .collect()
}

async fn findings(fx: &Fixture, scope: &GrantScope) -> Vec<String> {
    rows(fx, &role_findings_sql(scope).expect("valid scope")).await
}

fn writer_scope(fx: &Fixture, writer: &WriterRef) -> GrantScope {
    GrantScope::for_writer(writer, GrantLevel::ReadWrite, &fx.role_name(writer))
}

/// A table a migration made in `writer`'s schema, as migrations do: owned by
/// the database owner, so the owner can grant on it.
async fn owner_table(fx: &Fixture, writer: &WriterRef, table: &str) -> String {
    let name = format!("{}.{table}", writer.schema_name());
    fx.as_owner(&format!("CREATE TABLE {name} (id int)")).await;
    name
}

#[tokio::test]
async fn a_freshly_provisioned_tenant_has_no_findings() {
    let Some(fx) = Fixture::create("audit_clean").await else {
        return;
    };
    let app = fx.app("bookings");
    let pipeline = fx.pipeline("toast");
    fx.add_writer(&app).await;
    let pipeline_dsn = fx.add_writer(&pipeline).await;
    owner_table(&fx, &app, "visits").await;
    attempt(&pipeline_dsn, "CREATE TABLE sales (id serial)")
        .await
        .expect("pipeline creates its own table");
    fx.provider
        .create_role(&fx.database, "local", &fx.analyst_role())
        .await
        .expect("analyst role");
    PgSqlExecutor
        .execute_batch(
            &fx.owner_dsn,
            &schema::grant_analyst_schema_sql(&pipeline, &fx.analyst_role()),
        )
        .await
        .expect("expose the pipeline");

    assert_eq!(
        findings(&fx, &writer_scope(&fx, &app)).await,
        Vec::<String>::new()
    );
    assert_eq!(
        findings(&fx, &writer_scope(&fx, &pipeline)).await,
        Vec::<String>::new()
    );
    let analyst = GrantScope::analyst(&fx.analyst_role(), vec![pipeline.schema_name()]);
    assert_eq!(findings(&fx, &analyst).await, Vec::<String>::new());
    assert_eq!(
        rows(&fx, &public_findings_sql()).await,
        Vec::<String>::new()
    );
    for writer in [&app, &pipeline] {
        let assertion = assert_role_grants_sql(&writer_scope(&fx, writer)).unwrap();
        attempt(&fx.owner_dsn, &assertion)
            .await
            .expect("a clean writer passes the provisioning check");
    }

    fx.cleanup(&[&app, &pipeline]).await;
}

#[tokio::test]
async fn a_hand_made_cross_writer_grant_is_found_and_stops_provisioning() {
    let Some(fx) = Fixture::create("audit_cross").await else {
        return;
    };
    let app = fx.app("marketing");
    let other = fx.app("clinic");
    fx.add_writer(&app).await;
    fx.add_writer(&other).await;
    let patients = owner_table(&fx, &other, "patients").await;

    let role = fx.role_name(&app);
    fx.as_owner(&format!(
        "GRANT USAGE ON SCHEMA {} TO \"{role}\"; GRANT SELECT, UPDATE ON {patients} TO \"{role}\"",
        other.schema_name()
    ))
    .await;

    let got = findings(&fx, &writer_scope(&fx, &app)).await;
    assert!(
        got.contains(&format!("USAGE on schema {}", other.schema_name())),
        "{got:?}"
    );
    assert!(got.contains(&format!("SELECT on {patients}")), "{got:?}");
    assert!(got.contains(&format!("UPDATE on {patients}")), "{got:?}");
    // The victim's own role is untouched by someone else's grant.
    assert_eq!(
        findings(&fx, &writer_scope(&fx, &other)).await,
        Vec::<String>::new()
    );

    let assertion = assert_role_grants_sql(&writer_scope(&fx, &app)).unwrap();
    let err = attempt(&fx.owner_dsn, &assertion)
        .await
        .expect_err("provisioning refuses a drifted writer");
    assert!(err.starts_with(GRANT_DRIFT_SQLSTATE), "{err}");
    assert!(err.contains(&format!("SELECT on {patients}")), "{err}");

    fx.cleanup(&[&app, &other]).await;
}

#[tokio::test]
async fn the_analyst_may_read_exposed_schemas_but_never_write() {
    let Some(fx) = Fixture::create("audit_analyst").await else {
        return;
    };
    let pipeline = fx.pipeline("toast");
    let app = fx.app("bookings");
    fx.add_writer(&pipeline).await;
    fx.add_writer(&app).await;
    let sales = owner_table(&fx, &pipeline, "sales").await;
    let visits = owner_table(&fx, &app, "visits").await;
    fx.provider
        .create_role(&fx.database, "local", &fx.analyst_role())
        .await
        .expect("analyst role");
    let analyst = fx.analyst_role();
    fx.as_owner(&format!(
        "GRANT USAGE ON SCHEMA {p} TO \"{analyst}\"; GRANT SELECT, INSERT ON {sales} TO \"{analyst}\"; \
         GRANT USAGE ON SCHEMA {a} TO \"{analyst}\"; GRANT SELECT ON {visits} TO \"{analyst}\"",
        p = pipeline.schema_name(),
        a = app.schema_name(),
    ))
    .await;

    let got = findings(
        &fx,
        &GrantScope::analyst(&analyst, vec![pipeline.schema_name()]),
    )
    .await;
    assert!(
        got.contains(&format!("INSERT on {sales}")),
        "write on an exposed schema: {got:?}"
    );
    assert!(
        !got.contains(&format!("SELECT on {sales}")),
        "reading it is allowed: {got:?}"
    );
    assert!(
        got.contains(&format!("USAGE on schema {}", app.schema_name())),
        "{got:?}"
    );
    assert!(
        got.contains(&format!("SELECT on {visits}")),
        "app data not exposed: {got:?}"
    );

    fx.cleanup(&[&pipeline, &app]).await;
}

#[tokio::test]
async fn a_grant_to_public_is_reported_once_under_public() {
    let Some(fx) = Fixture::create("audit_public").await else {
        return;
    };
    let app = fx.app("bookings");
    let other = fx.app("clinic");
    fx.add_writer(&app).await;
    fx.add_writer(&other).await;
    let patients = owner_table(&fx, &other, "patients").await;
    fx.as_owner(&format!(
        "GRANT USAGE ON SCHEMA {} TO PUBLIC; GRANT SELECT ON {patients} TO PUBLIC",
        other.schema_name()
    ))
    .await;

    let public = rows(&fx, &public_findings_sql()).await;
    assert!(
        public.contains(&format!("SELECT on {patients}")),
        "{public:?}"
    );
    assert!(
        public.contains(&format!("USAGE on schema {}", other.schema_name())),
        "{public:?}"
    );
    assert_eq!(
        findings(&fx, &writer_scope(&fx, &app)).await,
        Vec::<String>::new(),
        "PUBLIC's privileges are not repeated per role"
    );

    // But every writer holds them through PUBLIC, so provisioning an unrelated
    // writer still stops on them.
    let assertion = assert_role_grants_sql(&writer_scope(&fx, &app)).unwrap();
    let err = attempt(&fx.owner_dsn, &assertion)
        .await
        .expect_err("a PUBLIC grant into another app's data stops provisioning");
    assert!(err.starts_with(GRANT_DRIFT_SQLSTATE), "{err}");
    assert!(
        err.contains(&format!("PUBLIC holds SELECT on {patients}")),
        "{err}"
    );

    fx.cleanup(&[&app, &other]).await;
}

#[tokio::test]
async fn a_pipeline_may_own_its_extra_schemas_but_not_another_writers_name() {
    let Some(fx) = Fixture::create("audit_pipeline").await else {
        return;
    };
    let pipeline = fx.pipeline("toast");
    let app = fx.app("bookings");
    let pipeline_dsn = fx.add_writer(&pipeline).await;
    fx.add_writer(&app).await;

    let extra = format!("toast_extra_{}", fx.database.len());
    attempt(&pipeline_dsn, &format!("CREATE SCHEMA {extra}"))
        .await
        .expect("a pipeline may create schemas");
    assert_eq!(
        findings(&fx, &writer_scope(&fx, &pipeline)).await,
        Vec::<String>::new()
    );

    let squat = format!("app_squat_{}", fx.database.len());
    attempt(&pipeline_dsn, &format!("CREATE SCHEMA {squat}"))
        .await
        .expect("Postgres lets it; the audit is what notices");
    let got = findings(&fx, &writer_scope(&fx, &pipeline)).await;
    assert!(got.contains(&format!("owns schema {squat}")), "{got:?}");

    // An app writer never holds CREATE on the database.
    fx.as_owner(&format!(
        "GRANT CREATE ON DATABASE \"{}\" TO \"{}\"",
        fx.database,
        fx.role_name(&app)
    ))
    .await;
    let got = findings(&fx, &writer_scope(&fx, &app)).await;
    assert!(
        got.iter().any(|f| f.starts_with("CREATE on database ")),
        "{got:?}"
    );

    attempt(
        &fx.owner_dsn,
        &format!("DROP SCHEMA {squat}; DROP SCHEMA {extra}"),
    )
    .await
    .ok();
    fx.cleanup(&[&pipeline, &app]).await;
}
