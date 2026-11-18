//! The events outbox in an org's OLTP store (`oxy_events.outbox`, platform
//! step 5), against a real tenant provisioned by the real `OltpProvisioner`.
//!
//! The outbox is a security boundary as much as a table: an app's writer role
//! may call `oxy_events.publish` and nothing else in `oxy_events`, and the row
//! records `session_user`, so one app cannot publish as another. Every one of
//! those properties is a Postgres grant, so each is asserted by connecting as
//! the role in question and watching Postgres refuse — not by reading the DDL.
//!
//! The grant audit must agree: a writer holding exactly that door is clean,
//! so it can still be re-provisioned and rotated, while the door in anyone
//! else's hands, or a writer reaching past it, is drift.

use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use oxy_custom_apps::events::jsonb_text_len;
use oxy_oltp::GrantLevel;
use oxy_oltp::entity::tenants::{self as oltp_tenants, Entity as OltpTenants};
use oxy_oltp::events::{self, EventsError, OutboxTarget, PUBLISH_SQL};
use oxy_oltp::grant_audit::{RoleFindings, tenant_findings};
use oxy_oltp::provisioner::WriterCredentials;
use tokio_postgres::error::SqlState;

use crate::oltp_fx::{Fx, with_fx};

/// The definer function's signature, as `has_function_privilege` names it.
const PUBLISH_FUNCTION: &str =
    "oxy_events.publish(uuid,text,text,text,jsonb,uuid,text,uuid,uuid,int,uuid,int)";

async fn connect(dsn: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(dsn, tokio_postgres::NoTls)
        .await
        .unwrap_or_else(|e| panic!("connect: {e}"));
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

/// The SQLSTATE and message: `Display` on a server error is the bare "db
/// error".
fn rendered(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => format!("{}: {}", db.code().code(), db.message()),
        None => e.to_string(),
    }
}

/// Provision the org's store and one app writer, made able to publish the way
/// the host does before a first publish.
async fn publishing_writer(fx: &Fx, base: &str) -> WriterCredentials {
    fx.provisioner
        .provision(fx.org_id)
        .await
        .expect("provision");
    let creds = fx
        .ensure_writer(&fx.writer(base), GrantLevel::ReadWrite)
        .await
        .expect("ensure writer");
    events::ensure_publishable(&fx.db, fx.org_id, &creds.role_name)
        .await
        .expect("ensure publishable");
    creds
}

/// The tenant owner — the role the relay reads the outbox as.
async fn owner(fx: &Fx) -> tokio_postgres::Client {
    let tenant = fx.tenant_row().await.expect("tenant row");
    connect(&oxy_oltp::migrator::owner_dsn(&tenant).expect("owner dsn")).await
}

/// One publish through [`PUBLISH_SQL`], as an app's host makes it.
async fn publish(
    client: &impl tokio_postgres::GenericClient,
    id: Uuid,
    name: &str,
    data: &Value,
) -> Result<Uuid, tokio_postgres::Error> {
    let row = client
        .query_one(
            PUBLISH_SQL,
            &[
                &id,
                &"production",
                &name,
                &format!("key:{id}"),
                data,
                &None::<Uuid>,
                &Some("user:test"),
                &None::<Uuid>,
                &id,
                &0i32,
                &None::<Uuid>,
                &0i32,
            ],
        )
        .await?;
    Ok(row.get(0))
}

/// `oxy oltp audit`'s report for the tenant: every recorded writer, the
/// analyst and `PUBLIC`.
async fn audit(fx: &Fx, owner: &tokio_postgres::Client) -> Vec<RoleFindings> {
    let tenant = fx.tenant_row().await.expect("tenant row");
    tenant_findings(&fx.db, owner, &tenant)
        .await
        .expect("audit runs")
}

fn findings_of<'a>(report: &'a [RoleFindings], role: &str) -> &'a [String] {
    &report
        .iter()
        .find(|r| r.role == role)
        .unwrap_or_else(|| panic!("{role} is audited: {report:?}"))
        .findings
}

async fn outbox_count(owner: &tokio_postgres::Client) -> i64 {
    owner
        .query_one("SELECT count(*) FROM oxy_events.outbox", &[])
        .await
        .expect("count the outbox")
        .get(0)
}

