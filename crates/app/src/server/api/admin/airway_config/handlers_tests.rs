//! Tests for `list_airway_config` (Task 1, read) and `upsert_global` /
//! `upsert_override` / `delete_global` / `delete_override` (Task 2, write).
//! DB-backed — skips (not fails) when `OXY_DATABASE_URL` is unset, per
//! `test_support::test_db`.
//!
//! Tests below insert rows under the LITERAL known-kind spellings
//! (`"toast"`, …) because `lists_every_known_kind_even_with_no_rows`
//! specifically asserts against [`KNOWN_SOURCE_KINDS`] — the real constant,
//! not a stand-in. That's unlike `agentic-pipeline`'s
//! `tests/airway_config_test.rs`, which suffixes every kind with a fresh
//! UUID to stay independent of the shared, non-transactional test Postgres.
//! A `unique_kind()` trick isn't available here: a made-up kind would never
//! appear in the response at all.
//!
//! So every test wraps its reset+mutate+assert in a
//! [`test_support::AdvisoryLock`], keyed by [`LOCK_KEY`], instead of
//! `#[serial_test::serial]` — that attribute's default lock is an in-process
//! `parking_lot` mutex (this crate's `Cargo.lock` doesn't pull in
//! `serial_test`'s `file_locks`/`fslock` feature), and nextest runs every
//! `kind(lib)` test in its own OS process, so two `#[serial]` tests never
//! actually contend. `AdvisoryLock` uses `pg_advisory_lock`, which serializes
//! across processes for real — see `test_support.rs`'s "Serializing a test's
//! own critical section". Each test also resets the four known kinds first,
//! independent of the lock, so a leftover row from a previous run against a
//! persistent (non-CI) local Postgres can't poison the next run either.

use axum::http::StatusCode;
use entity::users::UserStatus;
use entity::{airway_source_config, organizations, workspaces};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
};
use uuid::Uuid;

use super::{
    KNOWN_SOURCE_KINDS, delete_global, delete_override, deny_out_of_scope_for_workspace,
    list_airway_config, upsert_global, upsert_override,
};
use crate::server::test_support::{self, AdvisoryLock, SKIP_MSG, test_db};

/// Arbitrary but fixed, and distinct from `test_support::MIGRATION_LOCK_KEY`
/// and every other advisory-lock caller — keys share one namespace per
/// database. Spells `AIRWAY` in ASCII hex: `41 49 52 57 41 59` = `A I R W A Y`.
const LOCK_KEY: i64 = 0x4149_5257_4159;

/// Acquire the critical-section lock for these tests. Panics (via
/// `database_url().expect`) rather than silently no-op'ing if called without
/// `OXY_DATABASE_URL` set — callers only reach this after `test_db()` has
/// already confirmed it's set, so that would mean the env changed mid-test.
async fn lock() -> AdvisoryLock {
    let url = test_support::database_url().expect("OXY_DATABASE_URL set (test_db confirmed it)");
    AdvisoryLock::acquire(&url, LOCK_KEY).await
}

/// Clear any row for every real known kind — see the module doc for why.
///
/// Deliberately not a count: it said "four" while there were five, and each
/// new connector made it more wrong. The constant is the only source of truth.
async fn reset_known_kinds(db: &DatabaseConnection) {
    airway_source_config::Entity::delete_many()
        .filter(airway_source_config::Column::SourceKind.is_in(KNOWN_SOURCE_KINDS.to_vec()))
        .exec(db)
        .await
        .expect("reset known kinds");
}

/// Seed a minimal `workspaces` row and return its id.
/// `airway_source_config.workspace_id` FKs to `workspaces(id)`, so a bare
/// `Uuid::new_v4()` violates `airway_source_config_workspace_id_fkey`.
async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
    seed_workspace_in_org(db, None).await
}

