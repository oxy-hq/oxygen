//! Live tests against the **real Neon API**.
//!
//! Everything else in this crate tests the client against a stub of Neon's
//! published response shapes. A stub proves the client agrees with the spec; it
//! cannot prove the spec matches the service. These do.
//!
//! **Opt-in, and not just by credential presence.** Running these creates and
//! destroys real projects in a real Neon org. `NEON_API_KEY` may legitimately be
//! set in a dev `.env` for `oxy oltp provision` (now `oxyc oltp provision`), so keying off it alone would
//! make an ordinary `cargo nextest run` start spending money:
//!
//! ```bash
//! OXY_NEON_LIVE_TEST=1 cargo nextest run -p oxy-oltp --test integration \
//!   -E 'test(neon_live)' --no-capture
//! ```
//!
//! Each test cleans up the project it made **even when an assertion fails** —
//! the assertions run after teardown, on captured values, because a panic would
//! otherwise leak a billable project every red run.

use oxy_oltp::provider::{CreateProjectRequest, NeonProvider, OltpProvider, Project};
use oxy_oltp::roles;
use oxy_oltp::schema::{self, WriterRef};

fn live() -> Option<NeonProvider> {
    if std::env::var("OXY_OLTP_NEON_LIVE_TEST").ok().as_deref() != Some("1") {
        return None;
    }
    let key = std::env::var(oxy_oltp::config::NEON_API_KEY_VAR).ok()?;
    let org = std::env::var(oxy_oltp::config::NEON_ORG_ID_VAR).ok()?;
    Some(NeonProvider::new(key, org))
}

/// Unique per run: Neon does NOT enforce unique project names, so a leftover
/// from a previous failed run would otherwise be silently adopted and then
/// deleted out from under whoever is looking at it.
fn unique_name(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after 1970")
        .as_nanos();
    format!("oxy-live-{tag}-{nanos}")
}

/// Non-panicking create, for a SECOND create in a test where the first project
/// is already live: a panic there leaks the first (billed) project. Captured and
/// asserted after teardown. [`create`] wraps this for the common first-create.
async fn try_create(p: &NeonProvider, name: &str) -> Result<Project, String> {
    p.create_project(CreateProjectRequest {
        name: name.to_string(),
        region_id: "aws-us-east-2".to_string(),
        // The version production actually provisions, not a literal beside it.
        // These are the only tests that talk to the real provider, so a
        // hardcoded number here means the shipped default has no live coverage
        // and the suite exercises a version we no longer send.
        pg_version: oxy_oltp::config::DEFAULT_PG_VERSION,
    })
    .await
    .map_err(|e| format!("create_project against live Neon: {e}"))
}

async fn create(p: &NeonProvider, name: &str) -> Project {
    try_create(p, name)
        .await
        .expect("create_project against live Neon")
}

/// The whole provisioning path, end to end, against the real service.
///
/// The load-bearing assertion is the **connection**: everything up to it could
/// pass with a wrong host or a mis-parsed password, and `sslmode=require` means
/// this also proves the shared TLS connector reaches a provider that refuses
/// plaintext — which no local test can, since the demo cluster has no
/// certificates.
#[tokio::test]
async fn a_real_project_is_reachable_over_tls_and_then_cleaned_up() {
    let Some(p) = live() else {
        return;
    };
    let name = unique_name("tls");
    let project = create(&p, &name).await;

    // Run the query BEFORE teardown, but capture rather than assert, so a
    // failure still deletes the project — including a missing owner password,
    // which captures here instead of panicking before teardown.
    let queried: Result<String, String> = async {
        let password = project
            .owner_role
            .password
            .clone()
            .ok_or("owner password was not disclosed at create")?;
        let dsn = project.dsn_for(&project.owner_role.name, &password);
        let client = oxy_oltp::connect::connect(&dsn, "neon live test")
            .await
            .map_err(|e| format!("connect: {e}"))?;
        let row = client
            .query_one("SELECT current_database()", &[])
            .await
            .map_err(|e| format!("query: {e}"))?;
        Ok(row.get::<_, String>(0))
    }
    .await;

    let deleted = p.delete_project(&project.id).await;

    assert!(
        project.host.contains("neon.tech"),
        "host should be a Neon endpoint, got {}",
        project.host
    );
    assert_eq!(
        queried.as_deref(),
        Ok(project.database.name.as_str()),
        "should have connected to the provisioned database over TLS"
    );
    deleted.expect("delete_project");
}

