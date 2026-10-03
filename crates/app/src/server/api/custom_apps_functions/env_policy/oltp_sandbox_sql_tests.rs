use super::*;

fn fence() -> SandboxFence {
    SandboxFence::new("app_store", "app_store__dev_a1")
}

fn admit(sql: &str) -> Result<(), HeldStatement> {
    admit_sandbox_statement(&fence(), sql)
}

fn refusal(sql: &str) -> HeldStatement {
    admit(sql).expect_err(sql)
}

/// What an app writes: unqualified names, which the search path lands in the
/// sandbox's schema.
#[test]
fn unqualified_statements_are_sent() {
    for sql in [
        "select count(*)::int as n from orders",
        "insert into orders (id, note) values ($1, $2) returning id",
        "update orders o set note = 'x' from customers c where c.id = o.customer_id",
        "delete from orders where id = 3",
        "with moved as (delete from orders returning *) insert into archive select * from moved",
        "create table notes (id int primary key, body text)",
        "alter table orders add column tier text",
        "create index orders_note_idx on orders (note)",
        "truncate orders",
        "select * from generate_series(1, 3) as g",
        "select nextval('orders_id_seq')",
        "select setval('orders_id_seq', 10, true)",
        "select 'orders'::regclass::oid",
        "select set_config('app.user_id', '7', true)",
        "set local statement_timeout = '5s'",
        "SAVEPOINT s",
        "select 1; select 2",
    ] {
        assert_eq!(admit(sql), Ok(()), "{sql}");
    }
}

/// The sandbox's own schema, and the system catalogs, may be named.
#[test]
fn the_sandboxs_own_schema_and_the_catalogs_may_be_named() {
    for sql in [
        "select * from app_store__dev_a1.orders",
        r#"select * from "app_store__dev_a1"."orders""#,
        "select * from APP_STORE__DEV_A1.orders",
        "select table_name from information_schema.tables",
        "select relname from pg_catalog.pg_class",
        "select nextval('app_store__dev_a1.orders_id_seq')",
    ] {
        assert_eq!(admit(sql), Ok(()), "{sql}");
    }
}

/// Staging's schema and another sandbox's are the ones the role could reach:
/// refused however they are spelled, wherever they appear.
#[test]
fn stagings_schema_and_another_sandboxs_are_refused_however_spelled() {
    for sql in [
        "select * from app_store.orders",
        "insert into app_store.orders (id) values (1)",
        "select * from APP_STORE.orders",
        r#"select * from "app_store"."orders""#,
        "select * from app_store__dev_b2.orders",
        "delete from app_store__staging.orders",
        "select app_store.bump()",
        "select 'open'::app_store.order_status",
        "alter table orders set schema app_store",
        "create index on app_store.orders (id)",
        "drop table app_store.orders",
        "select * from orders o join app_store.customers c on c.id = o.customer_id",
        "with x as (select * from app_store.orders) select * from x",
        "select (select count(*) from app_store.orders)",
        "grant select on all tables in schema app_store to public",
        "comment on table app_store.orders is 'x'",
    ] {
        let refused = refusal(sql);
        assert!(
            refused.why.contains("not this sandbox's own"),
            "{sql}: {refused:?}"
        );
    }
}

/// Postgres resolves a name inside a string when the statement runs, so a
/// string that spells one of those schemas is read too.
#[test]
fn a_string_that_spells_another_schema_of_the_app_is_refused() {
    for sql in [
        "select nextval('app_store.orders_id_seq')",
        "select setval('app_store.orders_id_seq', 1)",
        "select 'app_store.orders'::regclass",
        r#"select '"app_store"."orders"'::regclass"#,
        "select 'APP_STORE.orders'::regclass",
        "select table_to_xml('app_store.orders', true, false, '')",
        "select pg_relation_size('app_store__dev_b2.orders')",
        "select $$app_store.orders$$::regclass",
        "select E'app_store.orders'::regclass",
    ] {
        let refused = refusal(sql);
        assert!(
            refused.why.contains("names schema app_store"),
            "{sql}: {refused:?}"
        );
    }
    // Another app's name, and text that only contains the letters, are not it.
    for sql in [
        "select 'app_storefront.orders' as note",
        "select 'myapp_store' as note",
        "insert into notes (body) values ('see app_store__dev_a1.orders')",
    ] {
        assert_eq!(admit(sql), Ok(()), "{sql}");
    }
}

