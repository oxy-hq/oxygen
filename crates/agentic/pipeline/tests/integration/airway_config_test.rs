//! Precedence for `airway_source_config`: narrowest non-null field wins.
//!
//! Requires Docker (or `OXY_DATABASE_URL`); self-skips otherwise.

use std::sync::Arc;

use agentic_airway::AirwayMigrator;
use agentic_pipeline::airway_config::resolve_admission;
use agentic_runtime::migration::RuntimeMigrator;
use entity::airway_source_config;
use entity::workspaces::{self, WorkspaceStatus};
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, Set, Statement,
    Unchanged,
};
use uuid::Uuid;

static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static TEST_CONTAINER: tokio::sync::OnceCell<
    Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
> = tokio::sync::OnceCell::const_new();

async fn test_db() -> Option<DatabaseConnection> {
    let url = TEST_DB_URL
        .get_or_init(|| async {
            if let Ok(url) = std::env::var("OXY_DATABASE_URL") {
                return url;
            }
            use testcontainers::runners::AsyncRunner;
            use testcontainers::{ImageExt, ReuseDirective};
            use testcontainers_modules::postgres::Postgres;
            let container = TEST_CONTAINER
                .get_or_init(|| async {
                    Arc::new(
                        Postgres::default()
                            .with_tag("18-alpine")
                            .with_reuse(ReuseDirective::Always)
                            .start()
                            .await
                            .expect("start Postgres testcontainer — is Docker running?"),
                    )
                })
                .await;
            let port = container.get_host_port_ipv4(5432_u16).await.unwrap();
            format!("postgresql://postgres:postgres@127.0.0.1:{port}/postgres")
        })
        .await
        .clone();

    let mut db = None;
    for attempt in 0..10 {
        match sea_orm::Database::connect(&url).await {
            Ok(conn) => {
                db = Some(conn);
                break;
            }
            Err(e) if attempt < 9 => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                eprintln!("test_db: attempt {attempt} failed: {e}");
            }
            Err(e) => panic!("connect after 10 retries: {e}"),
        }
    }
    let db = db?;
    // Central first, matching every production call site (cli/commands/
    // admin.rs, agentic_cli.rs, airway.rs). `m20260317_000001_create_agentic_tables`
    // (central) creates `agentic_run_events` and its unique index without
    // `.if_not_exists()` on the index; `RuntimeMigrator`'s copy of the same
    // migration *does* guard the index. So central must run first — if
    // `RuntimeMigrator` creates the index first, central's bare `create_index`
    // collides on every run, deterministically (not a concurrency race).
    // `airway_source_config` (Task 1) lives in this central migrator. See
    // oxy_test_utils::migration for the full rationale (this is also the
    // ordering every other fixture on the shared test DB now follows).
    // `airway_run_extensions.run_id` and the queue row both FK / key off
    // `agentic_runs`, so RuntimeMigrator must precede AirwayMigrator.
    oxy_test_utils::migration::migrate_shared_test_db::<RuntimeMigrator>(&url, &db)
        .await
        .expect("shared migrations")
        .then::<AirwayMigrator>()
        .await
        .expect("airway migrations")
        .finish()
        .await;
    Some(db)
}

/// Seed a minimal `workspaces` row and return its id.
///
/// `airway_source_config.workspace_id` FKs to `workspaces(id)` (Task 1), so
/// any row with a non-null `workspace_id` needs a real referent — a bare
/// `Uuid::new_v4()` violates `airway_source_config_workspace_id_fkey`.
async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    let now = chrono::Utc::now().fixed_offset();
    workspaces::ActiveModel {
        id: Set(id),
        name: Set(format!("airway-config-test-{id}")),
        git_namespace_id: Set(None),
        git_remote_url: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        path: Set(None),
        last_opened_at: Set(None),
        created_by: Set(None),
        org_id: Set(None),
        status: Set(WorkspaceStatus::Ready),
        error: Set(None),
        monthly_vlm_budget_micros: Set(None),
        current_revision_id: Set(None),
        default_branch: Set(None),
        repo_subdir: Set(None),
    }
    .insert(db)
    .await
    .expect("seed workspace");
    id
}

