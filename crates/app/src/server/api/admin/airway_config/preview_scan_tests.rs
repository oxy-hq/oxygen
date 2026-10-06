//! DB-backed tests for [`super::scan_pipelines`] — the enumeration half.
//!
//! Separate from `preview_tests.rs`, which stays strictly pure (no DB, so it
//! cannot self-skip). These pin the thing the FS→compile-boundary correction
//! actually changed: pipelines come from `airway_pipelines` scoped to each
//! workspace's promoted `current_revision_id`, never from a working copy.
//!
//! Skips (does not fail) when `OXY_DATABASE_URL` is unset, per
//! `test_support::test_db`. Every assertion is scoped to the workspace this
//! test seeded — `scan_pipelines` is cross-tenant by design and the test
//! Postgres is shared and non-transactional, so a global count would be a
//! test that fails on whatever ran before it.
//!
//! Uses the keyed [`AdvisoryLock`] rather than `#[serial_test::serial]` for
//! the reason `handlers_tests.rs` documents at length: `serial_test`'s default
//! lock is in-process, and nextest runs every lib test in its own process, so
//! it serializes nothing.

use entity::{airway_pipelines, revisions, workspaces};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;

use super::{Scan, scan_pipelines};
use crate::server::test_support::{self, AdvisoryLock, SKIP_MSG, test_db};
use agentic_airway::{ContractPolicy, Environment};

/// Distinct from `handlers_tests::LOCK_KEY` and `MIGRATION_LOCK_KEY` — keys
/// share one namespace per database. Spells `AIRWAYP` in ASCII hex
/// (`41 49 52 57 41 59 50` = `A I R W A Y P`, P for preview).
const LOCK_KEY: i64 = 0x0041_4952_5741_5950;

async fn lock() -> AdvisoryLock {
    let url = test_support::database_url().expect("OXY_DATABASE_URL set (test_db confirmed it)");
    AdvisoryLock::acquire(&url, LOCK_KEY).await
}

async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
    seed_workspace_in_org(db, None).await
}

/// The same, with an owning org — which is what the scope fence reads. A
/// workspace is not itself scopeable; `admin::scope` keys on the org, and so
/// does [`super::load_workspaces`]' filter.
async fn seed_workspace_in_org(db: &DatabaseConnection, org_id: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    let now = chrono::Utc::now().fixed_offset();
    workspaces::ActiveModel {
        id: Set(id),
        name: Set(format!("airway-preview-test-{id}")),
        git_namespace_id: Set(None),
        git_remote_url: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        // Deliberately `None`: this endpoint must not need a working copy.
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
    entity::organizations::ActiveModel {
        id: Set(id),
        name: Set(format!("Airway Preview Org {id}")),
        slug: Set(format!("airway-preview-{id}")),
        logo: sea_orm::ActiveValue::NotSet,
        logo_content_type: sea_orm::ActiveValue::NotSet,
        created_at: sea_orm::ActiveValue::NotSet,
        updated_at: sea_orm::ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed org");
    id
}

/// A `ready` revision for `workspace_id`. Not promoted — the caller decides
/// which one lands on `workspaces.current_revision_id`, which is the whole
/// point of `a_superseded_revision_is_not_scanned`.
async fn seed_revision(db: &DatabaseConnection, workspace_id: Uuid) -> Uuid {
    let revision_id = Uuid::new_v4();
    revisions::ActiveModel {
        revision_id: Set(revision_id),
        workspace_id: Set(workspace_id),
        git_sha: Set(format!("sha-{revision_id}")),
        branch: Set(Some("main".to_string())),
        schema_version: Set(1),
        status: Set("ready".to_string()),
        kind: Set("main".to_string()),
        owner_user_id: Set(None),
        compiler_version: Set("test".to_string()),
        started_at: Set(chrono::Utc::now().fixed_offset()),
        finished_at: Set(Some(chrono::Utc::now().fixed_offset())),
        file_count_seen: Set(0),
        file_count_compiled: Set(0),
        file_count_failed: Set(0),
        error_summary: Set(None),
    }
    .insert(db)
    .await
    .expect("seed revision");
    revision_id
}

async fn promote(db: &DatabaseConnection, workspace_id: Uuid, revision_id: Uuid) {
    let ws = workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await
        .expect("load workspace")
        .expect("workspace exists");
    let mut active: workspaces::ActiveModel = ws.into();
    active.current_revision_id = Set(Some(revision_id));
    active.update(db).await.expect("promote revision");
}

async fn seed_pipeline(
    db: &DatabaseConnection,
    revision_id: Uuid,
    name: &str,
    file_path: &str,
    definition: serde_json::Value,
) {
    airway_pipelines::ActiveModel {
        revision_id: Set(revision_id),
        name: Set(name.to_string()),
        file_path: Set(file_path.to_string()),
        definition: Set(definition),
    }
    .insert(db)
    .await
    .expect("seed pipeline");
}

/// A toast spec in the shape the compiler stores: the `.airway.yml` document
/// verbatim as JSON, credential still behind `client_secret_var`.
fn toast_definition(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "source": {
            "kind": "toast",
            "config": {
                "client_id": "client-123",
                "client_secret_var": "TOAST_CLIENT_SECRET",
                "restaurant_guids": ["restaurant-1"],
            },
        },
        "destination": { "database": "warehouse", "dataset_name": "raw" },
    })
}

