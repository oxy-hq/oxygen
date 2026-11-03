//! The environment namespace, and reads pinned to a snapshot.

use super::*;
use crate::sql_parse::on_sql_stack;

const SIB: &str = "app_w__dev_a1";
/// 2026-11-01T09:00:00Z.
const PIN_MICROS: i64 = 1_793_523_600_000_000;
const AT: &str = "AT(TIMESTAMP => TIMESTAMP WITH TIME ZONE '2026-11-01 09:00:00.000000+00')";

fn env() -> PreviewNamespace {
    PreviewNamespace::for_environment("app_w", SIB).expect("a valid sibling")
}

fn opts(pinned: bool) -> RewriteOptions {
    RewriteOptions {
        catalog: Some("lake".into()),
        read_live_only: false,
        pin: pinned.then(|| SnapshotPin::from_unix_micros(PIN_MICROS).unwrap()),
    }
}

fn shadowed(tables: &[(&str, &str)]) -> ShadowMap {
    let mut map = ShadowMap::default();
    for (schema, table) in tables {
        map.0
            .insert((schema.to_string(), table.to_string()), ShadowState::Shadow);
    }
    map
}

fn read(sql: &str, shadow: &ShadowMap, opts: &RewriteOptions) -> String {
    overlay_reads(sql, &env(), shadow, opts).unwrap_or_else(|e| panic!("{sql:?}: {e}"))
}

fn write(sql: &str, shadow: &ShadowMap, opts: &RewriteOptions) -> Rewrite {
    rewrite(sql, &env(), shadow, opts).unwrap_or_else(|e| panic!("{sql:?}: {e}"))
}

#[test]
fn an_environment_namespace_needs_a_sibling_of_the_apps_own_schema() {
    for (app, sibling) in [
        ("app_w", "app_w"),
        ("app_w", "app_w__"),
        ("app_w", "app_x__dev_a1"),
        ("app_w", "app_w_dev_a1"),
        ("app_w", "app_w__Dev_a1"),
        ("app_w", "app_w__dev__a1"),
        ("app_w", "app_w__dev-a1"),
        ("app_w", "app_w__dev_a1_"),
        ("App_w", "App_w__dev_a1"),
        ("", "__dev_a1"),
        ("toast_pos", "toast_pos__dev_a1"),
    ] {
        assert!(
            PreviewNamespace::for_environment(app, sibling).is_err(),
            "{app} / {sibling}"
        );
    }
    assert!(PreviewNamespace::for_environment("app_w", "app_w__staging").is_ok());
}

#[test]
fn an_environment_maps_the_apps_schema_to_its_sibling_and_nothing_else() {
    let ns = env();
    assert_eq!(ns.schema_for("app_w").unwrap(), SIB);
    assert_eq!(ns.schema_for("APP_W").unwrap(), SIB);
    for other in ["toast_pos", "app_x", SIB, "main"] {
        let Refused(why) = ns.schema_for(other).unwrap_err();
        assert!(
            why.contains("stands in for the app's own schema only"),
            "{other}: {why}"
        );
    }
    assert!(ns.owns_schema(SIB));
    assert!(ns.owns_schema(&SIB.to_uppercase()));
    for other in ["app_w", "toast_pos", "app_w__dev_a2", "app_w__dev_a1x"] {
        assert!(!ns.owns_schema(other), "{other}");
    }
    // The preview-only accessors name the sibling and are never a prefix.
    assert_eq!((ns.key(), ns.prefix().as_str()), (SIB, SIB));
    assert_ne!(
        ns,
        PreviewNamespace::for_environment("app_w", "app_w__dev_a2").unwrap()
    );
}