/// The same, with an owning org — which is what the scope fence reads. A
/// workspace is not itself scopeable; `admin::scope` keys on the org.
async fn seed_workspace_in_org(db: &DatabaseConnection, org_id: Option<Uuid>) -> Uuid {
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
        org_id: Set(org_id),
        status: Set(workspaces::WorkspaceStatus::Ready),
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

async fn seed_org(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(id),
        name: Set(format!("Airway Config Org {id}")),
        slug: Set(format!("airway-config-{id}")),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed org");
    id
}

/// Seed a platform grant and return the actor holding it. `scope` of `None` is
/// unbounded (`scope_all = true`); `Some(orgs)` writes the bounded form plus
/// its child rows — `Some(&[])` is the real "reaches nothing" grant.
///
/// The email is fresh per call on purpose: `platform_grant_checked` caches by
/// email for `ADMIN_CACHE_TTL`, so a reused address would answer from another
/// test's grant.
async fn seed_actor(db: &DatabaseConnection, scope: Option<&[Uuid]>) -> AuthenticatedUser {
    let grant_id = Uuid::new_v4();
    let email = format!("airway-scope-{grant_id}@example.com");
    entity::app_admins::ActiveModel {
        id: Set(grant_id),
        email: Set(email.clone()),
        granted_by: Set(None),
        created_at: ActiveValue::NotSet,
        role: Set(oxy_authz::PlatformRole::GlobalAdmin.as_str().to_string()),
        scope_all: Set(scope.is_none()),
        updated_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed platform grant");

    for org_id in scope.unwrap_or_default() {
        entity::app_admin_scope_orgs::ActiveModel {
            id: Set(Uuid::new_v4()),
            app_admin_id: Set(grant_id),
            org_id: Set(*org_id),
            created_at: ActiveValue::NotSet,
            created_by: Set(None),
        }
        .insert(db)
        .await
        .expect("seed grant scope org");
    }

    AuthenticatedUser {
        id: Uuid::new_v4(),
        email: Some(email),
        name: "airway scope test".to_string(),
        picture: None,
        status: UserStatus::Active,
    }
}

async fn insert_config(
    db: &DatabaseConnection,
    kind: &str,
    workspace_id: Option<Uuid>,
    contract_policy: Option<&str>,
    environment: Option<&str>,
) {
    airway_source_config::ActiveModel {
        source_kind: Set(kind.to_string()),
        workspace_id: Set(workspace_id),
        contract_policy: Set(contract_policy.map(str::to_string)),
        environment: Set(environment.map(str::to_string)),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert config row");
}

/// The global (`workspace_id IS NULL`) row for `kind`, if any. Panics on a
/// query error — a broken query is a broken test environment here, not an
/// absent row (that's `None`).
async fn load_global(db: &DatabaseConnection, kind: &str) -> Option<airway_source_config::Model> {
    airway_source_config::Entity::find()
        .filter(airway_source_config::Column::SourceKind.eq(kind))
        .filter(airway_source_config::Column::WorkspaceId.is_null())
        .one(db)
        .await
        .expect("load_global query")
}

/// The `workspace_id`-scoped override row for `kind`, if any.
async fn load_override(
    db: &DatabaseConnection,
    kind: &str,
    workspace_id: Uuid,
) -> Option<airway_source_config::Model> {
    airway_source_config::Entity::find()
        .filter(airway_source_config::Column::SourceKind.eq(kind))
        .filter(airway_source_config::Column::WorkspaceId.eq(workspace_id))
        .one(db)
        .await
        .expect("load_override query")
}

/// Every row (global and every override) for `kind` — used to assert the
/// partial unique index collapsed a second global write onto the first
/// rather than inserting a duplicate.
async fn all_rows_for(db: &DatabaseConnection, kind: &str) -> Vec<airway_source_config::Model> {
    airway_source_config::Entity::find()
        .filter(airway_source_config::Column::SourceKind.eq(kind))
        .all(db)
        .await
        .expect("all_rows_for query")
}

#[tokio::test]
async fn lists_every_known_kind_even_with_no_rows() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;

    reset_known_kinds(&db).await;
    let resp = list_airway_config(&db, None).await.unwrap();
    assert_eq!(resp.kinds.len(), KNOWN_SOURCE_KINDS.len());
    assert!(
        resp.kinds.iter().all(|k| k.global.is_none()),
        "an empty table yields no global values"
    );

    held.release().await;
}

#[tokio::test]
async fn groups_the_global_row_and_its_overrides_under_one_kind() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;

    reset_known_kinds(&db).await;
    let ws = seed_workspace(&db).await;
    insert_config(&db, "toast", None, Some("permissive"), Some("production")).await;
    insert_config(&db, "toast", Some(ws), Some("require_declared"), None).await;

    let resp = list_airway_config(&db, None).await.unwrap();
    let toast = resp
        .kinds
        .iter()
        .find(|k| k.source_kind == "toast")
        .unwrap();

    assert_eq!(
        toast.global.as_ref().unwrap().contract_policy.as_deref(),
        Some("permissive")
    );
    assert_eq!(toast.overrides.len(), 1);
    assert_eq!(toast.overrides[0].workspace_id, ws);
    assert_eq!(
        toast.overrides[0].values.contract_policy.as_deref(),
        Some("require_declared"),
        "an override reports its own value, not the merged one"
    );
    assert!(
        toast.overrides[0].values.environment.is_none(),
        "a field the override omits stays None here; merging is resolve_admission's job, \
         not this endpoint's"
    );

    held.release().await;
}

// Writes (Task 2)

#[tokio::test]
async fn rejects_an_unknown_policy_spelling() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;
    reset_known_kinds(&db).await;

    let err = upsert_global(&db, "toast", Some("require-declared"), None)
        .await
        .expect_err("a typo must not be stored");
    assert!(
        format!("{err}").contains("require-declared"),
        "the error names the bad value the operator typed"
    );
    assert!(
        load_global(&db, "toast").await.is_none(),
        "nothing was written"
    );

    held.release().await;
}