#[tokio::test]
async fn a_writer_can_publish_and_the_row_records_its_role() {
    with_fx(|fx| async move {
        let creds = publishing_writer(&fx, "publisher").await;
        let writer = connect(&creds.dsn).await;

        let id = Uuid::new_v4();
        let data = json!({ "statementId": "s1" });
        let returned = publish(&writer, id, "statement.uploaded", &data)
            .await
            .unwrap_or_else(|e| panic!("publish: {}", rendered(&e)));
        assert_eq!(returned, id, "publish returns the event id");

        let row = owner(&fx)
            .await
            .query_one(
                "SELECT writer_role, environment, name, key, data, actor, root_id, depth, \
                 delay_seconds FROM oxy_events.outbox WHERE id = $1",
                &[&id],
            )
            .await
            .expect("the published row");
        assert_eq!(
            row.get::<_, String>(0),
            creds.role_name,
            "the row records the calling writer role, not the function's owner"
        );
        assert_eq!(row.get::<_, String>(1), "production");
        assert_eq!(row.get::<_, String>(2), "statement.uploaded");
        assert_eq!(row.get::<_, String>(3), format!("key:{id}"));
        assert_eq!(row.get::<_, Value>(4), data);
        assert_eq!(
            row.get::<_, Option<String>>(5).as_deref(),
            Some("user:test")
        );
        assert_eq!(row.get::<_, Uuid>(6), id);
        assert_eq!(row.get::<_, i32>(7), 0);
        assert_eq!(row.get::<_, i32>(8), 0);
    })
    .await;
}

#[tokio::test]
async fn a_writer_cannot_touch_the_outbox_directly() {
    with_fx(|fx| async move {
        let creds = publishing_writer(&fx, "toucher").await;
        let writer = connect(&creds.dsn).await;
        let id = Uuid::new_v4();
        publish(&writer, id, "a.thing", &json!({}))
            .await
            .unwrap_or_else(|e| panic!("publish: {}", rendered(&e)));

        let forged = Uuid::new_v4();
        for statement in [
            "SELECT * FROM oxy_events.outbox".to_string(),
            format!(
                "INSERT INTO oxy_events.outbox (id, writer_role, environment, name, key, data, \
                 root_id) VALUES ('{forged}', 'someone_else', 'production', 'a.thing', 'k', \
                 '{{}}', '{forged}')"
            ),
            "UPDATE oxy_events.outbox SET writer_role = 'someone_else'".to_string(),
            "DELETE FROM oxy_events.outbox".to_string(),
            "CREATE TABLE oxy_events.sneak (id int)".to_string(),
        ] {
            let err = writer
                .batch_execute(&statement)
                .await
                .expect_err(&format!("a writer may not run {statement}"));
            assert!(
                rendered(&err).contains("permission denied"),
                "{statement}: {}",
                rendered(&err)
            );
        }

        let owner = owner(&fx).await;
        assert_eq!(
            outbox_count(&owner).await,
            1,
            "nothing was added or removed"
        );
        let role: String = owner
            .query_one(
                "SELECT writer_role FROM oxy_events.outbox WHERE id = $1",
                &[&id],
            )
            .await
            .expect("the published row")
            .get(0);
        assert_eq!(role, creds.role_name, "nothing was rewritten");
    })
    .await;
}

#[tokio::test]
async fn the_analyst_cannot_publish() {
    with_fx(|fx| async move {
        // A writer holds the grant, so the function is callable by someone:
        // what the analyst is refused is its own lack of one.
        let _creds = publishing_writer(&fx, "analysed").await;
        fx.provisioner
            .ensure_analyst(fx.org_id)
            .await
            .expect("ensure analyst");
        let conn = oxy_oltp::resolver::resolve_analyst_connection_for_org(&fx.db, fx.org_id)
            .await
            .expect("resolve the analyst connection");
        let analyst = connect(&conn.dsn()).await;

        let err = publish(&analyst, Uuid::new_v4(), "a.thing", &json!({}))
            .await
            .expect_err("the analyst may not publish");
        assert!(
            rendered(&err).contains("permission denied"),
            "{}",
            rendered(&err)
        );

        let owner = owner(&fx).await;
        assert_eq!(outbox_count(&owner).await, 0, "nothing was published");

        // Nor can anyone through PUBLIC: the function's default EXECUTE grant
        // is revoked, and the schema gives PUBLIC nothing.
        let tenant = fx.tenant_row().await.expect("tenant row");
        let analyst_role =
            oxy_oltp::schema::analyst_role_for(&tenant.provider, &tenant.database_name);
        for role in ["public", analyst_role.as_str()] {
            let row = owner
                .query_one(
                    "SELECT has_function_privilege($1::name, $2::text, 'EXECUTE'), \
                     has_schema_privilege($1::name, 'oxy_events', 'USAGE')",
                    &[&role, &PUBLISH_FUNCTION],
                )
                .await
                .expect("privilege probe");
            assert!(!row.get::<_, bool>(0), "{role} may execute publish");
            assert!(!row.get::<_, bool>(1), "{role} may use oxy_events");
        }
    })
    .await;
}