/// A source_kind unique to this call, so tests never collide on
/// `airway_source_config_global_uniq` / `..._workspace_uniq`.
///
/// The tests share one un-isolated Postgres schema across the whole
/// `serial-db` nextest group (no per-test transaction rollback, no
/// `TRUNCATE` between runs) — see `.config/nextest.toml`. Two tests each
/// inserting a *global* row for the literal kind `"toast"` would collide
/// with each other's leftover row, in whichever order nextest happens to
/// run them. `source_kind` is an opaque string as far as the resolver is
/// concerned, so tagging each test's kind with a fresh UUID keeps the
/// scenario realistic (`toast-<uuid>`, `quickbooks-<uuid>`) while making
/// every test independent of run order and safe to repeat against the
/// reused testcontainer.
fn unique_kind(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

async fn insert(
    db: &DatabaseConnection,
    kind: &str,
    workspace_id: Option<Uuid>,
    policy: Option<&str>,
    environment: Option<&str>,
) {
    airway_source_config::ActiveModel {
        source_kind: Set(kind.to_string()),
        workspace_id: Set(workspace_id),
        contract_policy: Set(policy.map(str::to_string)),
        environment: Set(environment.map(str::to_string)),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert config row");
}

#[tokio::test]
async fn an_empty_table_resolves_to_nothing() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let kind = unique_kind("toast");
    let r = resolve_admission(&db, &kind, Uuid::new_v4()).await.unwrap();
    assert_eq!(
        r.contract_policy, None,
        "empty table must mean airway's default"
    );
    assert_eq!(r.environment, None);
}

#[tokio::test]
async fn the_global_row_applies_to_every_workspace() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let kind = unique_kind("toast");
    insert(&db, &kind, None, Some("require_declared"), None).await;
    for _ in 0..2 {
        let r = resolve_admission(&db, &kind, Uuid::new_v4()).await.unwrap();
        assert_eq!(r.contract_policy.as_deref(), Some("require_declared"));
    }
}

#[tokio::test]
async fn a_workspace_row_overrides_the_global_one() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let kind = unique_kind("toast");
    let ws = seed_workspace(&db).await;
    insert(&db, &kind, None, Some("require_declared"), None).await;
    insert(&db, &kind, Some(ws), Some("permissive"), None).await;
    let r = resolve_admission(&db, &kind, ws).await.unwrap();
    assert_eq!(r.contract_policy.as_deref(), Some("permissive"));
}

/// The whole point of "sparse": a workspace row that sets one field must not
/// silently reset the other to airway's default.
#[tokio::test]
async fn a_sparse_workspace_row_inherits_the_field_it_omits() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let kind = unique_kind("quickbooks");
    let ws = seed_workspace(&db).await;
    insert(
        &db,
        &kind,
        None,
        Some("require_declared"),
        Some("production"),
    )
    .await;
    insert(&db, &kind, Some(ws), None, Some("sandbox")).await;

    let r = resolve_admission(&db, &kind, ws).await.unwrap();
    assert_eq!(
        r.contract_policy.as_deref(),
        Some("require_declared"),
        "the omitted field must inherit, not reset"
    );
    assert_eq!(r.environment.as_deref(), Some("sandbox"));
}

#[tokio::test]
async fn kinds_do_not_bleed_into_each_other() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let kind_a = unique_kind("toast");
    let kind_b = unique_kind("quickbooks");
    insert(&db, &kind_a, None, Some("forbid_opaque"), None).await;
    let r = resolve_admission(&db, &kind_b, Uuid::new_v4())
        .await
        .unwrap();
    assert_eq!(
        r.contract_policy, None,
        "a toast row must not govern quickbooks"
    );
}

/// Unlike `kinds_do_not_bleed_into_each_other` (which only seeds a *global*
/// row), this seeds a *scoped* row — `(kind_a, ws)` — and resolves
/// `(kind_b, ws)`. That's the discriminating shape: the correct SQL grouping
/// is `source_kind = $1 AND (workspace_id IS NULL OR workspace_id = $2)`, so
/// a `kind_a` row is filtered out before it ever reaches Rust. A broken
/// grouping — `(source_kind = $1 AND workspace_id IS NULL) OR workspace_id =
/// $2` — would let the `kind_a` row through on the `workspace_id = $2` arm
/// alone, and `resolve_admission`'s `scoped` predicate
/// (`airway_config.rs`) only checks `r.workspace_id == Some(workspace_id)` —
/// it never re-checks `source_kind` — so `kind_b` would silently inherit
/// `kind_a`'s policy. The global-only variant can't catch this: a global row
/// matches on `workspace_id IS NULL` under both groupings, so it passes
/// either way and guards nothing against this specific bug.
#[tokio::test]
async fn a_scoped_row_does_not_bleed_into_a_different_kind_in_the_same_workspace() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let kind_a = unique_kind("toast");
    let kind_b = unique_kind("quickbooks");
    let ws = seed_workspace(&db).await;
    insert(&db, &kind_a, Some(ws), Some("forbid_opaque"), None).await;

    let r = resolve_admission(&db, &kind_b, ws).await.unwrap();
    assert_eq!(
        r.contract_policy, None,
        "a toast row scoped to this workspace must not govern quickbooks \
         in the same workspace"
    );
}