#[test]
fn a_write_to_the_apps_schema_lands_in_the_sibling() {
    let out = write(
        "INSERT INTO app_w.t SELECT 1",
        &ShadowMap::default(),
        &opts(false),
    );
    assert_eq!(out.sql, format!("INSERT INTO \"{SIB}\".\"t\" SELECT 1"));
    assert_eq!(out.writes, vec![("app_w".to_string(), "t".to_string())]);
    let out = write(
        "DELETE FROM lake.app_w.t WHERE id = 1",
        &shadowed(&[("app_w", "t")]),
        &opts(false),
    );
    assert_eq!(
        out.sql,
        format!("DELETE FROM lake.\"{SIB}\".\"t\" WHERE id = 1")
    );
}

#[test]
fn a_write_to_any_other_schema_is_refused() {
    for sql in [
        "INSERT INTO toast_pos.x SELECT 1",
        "UPDATE toast_pos.x SET a = 1",
        "DELETE FROM app_x.t",
        "CREATE TABLE toast_pos.x AS SELECT 1",
        "DROP TABLE toast_pos.x",
        "TRUNCATE app_x.t",
    ] {
        let err = rewrite(sql, &env(), &ShadowMap::default(), &opts(false)).unwrap_err();
        assert!(
            err.0.contains("stands in for the app's own schema only"),
            "{sql}: {}",
            err.0
        );
    }
}

/// A sibling already has the table its migrations made, so a `TRUNCATE` of
/// one the overlay does not hold yet empties it in place, never replaces it
/// (which would drop a newer migrated column), and the overlay then holds it.
#[test]
fn a_truncate_of_an_unheld_table_empties_the_siblings_in_place() {
    let out = write("TRUNCATE app_w.t", &ShadowMap::default(), &opts(false));
    assert_eq!(out.sql, format!("TRUNCATE \"{SIB}\".\"t\""));
    assert!(!out.sql.contains("CREATE"), "{}", out.sql);
    let mut after = ShadowMap::default();
    after.apply_rewrite(&out, &[]);
    assert_eq!(
        after.state(&("app_w".to_string(), "t".to_string())),
        Some(ShadowState::Shadow)
    );
}

#[test]
fn reads_go_to_the_sibling_only_for_the_tables_it_holds() {
    let shadow = shadowed(&[("app_w", "orders")]);
    let sql = "SELECT * FROM app_w.orders o JOIN app_w.items i ON i.id = o.id \
               JOIN toast_pos.checks c ON c.id = o.id JOIN main_t m ON true";
    assert_eq!(
        read(sql, &shadow, &opts(false)),
        format!(
            "SELECT * FROM \"{SIB}\".\"orders\" o JOIN app_w.items i ON i.id = o.id \
             JOIN toast_pos.checks c ON c.id = o.id JOIN main_t m ON true"
        )
    );
    // With nothing shadowed, nothing moves, and a second pass changes nothing.
    let live = read(
        "SELECT * FROM app_w.orders",
        &ShadowMap::default(),
        &opts(false),
    );
    assert_eq!(live, "SELECT * FROM app_w.orders");
}

#[test]
fn a_pinned_live_read_renders_the_snapshot_clause() {
    let pinned = read(
        "SELECT * FROM app_w.items",
        &ShadowMap::default(),
        &opts(true),
    );
    assert_eq!(pinned, format!("SELECT * FROM app_w.items {AT}"));
    // Qualified by the workspace catalog, aliased, and unqualified (`main.t`).
    assert_eq!(
        read(
            "SELECT * FROM lake.toast_pos.checks AS c",
            &ShadowMap::default(),
            &opts(true)
        ),
        format!("SELECT * FROM lake.toast_pos.checks AS c {AT}")
    );
    assert_eq!(
        read("SELECT * FROM t", &ShadowMap::default(), &opts(true)),
        format!("SELECT * FROM t {AT}")
    );
}

#[test]
fn only_a_read_that_stays_live_is_pinned() {
    let shadow = shadowed(&[("app_w", "orders")]);
    let sql = "WITH cte AS (SELECT * FROM app_w.items) \
               SELECT * FROM cte, app_w.orders, information_schema.tables, pg_catalog.pg_class, \
               range(3) r, (SELECT 1) s, toast_pos.checks";
    assert_eq!(
        read(sql, &shadow, &opts(true)),
        format!(
            "WITH cte AS (SELECT * FROM app_w.items {AT}) SELECT * FROM cte, \"{SIB}\".\"orders\", \
             information_schema.tables, pg_catalog.pg_class, range(3) r, (SELECT 1) s, \
             toast_pos.checks {AT}"
        )
    );
}