#[tokio::test]
async fn publishing_the_same_id_twice_keeps_one_row() {
    with_fx(|fx| async move {
        let creds = publishing_writer(&fx, "repeater").await;
        let mut writer = connect(&creds.dsn).await;
        let id = Uuid::new_v4();

        // Inside one transaction, as an app retrying a publish would: the
        // second must not abort the first, or the app's write.
        let tx = writer.transaction().await.expect("begin");
        let first = publish(&tx, id, "a.thing", &json!({ "n": 1 }))
            .await
            .unwrap_or_else(|e| panic!("first publish: {}", rendered(&e)));
        let second = publish(&tx, id, "a.thing", &json!({ "n": 2 }))
            .await
            .unwrap_or_else(|e| panic!("a repeated publish must not fail: {}", rendered(&e)));
        tx.commit()
            .await
            .expect("the transaction commits after a repeat");
        assert_eq!((first, second), (id, id));

        let rows = owner(&fx)
            .await
            .query("SELECT data FROM oxy_events.outbox WHERE id = $1", &[&id])
            .await
            .expect("the outbox rows");
        assert_eq!(rows.len(), 1, "one row per id");
        assert_eq!(
            rows[0].get::<_, Value>(0),
            json!({ "n": 1 }),
            "the first wins"
        );
    })
    .await;
}

/// The key is `(writer_role, id)`: a writer cannot take another's id. A row
/// another writer inserted under an id first does not turn the real publish
/// into a no-op, and a claim marks relayed only the rows it holds.
#[tokio::test]
async fn another_writers_row_with_the_same_id_does_not_block_a_publish() {
    with_fx(|fx| async move {
        let squatter = publishing_writer(&fx, "squatter").await;
        let honest = publishing_writer(&fx, "honest").await;
        let id = Uuid::new_v4();
        publish(
            &connect(&squatter.dsn).await,
            id,
            "a.thing",
            &json!({ "n": 0 }),
        )
        .await
        .unwrap_or_else(|e| panic!("squatter's publish: {}", rendered(&e)));
        publish(
            &connect(&honest.dsn).await,
            id,
            "a.thing",
            &json!({ "n": 1 }),
        )
        .await
        .unwrap_or_else(|e| panic!("honest publish: {}", rendered(&e)));

        let owner = owner(&fx).await;
        let landed: Vec<String> = owner
            .query(
                "SELECT writer_role FROM oxy_events.outbox WHERE id = $1 ORDER BY created_at",
                &[&id],
            )
            .await
            .expect("the outbox rows")
            .iter()
            .map(|r| r.get(0))
            .collect();
        assert_eq!(
            landed,
            [squatter.role_name.clone(), honest.role_name.clone()],
            "both rows landed"
        );

        // The oldest is the squatter's; marking it leaves the honest row
        // waiting.
        let claim = events::claim(&fx.db, fx.org_id, &OutboxTarget::Production, 1)
            .await
            .expect("claim");
        assert_eq!(claim.rows()[0].writer_role, squatter.role_name);
        claim
            .mark_relayed_and_commit(&[])
            .await
            .expect("mark relayed and commit");
        let left: Vec<String> = owner
            .query(
                "SELECT writer_role FROM oxy_events.outbox \
                  WHERE id = $1 AND relayed_at IS NULL",
                &[&id],
            )
            .await
            .expect("the outbox rows")
            .iter()
            .map(|r| r.get(0))
            .collect();
        assert_eq!(
            left,
            std::slice::from_ref(&honest.role_name),
            "only the claimed row was marked"
        );
    })
    .await;
}

#[tokio::test]
async fn a_rolled_back_publish_leaves_nothing() {
    with_fx(|fx| async move {
        let creds = publishing_writer(&fx, "rollback").await;
        let mut writer = connect(&creds.dsn).await;
        let table = format!("{}.statements", creds.schema_name);
        writer
            .batch_execute(&format!("CREATE TABLE {table} (id uuid PRIMARY KEY)"))
            .await
            .expect("the app's own table");

        // The app's write and its publish, rolled back together.
        let gone = Uuid::new_v4();
        let tx = writer.transaction().await.expect("begin");
        tx.execute(&format!("INSERT INTO {table} VALUES ($1)"), &[&gone])
            .await
            .expect("the app's write");
        publish(&tx, gone, "statement.uploaded", &json!({}))
            .await
            .unwrap_or_else(|e| panic!("publish: {}", rendered(&e)));
        tx.rollback().await.expect("rollback");

        let owner = owner(&fx).await;
        assert_eq!(outbox_count(&owner).await, 0, "the event never happened");

        // And committed together, both are there.
        let kept = Uuid::new_v4();
        let tx = writer.transaction().await.expect("begin");
        tx.execute(&format!("INSERT INTO {table} VALUES ($1)"), &[&kept])
            .await
            .expect("the app's write");
        publish(&tx, kept, "statement.uploaded", &json!({}))
            .await
            .unwrap_or_else(|e| panic!("publish: {}", rendered(&e)));
        tx.commit().await.expect("commit");

        let app_rows: i64 = writer
            .query_one(&format!("SELECT count(*) FROM {table}"), &[])
            .await
            .expect("count the app's rows")
            .get(0);
        assert_eq!(app_rows, 1, "only the committed write");
        let ids: Vec<Uuid> = owner
            .query("SELECT id FROM oxy_events.outbox", &[])
            .await
            .expect("the outbox")
            .iter()
            .map(|r| r.get(0))
            .collect();
        assert_eq!(ids, vec![kept], "only the committed publish");
    })
    .await;
}