/// Any other schema is refused where a relation names it — the role holds no
/// grant there, and this says so before Postgres does.
#[test]
fn a_relation_in_any_other_schema_is_refused() {
    for sql in [
        "select * from public.orders",
        "select * from app_other.orders",
        "insert into raw_toast.orders (id) values (1)",
        "select * from oxy_meta.migrations",
        "select * from otherdb.app_store__dev_a1.orders",
    ] {
        let refused = refusal(sql);
        assert_eq!(refused.verb, "NAME", "{sql}: {refused:?}");
    }
}

/// The search path is what lands an unqualified name in the sandbox, so
/// nothing may change it — or the role names resolve as.
#[test]
fn changing_how_names_resolve_is_refused() {
    for sql in [
        "set search_path = app_other",
        "SET search_path TO public",
        "set local search_path = public",
        "set session search_path = public",
        r#"set "search_path" to public"#,
        "set schema 'public'",
        "set role postgres",
        "set local role postgres",
        "set session authorization postgres",
        "reset search_path",
        "reset all",
        "reset role",
        "reset session authorization",
        "discard all",
        "select 1; set search_path = public",
        "select set_config('search_path', 'public', false)",
        "select pg_catalog.set_config('SEARCH_PATH', 'public', true)",
        "select set_config('role', 'postgres', false)",
        "select set_config('search' || '_path', 'public', false)",
        "select set_config(name, 'public', false) from settings",
    ] {
        let refused = refusal(sql);
        assert!(
            refused.why.contains("how names resolve"),
            "{sql}: {refused:?}"
        );
    }
    // A column called `role`, or a setting that resolves no name, is neither.
    for sql in [
        "update members set role = 'admin' where id = 1",
        "set statement_timeout = 1000",
        "set local lock_timeout = '2s'",
        "reset statement_timeout",
        "set transaction isolation level serializable",
    ] {
        assert_eq!(admit(sql), Ok(()), "{sql}");
    }
}

/// A name assembled when the statement runs is not in any token to read.
#[test]
fn a_name_resolved_at_run_time_must_be_a_literal() {
    for sql in [
        "select nextval('app_' || 'store.orders_id_seq')",
        "select setval(concat('app_', 'store', '.orders_id_seq'), 1)",
        "select currval(seq_name) from sequences",
        "select ('app_' || 'store.orders')::regclass",
        "select cast('app_' || 'store.orders' as regclass)",
        "select (x)::pg_catalog.regclass from names",
        "select to_regclass('app_' || 'store.orders')",
        "select table_to_xml(tbl, true, false, '') from names",
        "select schema_to_xml(format('%s', 'app_store'), true, false, '')",
        "select database_to_xml(true, false, '')",
        "select database_to_xml_and_xmlschema(true, false, '')",
    ] {
        let refused = refusal(sql);
        assert!(
            matches!(refused.verb.as_str(), "CALL" | "CAST" | "NAME"),
            "{sql}: {refused:?}"
        );
    }
    // A column or an alias that only looks like one is not a cast.
    for sql in [
        "select region as region from stores",
        "select id as registered from members",
    ] {
        assert_eq!(admit(sql), Ok(()), "{sql}");
    }
}

/// What cannot be read cannot be checked.
#[test]
fn sql_that_does_not_parse_is_refused() {
    for sql in ["selec 1 frm", "", "select * from", "select 'unterminated"] {
        let refused = refusal(sql);
        assert_eq!(refused.verb, "UNCLASSIFIED", "{sql}: {refused:?}");
        assert!(refused.why.contains("does not parse"), "{sql}: {refused:?}");
    }
}

#[test]
fn the_refusal_names_the_environment_and_how_to_write_the_statement() {
    let environment = AppEnvironment::Dev {
        handle: "a1".into(),
    };
    let message = sandbox_statement_message(HostOp::OltpQuery, &environment, "names schema x");
    assert!(
        message.starts_with("EnvironmentRefused: ctx.oltp.query"),
        "{message}"
    );
    assert!(message.contains("dev-a1"), "{message}");
    assert!(
        message.contains("Write table names unqualified"),
        "{message}"
    );
    assert!(
        message.ends_with("This statement names schema x."),
        "{message}"
    );
}