/// A rest_api spec with one **cursored** endpoint — uncursored endpoints are
/// exempt from every policy, so an uncursored fixture would pass vacuously.
///
/// `auth.type` is required: airway's `AuthConfig` is internally tagged
/// (`#[serde(tag = "type")]`), and the executor's secret substitution only ever
/// inserts `token`/`key` alongside it. Omitting the tag makes `RestApiConfig`
/// fail to deserialize, which lands the pipeline in `unevaluated` — worth
/// stating, because that failure mode looks like a broken preview rather than
/// a malformed fixture.
fn rest_api_definition(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "source": {
            "kind": "rest_api",
            "config": {
                "base_url": "https://api.example.invalid",
                "auth": { "type": "bearer", "token_var": "STRIPE_TOKEN" },
                "endpoints": [{
                    "name": "charges",
                    "path": "/charges",
                    "cursor_field": "created",
                }],
            },
        },
        "destination": { "database": "warehouse", "dataset_name": "raw" },
    })
}

/// The same pipeline with the endpoint's contract **declared in YAML** — the
/// exact edit [`super::declaration_site`] tells a `rest_api` operator to make.
///
/// `contract` is singular and sits on the endpoint, which is airway 0.1.24's
/// `EndpointConfig::contract` (#105) and *not* a name-keyed map on
/// `RestApiConfig`. The nesting is load-bearing in both directions:
/// `EndpointConfig` does not `deny_unknown_fields`, so a misplaced or mistyped
/// key parses as "no contract" and this fixture would silently stop testing
/// anything — which is why the assertion is that the verdict *passes*, a state
/// only a really-decoded contract can produce.
fn rest_api_definition_with_contract(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "source": {
            "kind": "rest_api",
            "config": {
                "base_url": "https://api.example.invalid",
                "auth": { "type": "bearer", "token_var": "STRIPE_TOKEN" },
                "endpoints": [{
                    "name": "charges",
                    "path": "/charges",
                    "cursor_field": "created",
                    "contract": { "mutability": "immutable" },
                }],
            },
        },
        "destination": { "database": "warehouse", "dataset_name": "raw" },
    })
}

/// Verdicts belonging to the workspace this test seeded. The scan is
/// cross-tenant and the test DB is shared, so nothing may assert on totals.
fn mine(scan: &Scan, workspace_id: Uuid) -> Vec<&super::ResourceVerdict> {
    let prefix = format!("{workspace_id}:");
    scan.resources
        .iter()
        .filter(|v| v.pipeline_ref.starts_with(&prefix))
        .collect()
}

fn my_unevaluated(scan: &Scan, workspace_id: Uuid) -> Vec<&super::UnevaluatedPipeline> {
    let prefix = format!("{workspace_id}:");
    scan.unevaluated
        .iter()
        .filter(|u| u.pipeline_ref.starts_with(&prefix))
        .collect()
}