#[tokio::test]
async fn upsert_replaces_rather_than_duplicating_the_global_row() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;
    reset_known_kinds(&db).await;

    upsert_global(&db, "toast", Some("permissive"), Some("production"))
        .await
        .unwrap();
    upsert_global(&db, "toast", Some("forbid_opaque"), Some("production"))
        .await
        .unwrap();

    let rows = all_rows_for(&db, "toast").await;
    assert_eq!(
        rows.len(),
        1,
        "the partial unique index collapses the second write"
    );
    assert_eq!(rows[0].contract_policy.as_deref(), Some("forbid_opaque"));

    held.release().await;
}

#[tokio::test]
async fn a_null_field_clears_back_to_inherit() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;
    reset_known_kinds(&db).await;

    upsert_global(&db, "toast", Some("permissive"), Some("sandbox"))
        .await
        .unwrap();
    upsert_global(&db, "toast", Some("permissive"), None)
        .await
        .unwrap();

    let row = load_global(&db, "toast").await.unwrap();
    assert!(
        row.environment.is_none(),
        "omitting a field clears it; it does not preserve the prior value"
    );

    held.release().await;
}

#[tokio::test]
async fn deleting_an_override_leaves_the_global_row() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;
    reset_known_kinds(&db).await;

    let ws = seed_workspace(&db).await;
    upsert_global(&db, "toast", Some("permissive"), None)
        .await
        .unwrap();
    upsert_override(&db, "toast", ws, Some("forbid_opaque"), None)
        .await
        .unwrap();

    delete_override(&db, "toast", ws).await.unwrap();

    assert!(load_global(&db, "toast").await.is_some());
    assert!(load_override(&db, "toast", ws).await.is_none());

    held.release().await;
}

/// Mirror of `deleting_an_override_leaves_the_global_row`: `delete_global`
/// is the other half of the pair and must not sweep up an override row for
/// the same `source_kind` just because it shares the kind.
#[tokio::test]
async fn deleting_the_global_row_leaves_an_override() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;
    reset_known_kinds(&db).await;

    let ws = seed_workspace(&db).await;
    upsert_global(&db, "toast", Some("permissive"), None)
        .await
        .unwrap();
    upsert_override(&db, "toast", ws, Some("forbid_opaque"), None)
        .await
        .unwrap();

    delete_global(&db, "toast").await.unwrap();

    assert!(
        load_global(&db, "toast").await.is_none(),
        "the global row is gone"
    );
    assert!(
        load_override(&db, "toast", ws).await.is_some(),
        "the override row is untouched — deleting the global row must not \
         cascade to it"
    );

    held.release().await;
}