/// The analyst credential, its grants, and `sslmode=require` — end to end on
/// Neon, which no local cluster can cover.
///
/// The carried gap: the live suite only ever connected as the project owner, so
/// the analyst login, the grant SQL, and the TLS-required analyst DSN had never
/// run against the real service. This mints the analyst the way the product
/// does — `roles::ensure_login_role_sql` + the `schema::grant_analyst_*`
/// builders, the actual shipped SQL, not hand-rolled — grants read on ONE
/// schema, then connects AS the analyst over TLS and proves it reads what it
/// was granted and is denied what it was not.
///
/// Assertions run after teardown on captured values, so a failure still deletes
/// the billed project.
#[tokio::test]
async fn the_analyst_reads_its_granted_schema_over_tls_and_nothing_else() {
    let Some(p) = live() else {
        return;
    };
    let name = unique_name("analyst");
    let project = create(&p, &name).await;

    // Bare `oxy_analyst_ro`: `neon` does not share a role namespace, so the
    // analyst is unqualified (unlike the hashed local name).
    let analyst = schema::analyst_role_for("neon", &project.database.name);
    // Generated, not a literal: if `delete_project` below errors, a hardcoded
    // password would survive on a real project.
    let analyst_pw = roles::generate_password();
    let readable = WriterRef::app("bookings").expect("writer");
    let secret = WriterRef::app("secret").expect("writer");

    let outcome: Result<(), String> = async {
        // Inside the block so a missing password captures into `outcome` and
        // still deletes the project — an `expect` here would leak a billed one.
        let owner_pw = project
            .owner_role
            .password
            .clone()
            .ok_or("owner password was not disclosed at create")?;
        let owner_dsn = project.dsn_for(&project.owner_role.name, &owner_pw);
        let owner = oxy_oltp::connect::connect(&owner_dsn, "neon analyst test: owner")
            .await
            .map_err(|e| format!("owner connect: {e}"))?;

        // A schema the analyst will be granted, and one it will not.
        for stmt in [
            format!("CREATE SCHEMA {}", readable.schema_name()),
            format!("CREATE TABLE {}.orders (id int)", readable.schema_name()),
            format!("INSERT INTO {}.orders VALUES (1)", readable.schema_name()),
            format!("CREATE SCHEMA {}", secret.schema_name()),
            format!("CREATE TABLE {}.keys (id int)", secret.schema_name()),
        ] {
            owner
                .batch_execute(&stmt)
                .await
                .map_err(|e| format!("seed `{stmt}`: {e}"))?;
        }

        // Mint the analyst and grant read on the one schema — the product's SQL.
        let mut mint = roles::ensure_login_role_sql(&analyst, &analyst_pw)
            .map_err(|e| format!("build mint sql: {e}"))?;
        mint.push(roles::grant_connect_sql(&analyst).map_err(|e| format!("build connect: {e}"))?);
        mint.extend(schema::grant_analyst_schema_sql(&readable, &analyst));
        // Run here as the OWNER (who owns the seeded tables), so the
        // `ALTER DEFAULT PRIVILEGES` half records `defaclrole = owner`, not the
        // writer — production issues it writer-owned. The assertions turn on the
        // `GRANT SELECT ON ALL TABLES` half, which is identical either way; the
        // default-privilege shape is not what this test checks.
        mint.extend(schema::grant_analyst_tables_sql(&readable, &analyst));
        for stmt in &mint {
            owner
                .batch_execute(stmt)
                .await
                .map_err(|e| format!("mint/grant `{stmt}`: {e}"))?;
        }

        // Connect AS the analyst — `dsn_for` is `sslmode=require`, so this also
        // exercises the TLS-required path for the analyst credential.
        let analyst_dsn = project.dsn_for(&analyst, &analyst_pw);
        let a = oxy_oltp::connect::connect(&analyst_dsn, "neon analyst test: analyst")
            .await
            .map_err(|e| format!("analyst connect over TLS: {e}"))?;

        let n: i64 = a
            .query_one(
                &format!("SELECT count(*) FROM {}.orders", readable.schema_name()),
                &[],
            )
            .await
            .map_err(|e| format!("analyst read of granted schema: {e}"))?
            .get(0);
        if n != 1 {
            return Err(format!("granted schema should hold 1 row, saw {n}"));
        }

        // The ungranted schema must be denied — no USAGE was granted on it.
        if a.query_one(
            &format!("SELECT count(*) FROM {}.keys", secret.schema_name()),
            &[],
        )
        .await
        .is_ok()
        {
            return Err("analyst read a schema it was never granted".into());
        }
        Ok(())
    }
    .await;

    let deleted = p.delete_project(&project.id).await;
    outcome.expect("analyst end-to-end over TLS");
    deleted.expect("delete_project");
}