/// `ensure_publishable` remembers a grant, never a refusal: an org with no
/// store is told how to get one, and a role refused before it existed is
/// granted once it does — the same `(org, role)` both times.
#[tokio::test]
async fn a_refused_ensure_publishable_is_not_remembered() {
    with_fx(|fx| async move {
        let writer = fx.writer("late");
        let database = oxy_oltp::provider::database_name_for(
            &oxy_oltp::provisioner::project_name_for(fx.org_id),
        );
        let role = oxy_oltp::schema::qualify_role(
            "local",
            &database,
            &writer.role_name(GrantLevel::ReadWrite),
        );

        let err = events::ensure_publishable(&fx.db, fx.org_id, &role)
            .await
            .expect_err("no store yet");
        assert!(
            matches!(err, EventsError::NotProvisioned { org_id } if org_id == fx.org_id),
            "{err}"
        );
        assert!(err.to_string().starts_with("EventsStoreMissing: "), "{err}");

        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let err = events::ensure_publishable(&fx.db, fx.org_id, &role)
            .await
            .expect_err("no role yet");
        assert!(matches!(err, EventsError::Store(_)), "{err}");

        let creds = fx
            .ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("ensure writer");
        assert_eq!(creds.role_name, role, "the role the refusals asked about");
        events::ensure_publishable(&fx.db, fx.org_id, &role)
            .await
            .expect("granted once the role exists");
        publish(
            &connect(&creds.dsn).await,
            Uuid::new_v4(),
            "a.thing",
            &json!({}),
        )
        .await
        .unwrap_or_else(|e| panic!("publish: {}", rendered(&e)));
    })
    .await;
}

/// Every store provisioned before this step is at platform version 4 with no
/// `oxy_events`; `ensure_publishable` brings it current before granting, as
/// an inbox write does for step 4.
#[tokio::test]
async fn a_store_from_before_the_outbox_gets_it_on_first_publish() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let creds = fx
            .ensure_writer(&fx.writer("older"), GrantLevel::ReadWrite)
            .await
            .expect("ensure writer");
        owner(&fx)
            .await
            .batch_execute("DROP SCHEMA oxy_events CASCADE")
            .await
            .expect("drop the outbox");
        OltpTenants::update_many()
            .col_expr(oltp_tenants::Column::PlatformSchemaVersion, Expr::value(4))
            .filter(oltp_tenants::Column::OrgId.eq(fx.org_id))
            .exec(&fx.db)
            .await
            .expect("wind the tenant back to version 4");

        events::ensure_publishable(&fx.db, fx.org_id, &creds.role_name)
            .await
            .expect("ensure publishable");

        let tenant = fx.tenant_row().await.expect("tenant row");
        assert_eq!(
            tenant.platform_schema_version,
            oxy_oltp::platform::PLATFORM_SCHEMA_VERSION
        );
        publish(
            &connect(&creds.dsn).await,
            Uuid::new_v4(),
            "a.thing",
            &json!({}),
        )
        .await
        .unwrap_or_else(|e| panic!("publish: {}", rendered(&e)));
    })
    .await;
}