/// A typo in `source_kind` must not read as a successful delete.
///
/// `DELETE` being idempotent is about the *row* — deleting one that isn't
/// there is fine. It is not a licence to accept a kind that does not exist:
/// the two are indistinguishable at the wire, so without this the operator
/// reads `204` and a success toast for a delete that could never match. The
/// upsert path has validated this since Task 2; the deletes had not.
#[tokio::test]
async fn deletes_reject_an_unknown_source_kind() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;
    reset_known_kinds(&db).await;

    let ws = seed_workspace(&db).await;
    upsert_global(&db, "toast", Some("permissive"), None)
        .await
        .unwrap();
    upsert_override(&db, "toast", ws, Some("forbid_opaque"), None)
        .await
        .unwrap();

    let err = delete_global(&db, "toats")
        .await
        .expect_err("a misspelled kind is not a no-op, it is a mistake");
    assert!(
        format!("{err}").contains("toats"),
        "the error names the bad kind the operator typed"
    );
    let err = delete_override(&db, "toats", ws)
        .await
        .expect_err("the override delete validates the same way");
    assert!(format!("{err}").contains("toats"));

    assert!(
        load_global(&db, "toast").await.is_some(),
        "the real rows are untouched — a rejected kind writes nothing"
    );
    assert!(load_override(&db, "toast", ws).await.is_some());

    held.release().await;
}

// ---------------------------------------------------------------------------
// Scope (review round 2)
// ---------------------------------------------------------------------------
//
// `platform_cap_guard` decides on `Resource::platform()`, which has no org, so
// a **bounded** grant passes the `PlatformOperate` gate on this console and the
// handler is the only thing left. These pin the handler half: the write fence
// and the read filter, in both directions, plus the property that a caller who
// is not scoped at all is unaffected by either.

/// The status a refusal carries — `deny_out_of_scope_for_workspace` answers in
/// `Response`s, which is what the handlers propagate.
fn status_of(result: Result<(), axum::response::Response>) -> Option<StatusCode> {
    result.err().map(|resp| resp.status())
}

#[tokio::test]
async fn a_bounded_grant_may_write_an_override_inside_its_scope() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;
    reset_known_kinds(&db).await;

    let org = seed_org(&db).await;
    let ws = seed_workspace_in_org(&db, Some(org)).await;
    let actor = seed_actor(&db, Some(&[org])).await;

    assert_eq!(
        status_of(deny_out_of_scope_for_workspace(&db, &actor, ws).await),
        None,
        "a grant that reaches this workspace's org passes the fence"
    );
    // And the write itself still works — the fence narrows, it does not break
    // the surface for the operators it admits.
    upsert_override(&db, "toast", ws, Some("forbid_opaque"), None)
        .await
        .unwrap();
    assert!(load_override(&db, "toast", ws).await.is_some());

    held.release().await;
}

/// The finding, in one assertion: a Global Admin bounded to org A must not be
/// able to pin org B's pipelines to a stricter policy.
#[tokio::test]
async fn a_bounded_grant_is_refused_an_override_outside_its_scope() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;
    reset_known_kinds(&db).await;

    let mine = seed_org(&db).await;
    let theirs = seed_org(&db).await;
    let their_ws = seed_workspace_in_org(&db, Some(theirs)).await;
    let actor = seed_actor(&db, Some(&[mine])).await;

    assert_eq!(
        status_of(deny_out_of_scope_for_workspace(&db, &actor, their_ws).await),
        Some(StatusCode::NOT_FOUND),
        "404, not 403 — an operator with no reach into a tenant must not learn \
         its workspaces exist by being told 'forbidden'"
    );

    held.release().await;
}

/// A workspace with no owning org is refused for a bounded grant. A null org is
/// by definition not in `Scope::Orgs(..)`, and `if let Some(org) { check }` —
/// the obvious spelling — is not a check that passes but no check at all.
#[tokio::test]
async fn a_bounded_grant_is_refused_an_org_less_workspace() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;
    reset_known_kinds(&db).await;

    let org = seed_org(&db).await;
    let orphan_ws = seed_workspace_in_org(&db, None).await;
    let actor = seed_actor(&db, Some(&[org])).await;

    assert_eq!(
        status_of(deny_out_of_scope_for_workspace(&db, &actor, orphan_ws).await),
        Some(StatusCode::NOT_FOUND)
    );

    held.release().await;
}