/// The **writer** path `ctx.oltp` actually takes — `PostgresConnector::from_dsn(
/// writer_dsn, true)` then `begin_transaction`, i.e. `tls_connector(true)`
/// against `SslMode::Require`.
///
/// This is the one path a testcontainer cannot cover: `postgres_tx_tests` speaks
/// `SslMode::Prefer` and the container declines TLS, so it never negotiates the
/// verifying handshake a managed tenant demands. Before the NoTls fix this
/// connect could not happen at all, and local (`sslmode=disable`) connected in
/// plaintext and hid it — so this test is what distinguishes "fixed" from
/// "differently broken", and it would also trip `from_dsn`'s debug-assert if a
/// DSN ever lost its sslmode. Assertions run after teardown so a red run still
/// deletes the billed project.
#[tokio::test]
async fn a_writer_reads_and_writes_its_own_schema_over_verified_tls() {
    use agentic_connector::{DatabaseConnector, PostgresConnector};

    let Some(p) = live() else {
        return;
    };
    let name = unique_name("writer");
    let project = create(&p, &name).await;

    let writer = WriterRef::app("bookings").expect("writer");
    // `role_name` directly, not through `qualify_role`: on Neon a project IS a
    // cluster, so `shares_role_namespace("neon")` is false and the qualified name
    // is the bare `app_bookings_rw` — same as production resolves here. The
    // hashed qualification only applies to the shared-cluster local provider.
    let writer_role = writer.role_name(schema::GrantLevel::ReadWrite);
    let writer_pw = roles::generate_password();

    let outcome: Result<(), String> = async {
        let owner_pw = project
            .owner_role
            .password
            .clone()
            .ok_or("owner password was not disclosed at create")?;
        let owner_dsn = project.dsn_for(&project.owner_role.name, &owner_pw);
        let owner = oxy_oltp::connect::connect(&owner_dsn, "neon writer test: owner")
            .await
            .map_err(|e| format!("owner connect: {e}"))?;

        // Mint the writer with DML on its own schema — the shipped SQL builders.
        let mut mint = roles::ensure_login_role_sql(&writer_role, &writer_pw)
            .map_err(|e| format!("build mint sql: {e}"))?;
        mint.push(
            roles::grant_connect_sql(&writer_role).map_err(|e| format!("build connect: {e}"))?,
        );
        mint.extend(
            schema::ensure_writer_sql(
                &writer,
                schema::GrantLevel::ReadWrite,
                &project.owner_role.name,
                &writer_role,
            )
            .map_err(|e| format!("build writer grants: {e}"))?,
        );
        for stmt in &mint {
            owner
                .batch_execute(stmt)
                .await
                .map_err(|e| format!("mint/grant `{stmt}`: {e}"))?;
        }

        // Connect AS the writer through the REAL ctx.oltp path: from_dsn parses
        // the search_path-bearing DSN, verify_tls=true asks for a verified
        // handshake, and begin_transaction is where the NoTls bug lived.
        let base = project.dsn_for(&writer_role, &writer_pw);
        let writer_dsn = schema::with_search_path(&base, &writer);
        let connector =
            PostgresConnector::from_dsn(&writer_dsn, true).map_err(|e| format!("from_dsn: {e}"))?;
        let mut tx = connector
            .begin_transaction()
            .await
            .map_err(|e| format!("begin_transaction over verified TLS: {e}"))?;

        // search_path pins app_bookings, so unqualified names land there.
        tx.exec(
            "CREATE TABLE bookings (id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, \
             name text NOT NULL)",
            &[],
        )
        .await
        .map_err(|e| format!("writer CREATE TABLE: {e}"))?;
        let rows = tx
            .query(
                "INSERT INTO bookings (name) VALUES ($1) RETURNING id",
                &[serde_json::json!("ada")],
            )
            .await
            .map_err(|e| format!("writer INSERT RETURNING: {e}"))?;
        if rows.len() != 1 {
            return Err(format!(
                "INSERT RETURNING should yield 1 row, saw {}",
                rows.len()
            ));
        }
        tx.commit()
            .await
            .map_err(|e| format!("writer commit: {e}"))?;

        // A fresh verified connection sees the committed row — proving the
        // transaction committed rather than silently rolling back.
        let verify = oxy_oltp::connect::connect(&writer_dsn, "neon writer test: verify")
            .await
            .map_err(|e| format!("verify connect: {e}"))?;
        let n: i64 = verify
            .query_one("SELECT count(*) FROM bookings", &[])
            .await
            .map_err(|e| format!("verify read: {e}"))?
            .get(0);
        if n != 1 {
            return Err(format!("committed table should hold 1 row, saw {n}"));
        }
        Ok(())
    }
    .await;

    let deleted = p.delete_project(&project.id).await;
    outcome.expect("writer end-to-end over verified TLS");
    deleted.expect("delete_project");
}