#[tokio::test]
async fn a_second_claim_skips_rows_the_first_holds() {
    with_fx(|fx| async move {
        let creds = publishing_writer(&fx, "claimed").await;
        let writer = connect(&creds.dsn).await;

        // Three publishes, three transactions, so `created_at` orders them.
        let mut published = Vec::new();
        for n in 0..3 {
            let id = Uuid::new_v4();
            publish(&writer, id, "a.thing", &json!({ "n": n }))
                .await
                .unwrap_or_else(|e| panic!("publish: {}", rendered(&e)));
            published.push(id);
        }
        let ids = |claim: &events::OutboxClaim| -> Vec<Uuid> {
            claim.rows().iter().map(|r| r.id).collect()
        };

        let first = events::claim(&fx.db, fx.org_id, &OutboxTarget::Production, 2)
            .await
            .expect("first claim");
        assert_eq!(ids(&first), published[..2], "oldest first, up to the limit");
        assert!(
            first
                .rows()
                .iter()
                .all(|r| r.writer_role == creds.role_name),
            "rows carry the writer role"
        );

        let second = events::claim(&fx.db, fx.org_id, &OutboxTarget::Production, 10)
            .await
            .expect("second claim");
        assert_eq!(
            ids(&second),
            published[2..],
            "the second claim skips the rows the first holds"
        );

        // The second of the first claim's rows is discarded: deleted, not
        // marked.
        first
            .mark_relayed_and_commit(&[events::OutboxKey {
                writer_role: creds.role_name.clone(),
                id: published[1],
            }])
            .await
            .expect("mark relayed and commit");
        second.release().await.expect("release");

        let owner = owner(&fx).await;
        let rows: Vec<(Uuid, bool)> = owner
            .query(
                "SELECT id, relayed_at IS NOT NULL FROM oxy_events.outbox ORDER BY created_at",
                &[],
            )
            .await
            .expect("the outbox")
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect();
        assert_eq!(
            rows,
            [(published[0], true), (published[2], false)],
            "the first claim's kept row is marked relayed and stays as the body's log, its \
             discarded one is deleted, and the released one is still waiting"
        );

        let third = events::claim(&fx.db, fx.org_id, &OutboxTarget::Production, 10)
            .await
            .expect("third claim");
        assert_eq!(
            ids(&third),
            published[2..],
            "a released row is claimable again"
        );
        third
            .mark_relayed_and_commit(&[])
            .await
            .expect("mark relayed and commit");
        assert_eq!(
            outbox_count(&owner).await,
            2,
            "relayed rows stay as the log"
        );
        let again = events::claim(&fx.db, fx.org_id, &OutboxTarget::Production, 10)
            .await
            .expect("a claim after every row is relayed");
        assert!(
            again.rows().is_empty(),
            "a relayed row is never claimed again"
        );
        again.release().await.expect("release");
    })
    .await;
}

/// The door `ensure_publishable` opens is not drift: the audit is clean, and
/// the writer can still be re-provisioned and rotated — both of which end in
/// the grant assertion — and publishes on the rotated credential.
#[tokio::test]
async fn a_writer_that_publishes_passes_the_grant_audit() {
    with_fx(|fx| async move {
        let creds = publishing_writer(&fx, "audited").await;
        let writer = fx.writer("audited");
        let owner = owner(&fx).await;

        let held = owner
            .query_one(
                "SELECT has_schema_privilege($1::name, 'oxy_events', 'USAGE') \
                 AND has_function_privilege($1::name, $2::text, 'EXECUTE')",
                &[&creds.role_name, &PUBLISH_FUNCTION],
            )
            .await
            .expect("privilege probe");
        assert!(held.get::<_, bool>(0), "the writer holds the door");
        let report = audit(&fx, &owner).await;
        assert!(report.iter().all(|r| r.findings.is_empty()), "{report:?}");

        fx.ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("re-provisioning a publishing writer is not refused");
        let rotated = fx
            .provisioner
            .rotate_writer(fx.org_id, &writer, GrantLevel::ReadWrite)
            .await
            .expect("rotating a publishing writer is not refused");
        publish(
            &connect(&rotated.dsn).await,
            Uuid::new_v4(),
            "a.thing",
            &json!({}),
        )
        .await
        .unwrap_or_else(|e| panic!("publish after rotation: {}", rendered(&e)));

        let report = audit(&fx, &owner).await;
        assert!(report.iter().all(|r| r.findings.is_empty()), "{report:?}");
    })
    .await;
}

/// The door is a writer's alone: the analyst holding either half is a
/// finding, and `PUBLIC` holding `EXECUTE` is one that stops provisioning.
#[tokio::test]
async fn the_outbox_door_in_anyone_elses_hands_is_drift() {
    with_fx(|fx| async move {
        let creds = publishing_writer(&fx, "door").await;
        fx.provisioner
            .ensure_analyst(fx.org_id)
            .await
            .expect("ensure analyst");
        let tenant = fx.tenant_row().await.expect("tenant row");
        let analyst = oxy_oltp::schema::analyst_role_for(&tenant.provider, &tenant.database_name);
        let owner = owner(&fx).await;

        owner
            .batch_execute(&format!(
                "GRANT USAGE ON SCHEMA oxy_events TO \"{analyst}\"; \
                 GRANT EXECUTE ON FUNCTION {PUBLISH_FUNCTION} TO \"{analyst}\""
            ))
            .await
            .expect("hand the analyst the door");
        let report = audit(&fx, &owner).await;
        let held = findings_of(&report, &analyst);
        assert!(
            held.contains(&"USAGE on schema oxy_events".to_string()),
            "{report:?}"
        );
        assert!(
            held.iter()
                .any(|f| f.starts_with("EXECUTE on function oxy_events.publish(")),
            "{report:?}"
        );
        assert!(
            findings_of(&report, &creds.role_name).is_empty(),
            "{report:?}"
        );

        owner
            .batch_execute(&format!(
                "GRANT EXECUTE ON FUNCTION {PUBLISH_FUNCTION} TO PUBLIC"
            ))
            .await
            .expect("hand PUBLIC the function");
        let report = audit(&fx, &owner).await;
        assert!(
            findings_of(&report, "PUBLIC")
                .iter()
                .any(|f| f.starts_with("EXECUTE on function oxy_events.publish(")),
            "{report:?}"
        );
        let err = fx
            .ensure_writer(&fx.writer("door"), GrantLevel::ReadWrite)
            .await
            .expect_err("PUBLIC's reach stops provisioning")
            .to_string();
        assert!(
            err.contains("PUBLIC holds EXECUTE on function oxy_events.publish("),
            "{err}"
        );
    })
    .await;
}