#[test]
fn read_live_only_still_pins_what_it_reads() {
    let mut live_only = opts(true);
    live_only.read_live_only = true;
    let shadow = shadowed(&[("app_w", "orders")]);
    assert_eq!(
        read("SELECT * FROM app_w.orders", &shadow, &live_only),
        format!("SELECT * FROM app_w.orders {AT}")
    );
}

/// Applying the overlay to a tree it already pinned changes nothing. (Not
/// through text: the DuckDB dialect cannot parse `AT (…)`, so the pinned SQL
/// is what is sent, never parsed again.)
#[test]
fn applying_the_overlay_to_a_pinned_tree_again_changes_nothing() {
    let shadow = shadowed(&[("app_w", "orders")]);
    let opts = opts(true);
    let (once, twice) = on_sql_stack(
        "SELECT * FROM app_w.orders JOIN app_w.items USING (id)",
        "a test",
        |mut statements| {
            let statement = &mut statements[0];
            overlay::apply(&env(), &shadow, &opts, statement)?;
            let once = statement.to_string();
            overlay::apply(&env(), &shadow, &opts, statement)?;
            Ok((once, statement.to_string()))
        },
    )
    .unwrap();
    assert_eq!(once, twice);
    assert_eq!(once.matches("AT(").count(), 1, "{once}");
}

#[test]
fn a_write_pins_the_reads_it_makes_and_never_its_target() {
    let shadow = shadowed(&[("app_w", "t")]);
    for (sql, expected) in [
        (
            "INSERT INTO app_w.t SELECT * FROM app_w.u",
            format!("INSERT INTO \"{SIB}\".\"t\" SELECT * FROM app_w.u {AT}"),
        ),
        (
            "DELETE FROM app_w.t WHERE id IN (SELECT id FROM toast_pos.x)",
            format!("DELETE FROM \"{SIB}\".\"t\" WHERE id IN (SELECT id FROM toast_pos.x {AT})"),
        ),
        (
            "UPDATE app_w.t SET v = 1 FROM app_w.u WHERE t.id = u.id",
            format!("UPDATE \"{SIB}\".\"t\" SET v = 1 FROM app_w.u {AT} WHERE t.id = u.id"),
        ),
        (
            "UPDATE app_w.t SET v = 1",
            format!("UPDATE \"{SIB}\".\"t\" SET v = 1"),
        ),
        (
            "CREATE TABLE app_w.n AS SELECT * FROM app_w.t",
            format!("CREATE TABLE \"{SIB}\".\"n\" AS SELECT * FROM \"{SIB}\".\"t\""),
        ),
    ] {
        let out = write(sql, &shadow, &opts(true));
        assert_eq!(out.sql, expected, "{sql}");
    }
}

#[test]
fn a_preview_namespace_pins_live_reads_the_same_way() {
    let preview = PreviewNamespace::from_key("feat_000000").unwrap();
    let out = overlay_reads(
        "SELECT * FROM toast_pos.checks",
        &preview,
        &ShadowMap::default(),
        &opts(true),
    )
    .unwrap();
    assert_eq!(out, format!("SELECT * FROM toast_pos.checks {AT}"));
}

/// The pin is the last thing done to a statement: the DuckDB dialect cannot
/// parse `AT (…)`, so a host checks the SQL first and pins as it sends.
#[test]
fn pinned_sql_is_not_parsed_again() {
    let pinned = read(
        "SELECT * FROM app_w.items",
        &ShadowMap::default(),
        &opts(true),
    );
    let err = overlay_reads(&pinned, &env(), &ShadowMap::default(), &opts(true)).unwrap_err();
    assert!(err.0.contains("could not parse"), "{}", err.0);
}
