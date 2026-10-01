//! A preview schema comes to exist only through `registry::ensure_schema`:
//! its row first, then a strict `CREATE SCHEMA`. A schema the preview did not
//! create — one that was already there — is refused, never adopted, and so
//! never dropped.

use std::sync::atomic::Ordering;

use oxy_app::server::previews::registry::{self, RegistryError};
use uuid::Uuid;

use super::done_metadata;
use super::doubles::{Observing, Rendezvous};
use super::fixture::{Fx, TTL, setup};

fn observing(fx: &Fx, ws: Uuid) -> Observing {
    Observing {
        db: fx.db.clone(),
        ws,
        inner: fx.ddl(),
        seen: Default::default(),
    }
}

/// Two first writes into the same live schema at once (two replicas, or a
/// retry racing its original): exactly one creates the schema, and neither
/// refuses it. The second waits on the row's lock, finds the schema created,
/// and sends nothing — rather than failing its own strict create against the
/// first caller's fresh schema and refusing the preview's own schema, which
/// would leave its copies of production data behind for good.
#[tokio::test]
async fn two_concurrent_first_writes_create_once_and_refuse_nothing() {
    let fx = setup().await;
    let ddl = Rendezvous::new(fx.ddl());

    let (a, b) = tokio::join!(
        registry::ensure_schema(&fx.db, &ddl, fx.ws, &fx.ns, "toast_pos", "run-1", TTL),
        registry::ensure_schema(&fx.db, &ddl, fx.ws, &fx.ns, "toast_pos", "run-2", TTL),
    );

    let schema = a.expect("the first caller");
    assert_eq!(b.expect("the second caller"), schema);
    assert_eq!(ddl.creates.load(Ordering::SeqCst), 1, "created once");
    let row = fx.row(&schema).await;
    assert!(
        row.refused_at.is_none() && row.schema_created_at.is_some(),
        "{row:?}"
    );
    assert!(fx.duck_schemas().contains(&schema));
}

#[tokio::test]
async fn registry_row_exists_before_the_schema() {
    let fx = setup().await;
    let ddl = observing(&fx, fx.ws);
    let schema = registry::ensure_schema(&fx.db, &ddl, fx.ws, &fx.ns, "Toast_POS", "run-1", TTL)
        .await
        .unwrap();
    assert_eq!(schema, format!("{}toast_pos", fx.ns.prefix()));
    assert_eq!(*ddl.seen.lock().unwrap(), vec![(schema.clone(), true)]);
    assert!(fx.duck_schemas().contains(&schema));
    assert!(fx.row(&schema).await.schema_created_at.is_some());
    // Created once: the next write re-arms the row and sends no DDL.
    registry::ensure_schema(&fx.db, &ddl, fx.ws, &fx.ns, "toast_pos", "run-2", TTL)
        .await
        .unwrap();
    assert_eq!(ddl.seen.lock().unwrap().len(), 1);

    // No row, no schema: a row that cannot be written (here, a workspace that
    // does not exist) never reaches `CREATE SCHEMA`, nor does a live schema
    // with no preview stand-in.
    let ghost = observing(&fx, Uuid::new_v4());
    let err = registry::ensure_schema(&fx.db, &ghost, ghost.ws, &fx.ns, "sales", "run-1", TTL)
        .await
        .unwrap_err();
    assert!(matches!(err, RegistryError::Db(_)), "{err:?}");
    let err = registry::ensure_schema(&fx.db, &ghost, fx.ws, &fx.ns, "preview_notes", "run-1", TTL)
        .await
        .unwrap_err();
    assert!(matches!(err, RegistryError::Refused(_)), "{err:?}");
    assert!(ghost.seen.lock().unwrap().is_empty());
    assert!(
        !fx.duck_schemas()
            .contains(&format!("{}sales", fx.ns.prefix()))
    );
}