/// The exemption is the door and nothing past it: a writer that can also read
/// the outbox, or call another function in `oxy_events`, is drift, and
/// re-provisioning it is refused naming exactly that.
#[tokio::test]
async fn a_writer_reaching_past_the_outbox_door_is_drift() {
    with_fx(|fx| async move {
        let creds = publishing_writer(&fx, "past").await;
        let role = &creds.role_name;
        let owner = owner(&fx).await;
        owner
            .batch_execute(&format!(
                "GRANT SELECT ON oxy_events.outbox TO \"{role}\"; \
                 CREATE FUNCTION oxy_events.other() RETURNS int LANGUAGE sql AS 'SELECT 1'; \
                 REVOKE ALL ON FUNCTION oxy_events.other() FROM PUBLIC; \
                 GRANT EXECUTE ON FUNCTION oxy_events.other() TO \"{role}\""
            ))
            .await
            .expect("hand the writer more than the door");

        let report = audit(&fx, &owner).await;
        assert_eq!(
            findings_of(&report, role),
            [
                "EXECUTE on function oxy_events.other()".to_string(),
                "SELECT on oxy_events.outbox".to_string(),
            ],
            "the door itself is not listed; everything past it is"
        );

        let err = fx
            .ensure_writer(&fx.writer("past"), GrantLevel::ReadWrite)
            .await
            .expect_err("re-provisioning a drifted writer is refused")
            .to_string();
        assert!(err.contains("SELECT on oxy_events.outbox"), "{err}");
        assert!(
            !err.contains("USAGE on schema oxy_events"),
            "the door is not drift: {err}"
        );
    })
    .await;
}

/// A remembered grant that has gone — a re-mint's `DROP OWNED` takes it with
/// the role — refuses the publish `42501`; forgetting the pair makes the next
/// `ensure_publishable` grant again.
#[tokio::test]
async fn a_forgotten_grant_is_made_again() {
    with_fx(|fx| async move {
        let creds = publishing_writer(&fx, "forgotten").await;
        let role = &creds.role_name;
        owner(&fx)
            .await
            .batch_execute(&format!(
                "REVOKE EXECUTE ON FUNCTION {PUBLISH_FUNCTION} FROM \"{role}\"; \
                 REVOKE USAGE ON SCHEMA oxy_events FROM \"{role}\""
            ))
            .await
            .expect("take the grants away");
        let writer = connect(&creds.dsn).await;

        events::ensure_publishable(&fx.db, fx.org_id, role)
            .await
            .expect("remembered, so nothing is granted");
        let err = publish(&writer, Uuid::new_v4(), "a.thing", &json!({}))
            .await
            .expect_err("the grant is gone");
        assert_eq!(
            err.code(),
            Some(&SqlState::INSUFFICIENT_PRIVILEGE),
            "{}",
            rendered(&err)
        );

        events::forget_publishable(fx.org_id, role);
        events::ensure_publishable(&fx.db, fx.org_id, role)
            .await
            .expect("granted again");
        publish(&writer, Uuid::new_v4(), "a.thing", &json!({}))
            .await
            .unwrap_or_else(|e| panic!("publish after forgetting: {}", rendered(&e)));
    })
    .await;
}

/// One `oxy_events.publish` call as a writer's own SQL can make it, with the
/// key, data, depth and delay it chooses.
async fn publish_raw(
    client: &impl tokio_postgres::GenericClient,
    key: &str,
    data: &Value,
    depth: i32,
    delay_seconds: i32,
) -> Result<Uuid, tokio_postgres::Error> {
    let id = Uuid::new_v4();
    let row = client
        .query_one(
            PUBLISH_SQL,
            &[
                &id,
                &"production",
                &"a.thing",
                &key,
                data,
                &None::<Uuid>,
                &None::<String>,
                &None::<Uuid>,
                &id,
                &depth,
                &None::<Uuid>,
                &delay_seconds,
            ],
        )
        .await?;
    Ok(row.get(0))
}