/// **The reason roles are minted in SQL rather than through the API.**
///
/// Every role Neon's API creates is a member of `neon_superuser`. A per-writer
/// role made that way would own the run of the database — every other writer's
/// schema and Oxy's own ledger — and the whole one-schema-per-writer model
/// would be decorative. So `OltpProvisioner` issues `CREATE ROLE` over SQL and
/// then runs [`roles::assert_confined_sql`], which raises `OXY01` if the role
/// turns out to hold membership nobody asked for.
///
/// **Only the live service can test the premise.** `grants.rs` covers the half
/// CI can reach — a stand-in group role plays `neon_superuser`, and the check
/// raises `OXY01` on the membership — so the detection is verified there. What
/// no local cluster can produce is the premise itself: that Neon's API still
/// grants `neon_superuser` to every role it creates. That is this test's job,
/// and the reason it exists alongside one that resembles it.
///
/// So both paths run here, and the API-created role is the control: without it
/// a green run would prove only that the check is satisfiable, not that it
/// detects the hazard.
#[tokio::test]
async fn a_sql_minted_role_is_confined_and_an_api_minted_one_is_not() {
    let Some(p) = live() else {
        return;
    };
    let name = unique_name("confine");
    let project = create(&p, &name).await;

    // Captured, not asserted, so a failure still deletes the project — including
    // a missing owner password, which captures here rather than panicking before
    // teardown.
    let outcome: Result<(Result<(), String>, Result<(), String>), String> = async {
        let password = project
            .owner_role
            .password
            .clone()
            .ok_or("owner password was not disclosed at create")?;
        let dsn = project.dsn_for(&project.owner_role.name, &password);
        let client = oxy_oltp::connect::connect(&dsn, "neon live confinement test")
            .await
            .map_err(|e| format!("connect: {e}"))?;

        // The shipped path: CREATE ROLE over SQL, then the check.
        let sql_role = format!("{}_sql_writer", name.replace('-', "_"));
        for stmt in roles::ensure_login_role_sql(&sql_role, &roles::generate_password())
            .map_err(|e| format!("build role sql: {e}"))?
        {
            client
                .batch_execute(&stmt)
                .await
                .map_err(|e| format!("create sql role: {e}"))?;
        }
        let confined = client
            .batch_execute(
                &roles::assert_confined_sql(&sql_role).map_err(|e| format!("build assert: {e}"))?,
            )
            .await
            .map_err(crate::render_pg_error);

        // The control: the same check against a role the API made.
        let api_role = format!("{}_api_writer", name.replace('-', "_"));
        p.create_role(&project.id, &project.branch.id, &api_role)
            .await
            .map_err(|e| format!("create api role: {e}"))?;
        let unconfined = client
            .batch_execute(
                &roles::assert_confined_sql(&api_role).map_err(|e| format!("build assert: {e}"))?,
            )
            .await
            .map_err(crate::render_pg_error);

        Ok((confined, unconfined))
    }
    .await;

    let deleted = p.delete_project(&project.id).await;

    let (confined, unconfined) = outcome.expect("the confinement probe ran");
    confined.expect("a SQL-minted role must pass the confinement check");
    let err = unconfined.expect_err(
        "an API-minted role must FAIL the check — if this passes, either Neon          stopped granting `neon_superuser` (in which case the SQL-minting          detour can go) or the check has stopped looking at group membership,          which is the failure it was written for",
    );
    assert!(
        err.contains("OXY01") || err.contains("not confined"),
        "the failure must be OUR confinement error, not an unrelated one: {err}"
    );
    deleted.expect("delete_project");
}