/// A customer made `preview_<key>__marketing` by hand. The preview's first
/// write into `marketing` must not adopt it: the strict create fails, the row
/// is refused, the write is refused (now and on every retry), and the schema
/// is never claimed or dropped.
#[tokio::test]
async fn a_preexisting_customer_schema_with_a_preview_name_is_never_adopted() {
    let fx = setup().await;
    let theirs = format!("{}marketing", fx.ns.prefix());
    fx.duck_exec(&format!(
        "CREATE SCHEMA \"{theirs}\"; CREATE TABLE \"{theirs}\".campaigns AS SELECT 1 AS id;"
    ));

    let err = registry::ensure_schema(&fx.db, &fx.ddl(), fx.ws, &fx.ns, "marketing", "run-1", TTL)
        .await
        .unwrap_err();

    assert!(matches!(err, RegistryError::Refused(_)), "{err:?}");
    let row = fx.row(&theirs).await;
    assert!(
        row.refused_at.is_some() && row.schema_created_at.is_none(),
        "{row:?}"
    );
    let retry = observing(&fx, fx.ws);
    let err = registry::ensure_schema(&fx.db, &retry, fx.ws, &fx.ns, "marketing", "run-2", TTL)
        .await
        .unwrap_err();
    assert!(matches!(err, RegistryError::Refused(_)), "{err:?}");
    assert!(
        retry.seen.lock().unwrap().is_empty(),
        "no DDL for a refused row"
    );
    registry::expire_key(&fx.db, fx.ws, fx.key()).await.unwrap();
    assert!(fx.sweep_after(200).await.is_empty(), "never claimed");
    assert_eq!(fx.duck_count(&format!("\"{theirs}\".campaigns")), 1);
}

/// The same, one step later: the preview's own schema was dropped by the
/// TTL, then someone created one of that name. The preview writing again
/// re-arms its row, and the strict create refuses what is now someone else's.
#[tokio::test]
async fn a_schema_made_after_the_drop_is_not_adopted_on_rearm() {
    let fx = setup().await;
    let schema = fx.ensure("toast_pos").await;
    let claims = fx.sweep_after(73).await;
    done_metadata(&fx.run_drop(fx.droppers(), &claims[0]).await);
    assert!(fx.row(&schema).await.dropped_at.is_some());
    fx.duck_exec(&format!(
        "CREATE SCHEMA \"{schema}\"; CREATE TABLE \"{schema}\".theirs AS SELECT 1 AS id;"
    ));

    let err = registry::ensure_schema(&fx.db, &fx.ddl(), fx.ws, &fx.ns, "toast_pos", "run-3", TTL)
        .await
        .unwrap_err();

    assert!(matches!(err, RegistryError::Refused(_)), "{err:?}");
    let row = fx.row(&schema).await;
    assert!(
        row.refused_at.is_some() && row.dropped_at.is_none(),
        "{row:?}"
    );
    assert!(fx.sweep_after(200).await.is_empty());
    assert_eq!(fx.duck_count(&format!("\"{schema}\".theirs")), 1);
}

/// A live schema whose stand-in the DDL port would refuse (`toast-pos` is not
/// `[a-z0-9_]`) gets no registry row at all, not a row with no schema.
#[tokio::test]
async fn a_live_schema_the_port_refuses_gets_no_row() {
    let fx = setup().await;
    let ddl = observing(&fx, fx.ws);
    let err = registry::ensure_schema(&fx.db, &ddl, fx.ws, &fx.ns, "toast-pos", "run-1", TTL)
        .await
        .unwrap_err();
    assert!(matches!(err, RegistryError::Refused(_)), "{err:?}");
    let rows = fx
        .count(
            "SELECT count(*)::bigint AS n FROM workspace_preview_schemas WHERE workspace_id = $1",
            vec![fx.ws.into()],
        )
        .await;
    assert_eq!(rows, 0);
    assert!(ddl.seen.lock().unwrap().is_empty());
}

/// The database half of "a row can only name its own key's schema".
#[tokio::test]
async fn the_registry_refuses_a_name_its_key_does_not_own() {
    let fx = setup().await;
    let err = sea_orm::ConnectionTrait::execute_raw(
        &fx.db,
        sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "INSERT INTO workspace_preview_schemas \
                 (workspace_id, schema_name, preview_key, live_schema, created_by_run_id, expires_at) \
             VALUES ($1, 'preview_notes', $2, 'notes', 'run-1', now())",
            [fx.ws.into(), fx.key().into()],
        ),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("ck_preview_schema_name"), "{err}");
}