/// Pins the invariant `airway_source_config_global_uniq` exists to enforce:
/// at most one `workspace_id IS NULL` row per `source_kind`. A plain
/// composite unique index would treat two NULLs as distinct and silently
/// admit a second global row, making resolution non-deterministic.
#[tokio::test]
async fn a_second_global_row_for_the_same_kind_is_rejected() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let kind = unique_kind("toast");
    insert(&db, &kind, None, Some("require_declared"), None).await;

    let second = airway_source_config::ActiveModel {
        source_kind: Set(kind.clone()),
        workspace_id: Set(None),
        contract_policy: Set(Some("permissive".to_string())),
        environment: Set(None),
        ..Default::default()
    }
    .insert(&db)
    .await;

    assert!(
        second.is_err(),
        "a second global (workspace_id IS NULL) row for the same source_kind \
         must be rejected by airway_source_config_global_uniq"
    );
}

/// Pins the invariant `airway_source_config_workspace_uniq` exists to
/// enforce: at most one row per `(source_kind, workspace_id)` pair when
/// `workspace_id` is set.
#[tokio::test]
async fn a_duplicate_workspace_row_for_the_same_kind_is_rejected() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let kind = unique_kind("quickbooks");
    let ws = seed_workspace(&db).await;
    insert(&db, &kind, Some(ws), Some("require_declared"), None).await;

    let duplicate = airway_source_config::ActiveModel {
        source_kind: Set(kind.clone()),
        workspace_id: Set(Some(ws)),
        contract_policy: Set(Some("permissive".to_string())),
        environment: Set(None),
        ..Default::default()
    }
    .insert(&db)
    .await;

    assert!(
        duplicate.is_err(),
        "a duplicate (source_kind, workspace_id) pair must be rejected by \
         airway_source_config_workspace_uniq"
    );
}

/// Pins that `updated_at` advances on **every** update, without the writer
/// having to say so — the `airway_source_config_set_updated_at` trigger.
///
/// Both shapes matter and neither mentions the column:
///
/// - a plain ORM `UPDATE` of one field, and
/// - `INSERT ... ON CONFLICT DO UPDATE`, which is how the admin API writes.
///
/// Without the trigger the row keeps its insert-time value and the admin
/// surface reports a policy as older than it is — a lie that only ever errs
/// towards "unchanged", so nothing downstream can catch it.
#[tokio::test]
async fn updated_at_advances_on_every_update() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let kind = unique_kind("toast");
    let stale = chrono::DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z").unwrap();

    let row = airway_source_config::ActiveModel {
        source_kind: Set(kind.clone()),
        workspace_id: Set(None),
        contract_policy: Set(Some("permissive".to_string())),
        environment: Set(None),
        updated_at: Set(stale),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert with an explicitly stale updated_at");
    assert_eq!(
        row.updated_at, stale,
        "the trigger is BEFORE UPDATE only — an insert keeps the value it was given"
    );

    // 1. Plain ORM update of one unrelated column.
    airway_source_config::ActiveModel {
        id: Unchanged(row.id),
        contract_policy: Set(Some("require_declared".to_string())),
        ..Default::default()
    }
    .update(&db)
    .await
    .expect("update contract_policy only");

    let after_update = airway_source_config::Entity::find_by_id(row.id)
        .one(&db)
        .await
        .expect("re-read")
        .expect("row still there");
    assert!(
        after_update.updated_at > stale,
        "updated_at must advance on an UPDATE that never mentions it \
         (got {}, still the insert-time value)",
        after_update.updated_at
    );

    // 2. The admin API's shape: upsert onto the global partial unique index,
    //    with `updated_at` absent from the DO UPDATE SET list.
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO airway_source_config (source_kind, workspace_id, contract_policy, \
         environment, updated_at) VALUES ($1, NULL, $2, NULL, $3) \
         ON CONFLICT (source_kind) WHERE workspace_id IS NULL \
         DO UPDATE SET contract_policy = EXCLUDED.contract_policy",
        [kind.clone().into(), "forbid_opaque".into(), stale.into()],
    ))
    .await
    .expect("upsert onto the existing global row");

    let after_upsert = airway_source_config::Entity::find_by_id(row.id)
        .one(&db)
        .await
        .expect("re-read")
        .expect("row still there");
    assert_eq!(
        after_upsert.contract_policy.as_deref(),
        Some("forbid_opaque"),
        "the upsert should have taken the DO UPDATE branch, not inserted a second row"
    );
    assert!(
        after_upsert.updated_at > after_update.updated_at,
        "updated_at must advance through ON CONFLICT DO UPDATE too, even though \
         the statement passes the stale value in the INSERT half"
    );
}
