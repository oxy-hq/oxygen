use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use airhouse::preview_sql::PreviewNamespace;

use super::*;
use crate::server::previews::ddl::{PreviewDdlError, Relation, SchemaDropper};
use crate::server::previews::ddl_duckdb::DuckDbPreviewDdl;

const KEY: &str = "feat_je_v2_92a1b7";
const OTHER_KEY: &str = "feat_je_v3_000000";

fn duck(sql: &str) -> Arc<Mutex<duckdb::Connection>> {
    let conn = duckdb::Connection::open_in_memory().unwrap();
    conn.execute_batch(sql).unwrap();
    Arc::new(Mutex::new(conn))
}

fn schemas(conn: &Mutex<duckdb::Connection>) -> BTreeSet<String> {
    let conn = conn.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT schema_name FROM information_schema.schemata")
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn relations(conn: &Mutex<duckdb::Connection>, schema: &str) -> BTreeSet<String> {
    let conn = conn.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT table_name FROM information_schema.tables WHERE table_schema = ?")
        .unwrap();
    stmt.query_map([schema], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn claimed(live: &str, created: bool, recorded: &[&str]) -> ClaimedSchema {
    ClaimedSchema {
        live_schema: live.into(),
        created,
        recorded: recorded.iter().map(|s| s.to_string()).collect(),
    }
}

/// Only a name that is both registered (the caller's locked registry read)
/// and well formed for this preview is dropped — and it is dropped whole,
/// views and tables first, without `CASCADE`. Everything else asked for,
/// including names crafted to look like this preview's, is refused and left
/// exactly as it was.
#[tokio::test]
async fn only_registered_well_formed_names_are_dropped() {
    let ours = format!("preview_{KEY}__toast_pos");
    let unregistered = format!("preview_{KEY}__notes");
    let another_preview = format!("preview_{OTHER_KEY}__toast_pos");
    let uppercase = format!("PREVIEW_{KEY}__TOAST_POS");
    let nested = format!("preview_{KEY}__a__b");
    let conn = duck(&format!(
        "CREATE SCHEMA \"{ours}\"; \
         CREATE TABLE \"{ours}\".orders (id INT); \
         CREATE VIEW \"{ours}\".recent AS SELECT * FROM \"{ours}\".orders; \
         CREATE SCHEMA preview_notes; CREATE TABLE preview_notes.t (id INT); \
         CREATE SCHEMA \"{unregistered}\"; CREATE TABLE \"{unregistered}\".t (id INT); \
         CREATE SCHEMA \"{another_preview}\"; \
         CREATE SCHEMA \"{nested}\"; \
         CREATE SCHEMA toast_pos; CREATE TABLE toast_pos.orders (id INT);"
    ));
    let before = schemas(&conn);
    let ns = PreviewNamespace::from_key(KEY).unwrap();
    let ddl = DuckDbPreviewDdl::new(conn.clone(), ns.clone());
    // The registry vouches for every name but `unregistered` (each created,
    // each recording the same relations); only `ours` is also well formed.
    let registered: HashMap<String, ClaimedSchema> = [
        &ours,
        &another_preview,
        &uppercase,
        &nested,
        &"preview_notes".to_string(),
        &"toast_pos".to_string(),
    ]
    .into_iter()
    .map(|name| {
        let schema = claimed("toast_pos", true, &["orders", "recent", "t"]);
        (name.clone(), schema)
    })
    .collect();
    let requested: Vec<String> = registered
        .keys()
        .cloned()
        .chain([unregistered.clone(), ours.clone()])
        .collect();

    let report = drop_listed(&ddl, &ns, &registered, &requested).await;

    assert_eq!(report.dropped, vec![(ours.clone(), 2)], "{report:?}");
    assert!(report.failed.is_empty(), "{report:?}");
    let refused: BTreeSet<&str> = report.refused.iter().map(|(n, _)| n.as_str()).collect();
    let expected: BTreeSet<&str> = [
        unregistered.as_str(),
        another_preview.as_str(),
        uppercase.as_str(),
        nested.as_str(),
        "preview_notes",
        "toast_pos",
    ]
    .into_iter()
    .collect();
    assert_eq!(refused, expected);
    let mut after_expected = before.clone();
    after_expected.remove(&ours);
    assert_eq!(schemas(&conn), after_expected);
}

/// The drop drops only what the preview recorded. A schema that also holds a
/// relation the preview did not record keeps it — and is itself kept.
#[tokio::test]
async fn only_recorded_relations_are_dropped_and_a_schema_holding_others_is_kept() {
    let ours = format!("preview_{KEY}__toast_pos");
    let conn = duck(&format!(
        "CREATE SCHEMA \"{ours}\"; \
         CREATE TABLE \"{ours}\".orders (id INT); \
         CREATE VIEW \"{ours}\".recent AS SELECT * FROM \"{ours}\".orders; \
         CREATE TABLE \"{ours}\".customer_notes (id INT);"
    ));
    let ns = PreviewNamespace::from_key(KEY).unwrap();
    let ddl = DuckDbPreviewDdl::new(conn.clone(), ns.clone());
    let registered: HashMap<String, ClaimedSchema> = [(
        ours.clone(),
        claimed("toast_pos", true, &["orders", "recent"]),
    )]
    .into();

    let report = drop_listed(&ddl, &ns, &registered, std::slice::from_ref(&ours)).await;

    assert!(report.dropped.is_empty(), "{report:?}");
    assert_eq!(
        report.orphaned,
        vec![(ours.clone(), 2, vec!["customer_notes".to_string()])]
    );
    assert!(schemas(&conn).contains(&ours));
    assert_eq!(
        relations(&conn, &ours),
        ["customer_notes".to_string()].into()
    );
}

/// A claimed row whose schema the preview never created is closed with no
/// DDL at all, even when a schema of that name exists (and even when it is
/// empty, which a `DROP SCHEMA` would happily take).
#[tokio::test]
async fn a_schema_the_preview_never_created_is_never_touched() {
    let theirs = format!("preview_{KEY}__marketing");
    let conn = duck(&format!("CREATE SCHEMA \"{theirs}\";"));
    let ns = PreviewNamespace::from_key(KEY).unwrap();
    let ddl = DuckDbPreviewDdl::new(conn.clone(), ns.clone());
    let registered: HashMap<String, ClaimedSchema> =
        [(theirs.clone(), claimed("marketing", false, &[]))].into();

    let report = drop_listed(&ddl, &ns, &registered, std::slice::from_ref(&theirs)).await;

    assert_eq!(report.never_created, vec![theirs.clone()], "{report:?}");
    assert!(report.dropped.is_empty());
    assert!(schemas(&conn).contains(&theirs));
}

/// A schema that still holds something the drop did not list fails loudly
/// (no `CASCADE`), and the report says so rather than claiming a drop.
#[tokio::test]
async fn a_drop_that_fails_is_reported_failed_not_dropped() {
    /// Lists nothing, so the schema is still full when it is dropped.
    struct Stubborn(DuckDbPreviewDdl);
    #[async_trait::async_trait]
    impl SchemaDropper for Stubborn {
        async fn list_relations(&self, _schema: &str) -> Result<Vec<Relation>, PreviewDdlError> {
            Ok(Vec::new())
        }
        async fn drop_relation(
            &self,
            schema: &str,
            relation: &Relation,
        ) -> Result<(), PreviewDdlError> {
            self.0.drop_relation(schema, relation).await
        }
        async fn drop_schema(&self, schema: &str) -> Result<(), PreviewDdlError> {
            self.0.drop_schema(schema).await
        }
    }
    let ours = format!("preview_{KEY}__toast_pos");
    let conn = duck(&format!(
        "CREATE SCHEMA \"{ours}\"; CREATE TABLE \"{ours}\".orders (id INT);"
    ));
    let ns = PreviewNamespace::from_key(KEY).unwrap();
    let ddl = Stubborn(DuckDbPreviewDdl::new(conn.clone(), ns.clone()));
    let registered: HashMap<String, ClaimedSchema> =
        [(ours.clone(), claimed("toast_pos", true, &["orders"]))].into();
    let report = drop_listed(&ddl, &ns, &registered, std::slice::from_ref(&ours)).await;
    assert!(report.dropped.is_empty(), "{report:?}");
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(schemas(&conn).contains(&ours));
}

#[test]
fn a_payload_for_another_kind_is_refused() {
    let spec = TaskSpec::Custom {
        kind: "preview_analyze".into(),
        payload: serde_json::json!({}),
    };
    assert!(drop_payload(&spec).is_err());
    let spec = TaskSpec::Custom {
        kind: PREVIEW_SCHEMA_DROP_KIND.into(),
        payload: serde_json::json!({
            "workspace_id": uuid::Uuid::nil(),
            "preview_key": KEY,
            "schemas": [format!("preview_{KEY}__toast_pos")],
        }),
    };
    assert_eq!(drop_payload(&spec).unwrap().preview_key, KEY);
}