/// The core of the enumeration: pipelines come from the compiled rows under
/// the promoted revision, and only the requested `source_kind` is scored. Note
/// the seeded workspace has `path: None` — if this ever regressed to an FS
/// walk, there would be nothing to walk and the test would find no verdicts.
#[tokio::test]
async fn scans_compiled_pipelines_of_the_requested_kind_only() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;

    let workspace_id = seed_workspace(&db).await;
    let revision_id = seed_revision(&db, workspace_id).await;
    promote(&db, workspace_id, revision_id).await;
    seed_pipeline(
        &db,
        revision_id,
        "toast_orders",
        "pipelines/toast.airway.yml",
        toast_definition("toast_orders"),
    )
    .await;
    seed_pipeline(
        &db,
        revision_id,
        "stripe_charges",
        "pipelines/stripe.airway.yml",
        rest_api_definition("stripe_charges"),
    )
    .await;

    let scan = scan_pipelines(
        &db,
        "toast",
        ContractPolicy::RequireDeclared,
        Environment::Production,
        None,
    )
    .await
    .expect("scan");
    let verdicts = mine(&scan, workspace_id);

    assert!(
        !verdicts.is_empty(),
        "the compiled toast pipeline must be scored — the workspace has no `path`, so a \
         regression to a filesystem walk would produce exactly this empty result"
    );
    assert!(
        verdicts
            .iter()
            .all(|v| v.pipeline_ref == format!("{workspace_id}:pipelines/toast.airway.yml")),
        "a rest_api pipeline must not appear in a toast preview: {:?}",
        verdicts.iter().map(|v| &v.pipeline_ref).collect::<Vec<_>>()
    );
    assert!(
        my_unevaluated(&scan, workspace_id).is_empty(),
        "both definitions parse and both connectors build with placeholder credentials"
    );

    held.release().await;
}

/// An undeclared `rest_api` endpoint under `require_declared` fails, and is
/// **fixable** — end to end from a stored definition. The pure tests fake the
/// contract map; this one goes through a real `RestApiSource` built from
/// compiled JSON, which is what makes it the load-bearing check that airway
/// 0.1.24's `EndpointConfig::contract` field (#105) is actually reachable from
/// a definition Oxy stores.
///
/// This assertion was inverted by the 0.1.24 bump: the endpoint used to be
/// flagged `not_fixable_here` because no slot existed to declare through.
/// [`a_rest_api_endpoint_declaring_a_contract_in_yaml_passes`] is the other
/// half — that the fix this now advertises actually works.
#[tokio::test]
async fn an_undeclared_rest_api_endpoint_previews_as_fixable() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;

    let workspace_id = seed_workspace(&db).await;
    let revision_id = seed_revision(&db, workspace_id).await;
    promote(&db, workspace_id, revision_id).await;
    seed_pipeline(
        &db,
        revision_id,
        "stripe_charges",
        "pipelines/stripe.airway.yml",
        rest_api_definition("stripe_charges"),
    )
    .await;

    let scan = scan_pipelines(
        &db,
        "rest_api",
        ContractPolicy::RequireDeclared,
        Environment::Production,
        None,
    )
    .await
    .expect("scan");
    let verdicts = mine(&scan, workspace_id);

    let charges = verdicts
        .iter()
        .find(|v| v.resource == "charges")
        .expect("the cursored endpoint is scored");
    assert!(!charges.passes);
    assert!(
        !charges.not_fixable_here,
        "airway 0.1.24 gave `EndpointConfig` a `contract` field, so declaring this is an edit \
         to the pipeline's own YAML — not an upstream limitation"
    );
    let reason = charges
        .reason
        .as_ref()
        .expect("a failing verdict has a reason");
    assert!(
        reason.contains("source.config.endpoints"),
        "a `rest_api` operator must be sent to the endpoint in their own `.airway.yml`, not \
         to connector source — got: {reason}"
    );

    held.release().await;
}

/// The other half of the diagnostic: the edit it advertises **works**.
///
/// Declaring `contract:` on the endpoint, in the stored definition, makes the
/// same resource pass `require_declared` — through the real
/// `source_factory::build_rest_api` → `RestApiSource::contracts()` path, not a
/// fake map. This is what makes [`super::declaration_site`]'s `rest_api` branch
/// a checked fact rather than a claim: if #105's field were named differently,
/// nested elsewhere, or unreachable from `source.config`, `EndpointConfig`'s
/// tolerance of unknown keys would swallow it silently and this test would be
/// the only thing that noticed.
#[tokio::test]
async fn a_rest_api_endpoint_declaring_a_contract_in_yaml_passes() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;

    let workspace_id = seed_workspace(&db).await;
    let revision_id = seed_revision(&db, workspace_id).await;
    promote(&db, workspace_id, revision_id).await;
    seed_pipeline(
        &db,
        revision_id,
        "stripe_charges",
        "pipelines/stripe.airway.yml",
        rest_api_definition_with_contract("stripe_charges"),
    )
    .await;

    let scan = scan_pipelines(
        &db,
        "rest_api",
        ContractPolicy::RequireDeclared,
        Environment::Production,
        // Unbounded, like every case here that is about admission rather than
        // the scope fence — this one is checking that a declared contract
        // passes, not who may see it.
        None,
    )
    .await
    .expect("scan");
    let verdicts = mine(&scan, workspace_id);

    assert!(
        my_unevaluated(&scan, workspace_id).is_empty(),
        "the definition with a `contract:` block still builds a connector"
    );
    let charges = verdicts
        .iter()
        .find(|v| v.resource == "charges")
        .expect("the cursored endpoint is scored");
    assert!(
        charges.passes,
        "a contract declared in YAML must satisfy `require_declared` — got: {:?}",
        charges.reason
    );
    assert_eq!(
        charges.mutability, "immutable",
        "the declared mutability reaches the verdict, so this is the real contract and not a \
         silently-ignored unknown key"
    );

    held.release().await;
}

