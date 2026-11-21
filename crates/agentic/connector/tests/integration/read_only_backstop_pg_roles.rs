//! The backstop's login-role check on a real Postgres server (see
//! `read_only_backstop_pg`): a role that can write a server file, or switch
//! to one that can, is refused before its statement runs.

use agentic_connector::{DatabaseConnector, ExecutionResult};
use agentic_core::result::CellValue;

use super::postgres::Fixture;

/// A role that can write a server file is refused before its statement
/// runs: a read-only transaction does not stop `lo_export`, which changes no
/// database state. Superuser, `pg_write_server_files`, and an `EXECUTE`
/// grant on `lo_export` each count.
#[tokio::test]
async fn a_role_that_can_write_server_files_is_refused() {
    let Some(f) = Fixture::new("lo").await else {
        return;
    };
    let s = f.s.clone();
    let CellValue::Number(oid) = f.scalar("SELECT lo_create(0)").await else {
        panic!("lo_create returns an oid");
    };
    let export = format!("SELECT lo_export({oid}, '/tmp/{s}.lo')::text");
    let refused = |who: &str, got: Result<ExecutionResult, _>| {
        let err: agentic_connector::ConnectorError =
            got.expect_err(&format!("{who} must be refused"));
        assert!(
            err.to_string().contains("least-privilege role"),
            "{who}: {err}"
        );
    };

    refused(
        "a superuser",
        f.admin.execute_query_read_only(&export, 1).await,
    );
    let grants = [
        format!("GRANT pg_write_server_files TO {s}_agent"),
        format!("REVOKE pg_write_server_files FROM {s}_agent"),
        format!("GRANT EXECUTE ON FUNCTION lo_export(oid, text) TO {s}_agent"),
    ];
    f.admin.execute_statement(&grants[0]).await.unwrap();
    refused(
        "pg_write_server_files",
        f.agent().execute_query_read_only(&export, 1).await,
    );
    f.admin.execute_statement(&grants[1]).await.unwrap();
    f.admin.execute_statement(&grants[2]).await.unwrap();
    refused(
        "lo_export",
        f.agent().execute_query_read_only(&export, 1).await,
    );

    let listed = format!("SELECT count(*) FROM pg_ls_dir('/tmp') AS f WHERE f = '{s}.lo'");
    assert_eq!(
        f.scalar(&listed).await,
        CellValue::Number(0.0),
        "lo_export wrote a file"
    );
    f.admin
        .execute_query(&format!("SELECT lo_unlink({oid})::text"), 1)
        .await
        .unwrap();
    f.cleanup().await;
}

/// The statement can switch role mid-transaction (`set_config('role', …,
/// true)`) to any role the login role is a member of, then run SQL built at
/// run time as that role (`query_to_xml`). So a login role that can reach a
/// file-writing role is refused too: a superuser one, and one holding
/// `EXECUTE` on `lo_export` through a grant that does not inherit.

#[tokio::test]
async fn a_role_that_can_switch_to_a_file_writing_role_is_refused() {
    let Some(f) = Fixture::new("sw").await else {
        return;
    };
    let s = f.s.clone();
    for stmt in [
        format!("CREATE ROLE {s}_su SUPERUSER NOLOGIN"),
        format!("GRANT {s}_su TO {s}_agent"),
        format!("CREATE ROLE {s}_lo NOLOGIN"),
        format!("GRANT EXECUTE ON FUNCTION lo_export(oid, text) TO {s}_lo"),
    ] {
        f.admin.execute_statement(&stmt).await.expect(&stmt);
    }
    let CellValue::Number(oid) = f.scalar("SELECT lo_create(0)").await else {
        panic!("lo_create returns an oid");
    };
    let switch = |role: &str, file: &str| {
        format!(
            "SELECT set_config('role', '{role}', true) AS r, query_to_xml(\
             'SELECT lo_export({oid}, ''/tmp/{file}'')::text AS x', false, false, '')::text AS x"
        )
    };
    let listed =
        |file: &str| format!("SELECT count(*) FROM pg_ls_dir('/tmp') AS f WHERE f = '{file}'");

    let su = switch(&format!("{s}_su"), &format!("{s}.su"));
    let got = f.agent().execute_query_read_only(&su, 1).await;
    let written = f.scalar(&listed(&format!("{s}.su"))).await;
    assert_eq!(
        written,
        CellValue::Number(0.0),
        "switching to a superuser wrote a file"
    );
    let err = got.expect_err("a login role that reaches a superuser must be refused");
    assert!(err.to_string().contains("least-privilege role"), "{err}");

    f.admin
        .execute_statement(&format!("REVOKE {s}_su FROM {s}_agent"))
        .await
        .unwrap();
    f.admin
        .execute_statement(&format!("GRANT {s}_lo TO {s}_agent WITH INHERIT FALSE"))
        .await
        .unwrap();
    let lo = switch(&format!("{s}_lo"), &format!("{s}.lo"));
    let got = f.agent().execute_query_read_only(&lo, 1).await;
    let written = f.scalar(&listed(&format!("{s}.lo"))).await;
    assert_eq!(
        written,
        CellValue::Number(0.0),
        "switching to lo_export's grantee wrote a file"
    );
    let err = got.expect_err("a login role that reaches lo_export must be refused");
    assert!(err.to_string().contains("least-privilege role"), "{err}");

    for stmt in [
        format!("SELECT lo_unlink({oid})::text"),
        format!("DROP OWNED BY {s}_lo"),
        format!("DROP ROLE {s}_lo"),
        format!("DROP ROLE {s}_su"),
    ] {
        f.admin.execute_query(&stmt, 1).await.expect(&stmt);
    }
    f.cleanup().await;
}