/// An **unscoped** caller is unaffected — the whole point of narrowing only
/// those who are actually scoped. `scope_all = true` is the unbounded grant;
/// the Global Owner takes the same path one branch earlier (`admin::scope`
/// short-circuits on the env allow-list, pinned in `app_scope_boundary.rs`).
#[tokio::test]
async fn an_unbounded_grant_reaches_every_workspace() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;
    reset_known_kinds(&db).await;

    let org_a = seed_org(&db).await;
    let org_b = seed_org(&db).await;
    let ws_a = seed_workspace_in_org(&db, Some(org_a)).await;
    let ws_b = seed_workspace_in_org(&db, Some(org_b)).await;
    let orphan = seed_workspace_in_org(&db, None).await;
    let actor = seed_actor(&db, None).await;

    for ws in [ws_a, ws_b, orphan] {
        assert_eq!(
            status_of(deny_out_of_scope_for_workspace(&db, &actor, ws).await),
            None,
            "an unbounded grant is not narrowed by this fence"
        );
    }

    held.release().await;
}

/// A workspace that does not exist answers exactly what an out-of-scope one
/// does, so the route cannot be used to probe the workspace directory. (It also
/// stops a `PUT` naming a stale id from surfacing as a raw FK violation.)
#[tokio::test]
async fn a_missing_workspace_is_indistinguishable_from_an_unreachable_one() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;
    reset_known_kinds(&db).await;

    let actor = seed_actor(&db, None).await;
    assert_eq!(
        status_of(deny_out_of_scope_for_workspace(&db, &actor, Uuid::new_v4()).await),
        Some(StatusCode::NOT_FOUND),
        "even an unbounded grant gets 404 for a workspace that isn't there"
    );

    held.release().await;
}

/// The read half: `get_config` must not hand a bounded operator every tenant's
/// overrides. The global row is deliberately NOT filtered — it is one
/// fleet-wide row per kind, and it is what every override inherits from.
#[tokio::test]
async fn the_listing_filters_overrides_to_the_callers_scope() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;
    reset_known_kinds(&db).await;

    let mine = seed_org(&db).await;
    let theirs = seed_org(&db).await;
    let my_ws = seed_workspace_in_org(&db, Some(mine)).await;
    let their_ws = seed_workspace_in_org(&db, Some(theirs)).await;
    let orphan_ws = seed_workspace_in_org(&db, None).await;

    insert_config(&db, "toast", None, Some("require_declared"), None).await;
    insert_config(&db, "toast", Some(my_ws), Some("permissive"), None).await;
    insert_config(&db, "toast", Some(their_ws), Some("permissive"), None).await;
    insert_config(&db, "toast", Some(orphan_ws), Some("permissive"), None).await;

    let toast = |resp: super::AirwayConfigResponse| {
        resp.kinds
            .into_iter()
            .find(|k| k.source_kind == "toast")
            .expect("toast is a known kind")
    };

    let bounded = toast(list_airway_config(&db, Some(&[mine])).await.unwrap());
    assert_eq!(
        bounded
            .overrides
            .iter()
            .map(|o| o.workspace_id)
            .collect::<Vec<_>>(),
        vec![my_ws],
        "a bounded grant sees only the overrides in the orgs it reaches — not \
         the other tenant's, and not the org-less one"
    );
    assert!(
        bounded.global.is_some(),
        "the fleet-wide global row is still reported; hiding it would leave a \
         scoped operator editing overrides against a policy they cannot see"
    );

    let unbounded = toast(list_airway_config(&db, None).await.unwrap());
    assert_eq!(
        unbounded.overrides.len(),
        3,
        "an unscoped caller is unaffected — every override, exactly as before"
    );

    let bounded_to_nothing = toast(list_airway_config(&db, Some(&[])).await.unwrap());
    assert!(
        bounded_to_nothing.overrides.is_empty(),
        "`Some(&[])` is a real answer — a grant bounded to nothing — and must \
         not be read as unbounded"
    );

    held.release().await;
}