/// Pipelines under a revision that is no longer promoted must not be scored.
/// This is what `current_revision_id` scoping buys: `airway_pipelines`
/// accumulates a row set per compile, so an unscoped query would preview
/// long-deleted pipelines.
#[tokio::test]
async fn a_superseded_revision_is_not_scanned() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;

    let workspace_id = seed_workspace(&db).await;
    let old_revision = seed_revision(&db, workspace_id).await;
    let new_revision = seed_revision(&db, workspace_id).await;
    seed_pipeline(
        &db,
        old_revision,
        "deleted_pipeline",
        "pipelines/deleted.airway.yml",
        toast_definition("deleted_pipeline"),
    )
    .await;
    seed_pipeline(
        &db,
        new_revision,
        "live_pipeline",
        "pipelines/live.airway.yml",
        toast_definition("live_pipeline"),
    )
    .await;
    promote(&db, workspace_id, new_revision).await;

    let scan = scan_pipelines(
        &db,
        "toast",
        ContractPolicy::RequireDeclared,
        Environment::Production,
        None,
    )
    .await
    .expect("scan");
    let refs: Vec<&String> = mine(&scan, workspace_id)
        .iter()
        .map(|v| &v.pipeline_ref)
        .collect();

    assert!(
        refs.iter()
            .any(|r| r.ends_with("pipelines/live.airway.yml")),
        "the promoted revision's pipeline is scored: {refs:?}"
    );
    assert!(
        !refs
            .iter()
            .any(|r| r.ends_with("pipelines/deleted.airway.yml")),
        "a superseded revision's pipeline must not appear: {refs:?}"
    );

    held.release().await;
}

/// A stored definition that will not deserialize is reported, never dropped
/// and never counted as passing — the same rule as a connector that won't
/// build. A compiled row can hold this: the compiler only requires valid YAML
/// with a top-level mapping, not a valid `AirwayPipelineSpec`.
#[tokio::test]
async fn an_undeserializable_definition_is_unevaluated_not_dropped() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;

    let workspace_id = seed_workspace(&db).await;
    let revision_id = seed_revision(&db, workspace_id).await;
    promote(&db, workspace_id, revision_id).await;
    seed_pipeline(
        &db,
        revision_id,
        "broken",
        "pipelines/broken.airway.yml",
        // No `source`, and `destination` is the wrong shape.
        serde_json::json!({ "name": "broken", "destination": 7 }),
    )
    .await;

    let scan = scan_pipelines(
        &db,
        "toast",
        ContractPolicy::Permissive,
        Environment::Production,
        None,
    )
    .await
    .expect("scan");

    let reported = my_unevaluated(&scan, workspace_id);
    assert_eq!(
        reported.len(),
        1,
        "the broken pipeline is reported under `unevaluated`"
    );
    assert!(
        reported[0]
            .pipeline_ref
            .ends_with("pipelines/broken.airway.yml")
    );
    assert!(
        mine(&scan, workspace_id).is_empty(),
        "and contributes no passing verdicts"
    );

    held.release().await;
}