/// The readiness probe runs the same check once, with no statement: a
/// superuser login refuses before any query, a least-privilege one passes,
/// and the connector still reads after either.
#[tokio::test]
async fn read_only_ready_refuses_a_superuser_and_passes_a_least_privilege_role() {
    let Some(f) = Fixture::new("ready").await else {
        return;
    };
    let err = f
        .admin
        .read_only_ready()
        .await
        .expect_err("a superuser login must be refused at readiness");
    assert!(err.to_string().contains("least-privilege role"), "{err}");

    let agent = f.agent();
    agent
        .read_only_ready()
        .await
        .expect("a least-privilege role is ready (no adminpack is not a failure)");
    let count = format!("SELECT count(*) FROM {}.t", f.s);
    let got = agent.execute_query_read_only(&count, 1).await.unwrap();
    assert_eq!(got.result.rows[0].0[0], CellValue::Number(3.0));
    f.cleanup().await;
}

/// `pg_read_server_files` (read any server file) and
/// `pg_execute_server_program` (`COPY … FROM PROGRAM`) are refused too, held
/// directly or reached through a membership that does not inherit.
#[tokio::test]
async fn a_role_that_can_read_server_files_or_run_programs_is_refused() {
    let Some(f) = Fixture::new("rp").await else {
        return;
    };
    let s = f.s.clone();
    let refused = |who: &str, got: Result<(), agentic_connector::ConnectorError>| {
        let err = got.expect_err(&format!("{who} must be refused"));
        assert!(
            err.to_string().contains("least-privilege role"),
            "{who}: {err}"
        );
    };
    for role in ["pg_read_server_files", "pg_execute_server_program"] {
        let grant = format!("GRANT {role} TO {s}_agent");
        f.admin.execute_statement(&grant).await.expect(&grant);
        refused(role, f.agent().read_only_ready().await);
        let query = f.agent().execute_query_read_only("SELECT 1 AS x", 1).await;
        refused(role, query.map(|_| ()));
        let revoke = format!("REVOKE {role} FROM {s}_agent");
        f.admin.execute_statement(&revoke).await.expect(&revoke);

        // Through a role the agent can switch to but does not inherit from.
        let via = format!("{s}_via");
        for stmt in [
            format!("CREATE ROLE {via} NOLOGIN"),
            format!("GRANT {role} TO {via}"),
            format!("GRANT {via} TO {s}_agent WITH INHERIT FALSE"),
        ] {
            f.admin.execute_statement(&stmt).await.expect(&stmt);
        }
        refused(
            &format!("{role} via {via}"),
            f.agent().read_only_ready().await,
        );
        for stmt in [
            format!("REVOKE {via} FROM {s}_agent"),
            format!("DROP ROLE {via}"),
        ] {
            f.admin.execute_statement(&stmt).await.expect(&stmt);
        }
        f.agent()
            .read_only_ready()
            .await
            .expect("ready again once the grant is gone");
    }
    f.cleanup().await;
}

/// `EXECUTE` granted on a server-file function directly, with no membership
/// in the role that holds it by default, is refused too (here
/// `pg_read_file(text)`); revoked, the role is ready again.
#[tokio::test]
async fn a_role_granted_a_server_file_function_directly_is_refused() {
    let Some(f) = Fixture::new("rf").await else {
        return;
    };
    let s = f.s.clone();
    let grant = format!("GRANT EXECUTE ON FUNCTION pg_read_file(text) TO {s}_agent");
    f.admin.execute_statement(&grant).await.expect(&grant);
    let err = f
        .agent()
        .read_only_ready()
        .await
        .expect_err("EXECUTE on pg_read_file must be refused");
    assert!(err.to_string().contains("least-privilege role"), "{err}");
    let read = f
        .agent()
        .execute_query_read_only("SELECT pg_read_file('PG_VERSION') AS v", 1)
        .await
        .expect_err("refused before the statement runs");
    assert!(read.to_string().contains("least-privilege role"), "{read}");

    let revoke = format!("REVOKE EXECUTE ON FUNCTION pg_read_file(text) FROM {s}_agent");
    f.admin.execute_statement(&revoke).await.expect(&revoke);
    f.agent()
        .read_only_ready()
        .await
        .expect("ready again once the grant is gone");
    f.cleanup().await;
}