/// The writer may call the definer function from its own SQL, past every
/// check the host makes (`ctx.oltp` SQL built while it runs). The function
/// holds the bounds itself: a depth past 8, a delay past a day, a key outside
/// 1–200 characters or data past 64 KiB as `jsonb` text — the relay's own
/// limit, by the relay's own measure (R32) — is refused
/// `invalid_parameter_value`, naming the field, and writes nothing; each
/// limit itself is accepted.
#[tokio::test]
async fn the_definer_refuses_a_call_out_of_bounds() {
    with_fx(|fx| async move {
        let creds = publishing_writer(&fx, "bounded").await;
        let writer = connect(&creds.dsn).await;
        let data = json!({ "n": 1 });
        // `{"b": "…"}` as `jsonb` text: 9 bytes around the string.
        let at_limit = json!({ "b": "x".repeat(65_536 - 9) });
        let over_limit = json!({ "b": "x".repeat(65_536 - 8) });
        let long_key = "k".repeat(201);
        let wide_key = "é".repeat(200);

        let refused: [(&str, &str, &Value, i32, i32); 7] = [
            ("depth", "k", &data, 9, 0),
            ("depth", "k", &data, -1, 0),
            ("delay", "k", &data, 0, 86_401),
            ("delay", "k", &data, 0, -1),
            ("key", "", &data, 0, 0),
            ("key", &long_key, &data, 0, 0),
            ("data", "k", &over_limit, 0, 0),
        ];
        for (field, key, data, depth, delay) in refused {
            let err = publish_raw(&writer, key, data, depth, delay)
                .await
                .expect_err(field);
            assert_eq!(
                err.code(),
                Some(&SqlState::INVALID_PARAMETER_VALUE),
                "{}",
                rendered(&err)
            );
            let message = rendered(&err);
            assert!(
                message.contains("oxy_events.publish") && message.contains(field),
                "{message}"
            );
        }
        let owner = owner(&fx).await;
        assert_eq!(
            outbox_count(&owner).await,
            0,
            "nothing out of bounds is written"
        );

        for (key, data, depth, delay) in [
            ("depth-8", &data, 8, 0),
            ("delay-day", &data, 0, 86_400),
            (wide_key.as_str(), &data, 0, 0),
            ("data-limit", &at_limit, 0, 0),
        ] {
            publish_raw(&writer, key, data, depth, delay)
                .await
                .unwrap_or_else(|e| panic!("{key} is within bounds: {}", rendered(&e)));
        }
        assert_eq!(outbox_count(&owner).await, 4);
    })
    .await;
}

/// R32: the host measures a payload ([`jsonb_text_len`]) as the store does
/// (`octet_length(data::text)`, the definer function's bound and the relay's),
/// for every value the host can hold: Postgres keeps a number as `numeric`
/// and writes every digit of it, so `1e300` (5 bytes as JSON) is 301; and a
/// published row is measured by the relay as the host measured it. Asked of
/// Postgres here, value by value, with each value sent as the host sends it.
#[tokio::test]
async fn the_hosts_measure_of_a_payload_is_the_stores() {
    with_fx(|fx| async move {
        let creds = publishing_writer(&fx, "measured").await;
        let writer = connect(&creds.dsn).await;
        let parsed = |text: &str| serde_json::from_str::<Value>(text).expect(text);
        let values = [
            json!(1e300),
            json!(-1e-7),
            json!(1.5e-7),
            json!(5e-324),
            json!(f64::MAX),
            json!(f64::MIN),
            json!(1.0),
            json!(-0.0),
            parsed("123456789012345680000"),
            parsed("1e-400"),
            json!(1e16),
            json!(1e15),
            json!(1e-6),
            json!(0.1),
            json!(2.5e-5),
            json!(123.456),
            json!(u64::MAX),
            json!(i64::MIN),
            json!(0),
            json!(null),
            json!(true),
            json!(false),
            json!([]),
            json!({}),
            json!([[1, [2, [3, []]]], {"a": {"b": {"c": [null, {}]}}}]),
            json!({"é": "ü", "ключ": "値", "😀": ["😀"], "a\"b": 1, "": ""}),
            json!("tab\t nl\n cr\r ff\u{0c} bs\u{08} quote\" back\\ slash/ unit\u{1f} del\u{7f} ls\u{2028}"),
            json!({"orderId": 1, "xs": [1e300, -1e-7, 1.5e-7, 5e-324, f64::MAX, 1.0, -0.0]}),
        ];
        for value in &values {
            let row = writer
                .query_one(
                    "SELECT octet_length(($1::jsonb)::text), ($1::jsonb)::text",
                    &[value],
                )
                .await
                .unwrap_or_else(|e| panic!("{value}: {}", rendered(&e)));
            let (bytes, text): (i32, String) = (row.get(0), row.get(1));
            assert_eq!(
                jsonb_text_len(value),
                bytes as usize,
                "{value} is stored as {text}"
            );
        }

        // Published, the relay's claim reads the length the host measured.
        let payload = &values[values.len() - 1];
        publish_raw(&writer, "measured", payload, 0, 0)
            .await
            .unwrap_or_else(|e| panic!("publish: {}", rendered(&e)));
        let claim = events::claim(&fx.db, fx.org_id, &OutboxTarget::Production, 10)
            .await
            .expect("claim");
        let rows = claim.rows().to_vec();
        claim.release().await.expect("release");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].data_bytes as usize, jsonb_text_len(payload));
    })
    .await;
}