/// A workspace that has never compiled is **counted, not listed** — and the
/// count lives in its own field, never in `unevaluated`.
///
/// This is the regression test for the gate defect: folding never-compiled
/// workspaces into `unevaluated` made that list permanently non-empty on any
/// real deployment, which pinned `computeSaveGate` to `incomplete` and made
/// **every** save confirm. Two different facts were sharing one list — "this
/// pipeline exists and could not be evaluated" (a genuine coverage gap, must
/// gate) versus "this workspace has nothing compiled at all" (nothing of this
/// kind to check, honest to report, never resolves on its own).
///
/// Both halves are asserted: the count is reported, and `unevaluated` stays
/// free of anything synthetic.
#[tokio::test]
async fn a_workspace_with_no_promoted_revision_is_counted_not_listed_as_unevaluated() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;

    let workspace_id = seed_workspace(&db).await;

    let scan = scan_pipelines(
        &db,
        "toast",
        ContractPolicy::RequireDeclared,
        Environment::Production,
        None,
    )
    .await
    .expect("scan");

    // The workspace this test just seeded has no promoted revision, so the
    // count covers at least it. The scan is cross-tenant against a shared test
    // DB, so the assertion is `>= 1`, never an exact total.
    assert!(
        scan.uncompiled_workspaces >= 1,
        "the never-compiled workspace must be reported, not silently skipped"
    );

    // Nothing synthetic in `unevaluated`: no summary row, and no bare
    // workspace id. Both would break `pipeline_ref`'s `{workspace_id}:{path}`
    // contract that the UI splits on, and — the actual bug — both would make
    // `unevaluated.len() > 0` mean "coverage is incomplete" when it isn't.
    assert!(
        !scan
            .unevaluated
            .iter()
            .any(|u| u.pipeline_ref == "(workspaces with no compiled revision)"),
        "the never-compiled summary must NOT ride in `unevaluated` — that made the save gate \
         permanently `incomplete`, so every save confirmed and the confirmation stopped meaning \
         anything"
    );
    assert!(
        !scan
            .unevaluated
            .iter()
            .any(|u| u.pipeline_ref == workspace_id.to_string()),
        "a bare workspace id must never appear as a `pipeline_ref` — it breaks the \
         `{{workspace_id}}:{{path}}` contract the UI splits on"
    );
    assert!(
        my_unevaluated(&scan, workspace_id).is_empty(),
        "and the workspace contributes no `unevaluated` entry of its own"
    );

    held.release().await;
}

/// The other half of the same distinction: a pipeline that genuinely cannot be
/// evaluated still lands in `unevaluated` and does **not** move the
/// never-compiled counter. Paired with the test above so a future "simplify
/// this" change cannot collapse the two facts back into one list without
/// failing here.
#[tokio::test]
async fn an_unevaluable_pipeline_gates_while_a_never_compiled_workspace_does_not() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;

    // One workspace with a broken compiled pipeline…
    let broken_ws = seed_workspace(&db).await;
    let revision_id = seed_revision(&db, broken_ws).await;
    promote(&db, broken_ws, revision_id).await;
    seed_pipeline(
        &db,
        revision_id,
        "broken",
        "pipelines/broken.airway.yml",
        serde_json::json!({ "name": "broken", "destination": 7 }),
    )
    .await;
    // …and one that has never compiled at all.
    let never_compiled_ws = seed_workspace(&db).await;

    let scan = scan_pipelines(
        &db,
        "toast",
        ContractPolicy::RequireDeclared,
        Environment::Production,
        None,
    )
    .await
    .expect("scan");

    assert_eq!(
        my_unevaluated(&scan, broken_ws).len(),
        1,
        "the pipeline that exists but cannot be evaluated is the coverage gap that must gate"
    );
    assert!(
        my_unevaluated(&scan, never_compiled_ws).is_empty(),
        "the workspace with nothing compiled contributes no coverage gap"
    );
    assert!(
        scan.uncompiled_workspaces >= 1,
        "it is still reported, just on the other field"
    );

    held.release().await;
}

// ---------------------------------------------------------------------------
// Scope (review round 3)
// ---------------------------------------------------------------------------
//
// The preview shipped unfenced while the neighbouring override routes were
// being narrowed, and it returned strictly more than the listing did: a
// `pipeline_ref` is `{workspace_id}:{real path}`, `resource` names tables
// inside another tenant's pipeline, and an `unevaluated` entry quotes their
// `.airway.yml`. These pin both halves of the correction — the detail is
// narrowed, and the remainder is counted rather than dropped.

/// A workspace in `org`, with one promoted toast pipeline. Returns the
/// workspace id, which is the prefix every verdict of its own carries.
async fn seed_toast_workspace(db: &DatabaseConnection, org: Option<Uuid>, name: &str) -> Uuid {
    let workspace_id = seed_workspace_in_org(db, org).await;
    let revision_id = seed_revision(db, workspace_id).await;
    promote(db, workspace_id, revision_id).await;
    seed_pipeline(
        db,
        revision_id,
        name,
        &format!("pipelines/{name}.airway.yml"),
        toast_definition(name),
    )
    .await;
    workspace_id
}