/// The orphan-adoption path, which exists precisely because Neon does not
/// enforce unique names — so it can only be verified here.
#[tokio::test]
async fn creating_the_same_name_twice_adopts_rather_than_duplicating() {
    let Some(p) = live() else {
        return;
    };
    let name = unique_name("adopt");
    let first = create(&p, &name).await;
    // Non-panicking: the second create is the call under test (Neon does NOT
    // enforce unique names — the whole reason this test exists), so it is the
    // one most likely to fail, and `first` is already live. A panic here would
    // leak it; capture and assert after teardown instead.
    let second = try_create(&p, &name).await;

    // Count what the org actually holds under this name — the assertion that
    // would catch a duplicate even if both calls returned the same id by luck.
    let listed = p.get_project(&first.id).await;
    let deleted_first = p.delete_project(&first.id).await;
    let deleted_second = match &second {
        Ok(s) if s.id != first.id => p.delete_project(&s.id).await,
        _ => Ok(()),
    };

    let second = second.expect("second create (adoption) against live Neon");
    assert_eq!(
        second.id, first.id,
        "the second create must adopt the first project, not make another"
    );
    assert!(
        second.owner_role.password.is_some(),
        "adoption must reset the owner password — the original is unrecoverable"
    );
    assert_ne!(
        second.owner_role.password, first.owner_role.password,
        "an adopted project's password must be freshly reset, not the original"
    );
    assert!(matches!(listed, Ok(Some(_))), "project should be readable");
    deleted_first.expect("delete first");
    deleted_second.expect("delete second");
}

/// The role lifecycle, including the two contracts a stub cannot confirm: that
/// Neon really does disclose a password on create, and that a plain read really
/// does not (whatever `store_passwords` is set to on this org).
#[tokio::test]
async fn a_role_discloses_its_password_once_and_a_reset_issues_a_new_one() {
    let Some(p) = live() else {
        return;
    };
    let name = unique_name("roles");
    let project = create(&p, &name).await;
    let branch = project.branch.id.clone();

    let outcome: Result<(), String> = async {
        let created = p
            .create_role(&project.id, &branch, "app_bookings_rw")
            .await
            .map_err(|e| format!("create_role: {e}"))?;
        let first_pw = created
            .password
            .clone()
            .ok_or("create_role disclosed no password")?;

        let read = p
            .get_role(&project.id, &branch, "app_bookings_rw")
            .await
            .map_err(|e| format!("get_role: {e}"))?
            .ok_or("get_role found nothing")?;
        if read.password.is_some() {
            return Err("a read must never carry a password out".into());
        }

        let reset = p
            .reset_role_password(&project.id, &branch, "app_bookings_rw")
            .await
            .map_err(|e| format!("reset: {e}"))?;
        let new_pw = reset
            .password
            .clone()
            .ok_or("reset disclosed no password")?;
        if new_pw == first_pw {
            return Err("reset returned the same password".into());
        }

        p.delete_role(&project.id, &branch, "app_bookings_rw")
            .await
            .map_err(|e| format!("delete_role: {e}"))?;
        // Idempotent by contract: deleting an absent role is Ok(()).
        p.delete_role(&project.id, &branch, "app_bookings_rw")
            .await
            .map_err(|e| format!("second delete_role should be idempotent: {e}"))?;
        Ok(())
    }
    .await;

    let deleted = p.delete_project(&project.id).await;
    outcome.expect("role lifecycle");
    deleted.expect("delete_project");
}

/// `get_*` reports absence as `Ok(None)` and `delete_*` is idempotent — the
/// contract the whole reconcile-don't-duplicate design rests on.
#[tokio::test]
async fn absence_is_not_an_error() {
    let Some(p) = live() else {
        return;
    };
    assert!(
        p.get_project("does-not-exist-at-all-12345")
            .await
            .expect("a missing project is Ok(None), not an error")
            .is_none()
    );
    p.delete_project("does-not-exist-at-all-12345")
        .await
        .expect("deleting an absent project is idempotent");
}