/// R40: a claim whose marking fails rolls back there and then (an explicit
/// `ROLLBACK`, as `release` sends), so the rows it locked are free for the
/// next pass at once, before the failed pass reopens the mark.
#[tokio::test]
async fn a_claim_whose_marking_fails_frees_its_rows_at_once() {
    with_fx(|fx| async move {
        let creds = publishing_writer(&fx, "unmarkable").await;
        let writer = connect(&creds.dsn).await;
        publish_raw(&writer, "k", &json!({ "n": 1 }), 0, 0)
            .await
            .unwrap_or_else(|e| panic!("publish: {}", rendered(&e)));
        let owner = owner(&fx).await;
        owner
            .batch_execute(
                "CREATE FUNCTION oxy_events.refuse_marks() RETURNS trigger LANGUAGE plpgsql AS \
                 $$ BEGIN RAISE EXCEPTION 'marking refused (test)'; END $$; \
                 CREATE TRIGGER refuse_marks BEFORE UPDATE ON oxy_events.outbox \
                 FOR EACH ROW EXECUTE FUNCTION oxy_events.refuse_marks()",
            )
            .await
            .expect("make marking fail");

        for _ in 0..5 {
            let claim = events::claim(&fx.db, fx.org_id, &OutboxTarget::Production, 10)
                .await
                .expect("claim");
            assert_eq!(claim.rows().len(), 1, "the row is free to claim");
            claim
                .mark_relayed_and_commit(&[])
                .await
                .expect_err("marking fails");
            // At once, on another connection: the row is not still locked.
            let locked = owner
                .query_one(
                    "SELECT count(*) FROM (SELECT 1 FROM oxy_events.outbox \
                      WHERE relayed_at IS NULL FOR UPDATE SKIP LOCKED) AS free",
                    &[],
                )
                .await;
            let free: i64 = match locked {
                Ok(row) => row.get(0),
                Err(e) => panic!("{}", rendered(&e)),
            };
            assert_eq!(free, 1, "the failed claim still holds its row");
        }
    })
    .await;
}

/// R42 (N1): whether rows are left waiting that a claim did not take is read
/// from the claim's own snapshot. A row another claim holds counts; a row
/// committed after the claim does not — under steady publishing nearly every
/// pass sees one, and counting it would keep a healthy outbox from draining.
#[tokio::test]
async fn only_rows_another_claim_holds_count_as_held_elsewhere() {
    with_fx(|fx| async move {
        let creds = publishing_writer(&fx, "snapshot").await;
        let writer = connect(&creds.dsn).await;
        publish_raw(&writer, "first", &json!({ "n": 1 }), 0, 0)
            .await
            .unwrap_or_else(|e| panic!("publish: {}", rendered(&e)));

        let claim = events::claim(&fx.db, fx.org_id, &OutboxTarget::Production, 10)
            .await
            .expect("claim");
        assert_eq!(claim.rows().len(), 1);
        publish_raw(&writer, "after", &json!({ "n": 2 }), 0, 0)
            .await
            .unwrap_or_else(|e| panic!("publish: {}", rendered(&e)));
        assert!(
            !claim.others_waiting(),
            "a row committed after the claim is not left by it"
        );
        claim.release().await.expect("release");

        // Held: one claim takes the oldest row, the next sees it left.
        let holder = events::claim(&fx.db, fx.org_id, &OutboxTarget::Production, 1)
            .await
            .expect("hold");
        let next = events::claim(&fx.db, fx.org_id, &OutboxTarget::Production, 10)
            .await
            .expect("claim beside it");
        assert_eq!(next.rows().len(), 1, "the unheld row");
        assert!(next.others_waiting(), "a row another claim holds is");
        next.release().await.expect("release");
        holder.release().await.expect("release");
    })
    .await;
}