/// The finding, in one test: a Global Admin bounded to org A must not be able
/// to enumerate org B's airway pipelines — and must still be told how much of
/// the fleet the answer left out, because the *global* row they can write
/// reaches all of it.
#[tokio::test]
async fn a_bounded_grant_scans_only_its_own_orgs_and_counts_the_remainder() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;

    // Not named `mine` — that is the helper above, and shadowing it here would
    // silently turn every verdict assertion into a call on a `Uuid`.
    let my_org = seed_org(&db).await;
    let their_org = seed_org(&db).await;
    let my_ws = seed_toast_workspace(&db, Some(my_org), "mine").await;
    let their_ws = seed_toast_workspace(&db, Some(their_org), "theirs").await;
    // A workspace owned by nobody is not reachable by a bounded grant either —
    // a null org is by definition not in `Scope::Orgs(..)`.
    let orphan_ws = seed_toast_workspace(&db, None, "orphan").await;

    let scan = scan_pipelines(
        &db,
        "toast",
        ContractPolicy::RequireDeclared,
        Environment::Production,
        Some(&[my_org]),
    )
    .await
    .expect("scan");

    assert!(
        !mine(&scan, my_ws).is_empty(),
        "the fence narrows; it does not break the surface for the operator it admits"
    );
    assert!(
        mine(&scan, their_ws).is_empty(),
        "another tenant's pipeline_ref names their workspace id and a real file path — \
         a bounded grant must not be able to enumerate it"
    );
    assert!(
        mine(&scan, orphan_ws).is_empty(),
        "an org-less workspace is refused for a bounded grant, same direction as \
         `deny_out_of_scope_opt`"
    );
    assert!(
        scan.out_of_scope_pipelines >= 2,
        "the withheld portion is REPORTED, not silently dropped: at least the other \
         tenant's and the orphan's pipeline were kept out of this answer, and the \
         global row this operator can still write reaches both. Got {}",
        scan.out_of_scope_pipelines
    );

    held.release().await;
}

/// A grant bounded to nothing is a real answer, not an unbounded one: it scores
/// nothing and the entire fleet is the remainder.
#[tokio::test]
async fn a_grant_bounded_to_nothing_scores_nothing() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;

    let org = seed_org(&db).await;
    let ws = seed_toast_workspace(&db, Some(org), "bounded_to_nothing").await;

    let scan = scan_pipelines(
        &db,
        "toast",
        ContractPolicy::RequireDeclared,
        Environment::Production,
        Some(&[]),
    )
    .await
    .expect("scan");

    assert!(
        mine(&scan, ws).is_empty(),
        "`Some(&[])` must not be read as unbounded — it reaches no org at all"
    );
    assert!(
        scan.out_of_scope_pipelines >= 1,
        "and every pipeline in the deployment is the remainder"
    );

    held.release().await;
}

/// An unbounded caller — a Global Owner, or a `scope_all` grant — sees exactly
/// what this endpoint returned before it was fenced, with nothing withheld.
/// Paired with the bounded test so a future "simplify the scope plumbing"
/// change cannot start narrowing the callers it was never meant to narrow.
#[tokio::test]
async fn an_unbounded_grant_scans_every_org_and_withholds_nothing() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let held = lock().await;

    let org_a = seed_org(&db).await;
    let org_b = seed_org(&db).await;
    let ws_a = seed_toast_workspace(&db, Some(org_a), "unbounded_a").await;
    let ws_b = seed_toast_workspace(&db, Some(org_b), "unbounded_b").await;
    let orphan_ws = seed_toast_workspace(&db, None, "unbounded_orphan").await;

    let scan = scan_pipelines(
        &db,
        "toast",
        ContractPolicy::RequireDeclared,
        Environment::Production,
        None,
    )
    .await
    .expect("scan");

    for ws in [ws_a, ws_b, orphan_ws] {
        assert!(
            !mine(&scan, ws).is_empty(),
            "an unbounded grant is not narrowed by this fence — {ws} is missing"
        );
    }
    assert_eq!(
        scan.out_of_scope_pipelines, 0,
        "nothing was withheld, so the count is zero rather than a number the UI would \
         have to explain away"
    );

    held.release().await;
}
