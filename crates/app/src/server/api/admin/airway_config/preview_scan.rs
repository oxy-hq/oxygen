//! Reading the compile boundary for [`super::preview`]: enumerate every
//! compiled pipeline of one source kind, in every workspace, and score it.
//!
//! Split out of `preview.rs` along the seam the async/sync boundary already
//! draws. [`load_scan_rows`] is Postgres queries and nothing else;
//! [`evaluate_rows`] is a CPU-bound loop that deserializes every stored
//! `definition`, validates it, and builds a connector. Keeping them in one
//! `async fn` is what let a single operator's click hold a Tokio worker for
//! the whole scan on a large fleet, so the sync half runs under
//! [`tokio::task::spawn_blocking`] and the split is now structural rather than
//! a comment someone has to remember.
//!
//! **Why compiled rows and not the working copy** is argued in `preview.rs`'s
//! module doc — read it there before "fixing" any of this back to an FS walk.
//!
//! **Scope.** [`load_scan_rows`] narrows the `workspaces` query to the caller's
//! platform grant, and counts — without loading — the pipelines that narrowing
//! left out. `preview.rs`'s module doc argues why the fence belongs here at all;
//! this module owns only its mechanics.

use std::collections::HashMap;

use agentic_airway::placeholder::substitute_secret_vars;
use agentic_airway::{AirwayPipelineSpec, ContractPolicy, Environment, build_source_connector};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, FromQueryResult, JoinType,
    PaginatorTrait, QueryFilter, QuerySelect,
};
use uuid::Uuid;

use super::preview::{ResourceVerdict, UnevaluatedPipeline, verdicts};

/// What one scan found.
///
/// `unevaluated` and `uncompiled_workspaces` are deliberately **two fields,
/// not one list**. See [`Scan::uncompiled_workspaces`].
#[derive(Default)]
pub(crate) struct Scan {
    pub(crate) resources: Vec<ResourceVerdict>,
    /// Pipelines that exist and could not be evaluated. A genuine coverage
    /// gap: each names a real `.airway.yml` whose verdict is unknown, so the
    /// save gate must treat a non-empty list as "impact unknown".
    pub(crate) unevaluated: Vec<UnevaluatedPipeline>,
    /// How many workspaces have no promoted revision at all.
    ///
    /// **Reported, never gated on.** A workspace that has never compiled has
    /// no pipelines of any kind to check, so this is not a coverage gap in the
    /// answer — it is a statement about workspaces that are not in the
    /// question. It is also permanent: on any real deployment at least one
    /// workspace has never compiled, and nothing an operator does here ever
    /// resolves it.
    ///
    /// This used to be a synthetic `unevaluated` entry with the fabricated
    /// `pipeline_ref` `"(workspaces with no compiled revision)"`. That made
    /// `unevaluated` non-empty forever, which pinned the frontend save gate to
    /// its `incomplete` state, which made **every** save confirm — a
    /// confirmation that always fires trains operators to click through the
    /// guardrail, so the guardrail stopped existing. Two different facts were
    /// sharing one list; they are separated here so a consumer cannot
    /// accidentally count them together, and so the fabricated `pipeline_ref`
    /// (which broke [`ResourceVerdict::pipeline_ref`]'s
    /// `{workspace_id}:{path}` contract) is gone. A count rather than a list
    /// because one entry per workspace is unbounded on a fleet of idle
    /// tenants.
    ///
    /// Scoped like the verdicts: for a bounded grant this counts only the
    /// workspaces the caller reaches. The ones it does not reach are in
    /// [`Self::out_of_scope_pipelines`]' half of the answer, whether they have
    /// compiled or not — which is why the two must never be summed.
    pub(crate) uncompiled_workspaces: usize,
    /// How many compiled pipelines the caller's platform scope kept out of
    /// this answer. Always `0` for an unbounded grant.
    ///
    /// **A count, and never anything richer.** The whole reason the scan is
    /// fenced is that a `pipeline_ref` names a tenant's workspace id and a real
    /// file path, so a list here would re-open the leak the fence closes. A
    /// bare number says "your view is partial" without saying whose.
    ///
    /// **Reported rather than dropped**, because the global row a bounded
    /// operator can still write is fleet-wide (see `admin::mod`'s mount-point
    /// note). A preview that silently omitted the other tenants would render a
    /// small, clean verdict list for a change whose blast radius is the whole
    /// deployment — "reads as safe, means unknown", which is the exact failure
    /// this surface's save gate exists to prevent.
    ///
    /// **Counts pipelines of every kind, not just the previewed one**, and is
    /// therefore an over-count for any one kind. `source.kind` lives inside the
    /// JSON `definition`, so narrowing it means either deserializing the very
    /// rows the fence keeps out of this process, or a JSON-path predicate that
    /// returns NULL for exactly the malformed definitions an in-scope scan
    /// would report as a coverage gap. Over-stating the remainder is the safe
    /// direction and the UI wording says "pipelines", not "`<kind>`
    /// pipelines"; under-stating it would be the bug.
    pub(crate) out_of_scope_pipelines: usize,
}

/// Only the two `workspaces` columns the scan reads.
///
/// The full `Model` drags `name`, both git URLs, `path`, `status`, `error`,
/// `monthly_vlm_budget_micros` and the rest across the wire for every
/// workspace in the deployment, none of which this endpoint looks at.
#[derive(FromQueryResult)]
struct WorkspaceRow {
    id: Uuid,
    current_revision_id: Option<Uuid>,
}

/// Only the three `airway_pipelines` columns the scan reads. `name` is the
/// other half of the primary key and is never rendered — `file_path` is what
/// an operator reads.
#[derive(FromQueryResult)]
struct PipelineRow {
    revision_id: Uuid,
    file_path: String,
    definition: serde_json::Value,
}

/// Every compiled pipeline of this kind that the caller's grant reaches,
/// scored — plus a count of the ones it does not.
///
/// Cross-tenant *within the caller's reach*, because the config being previewed
/// is: the global row for a kind applies to every workspace that doesn't
/// override it, so a preview scoped to one workspace would understate the blast
/// radius of the exact change this endpoint exists to de-risk. `scope_orgs`
/// narrows which tenants' detail comes back — `None` (a Global Owner, or a
/// `scope_all` grant) is unbounded and takes exactly the queries this function
/// has always run.
///
/// `environment` is the second admission axis and is threaded all the way to
/// `build_source_connector`, not defaulted — see [`evaluate_pipeline`].
pub(crate) async fn scan_pipelines(
    db: &DatabaseConnection,
    source_kind: &str,
    policy: ContractPolicy,
    environment: Environment,
    scope_orgs: Option<&[Uuid]>,
) -> Result<Scan, DbErr> {
    let ScanRows {
        workspaces,
        pipelines,
        out_of_scope_pipelines,
    } = load_scan_rows(db, scope_orgs).await?;
    let source_kind = source_kind.to_string();
    // The loop below is pure CPU — N `serde_json::from_value` + `validate()` +
    // `build_source_connector` calls, one per compiled pipeline the caller
    // reaches. Off the async thread so one admin click cannot stall a Tokio
    // worker (and with it every other request that worker was multiplexing).
    let mut scan = tokio::task::spawn_blocking(move || {
        evaluate_rows(&workspaces, pipelines, &source_kind, policy, environment)
    })
    .await
    // `JoinError` here means the closure panicked (it returns no `Result` of
    // its own — an unreadable pipeline lands in `unevaluated`, not in `Err`).
    // Surfaced rather than swallowed; `DbErr` is the caller's error channel,
    // and both map to the same 500.
    .map_err(|e| DbErr::Custom(format!("airway policy preview evaluation panicked: {e}")))?;
    // Carried on the same struct the verdicts ride, so nothing downstream can
    // report one without the other.
    scan.out_of_scope_pipelines = out_of_scope_pipelines;
    Ok(scan)
}

/// What [`load_scan_rows`] hands the CPU half, plus the remainder the fence
/// withheld from it.
struct ScanRows {
    workspaces: Vec<WorkspaceRow>,
    pipelines: Vec<PipelineRow>,
    out_of_scope_pipelines: usize,
}

/// **Two queries for an unbounded caller, no N+1 and no filesystem.** One for
/// `workspaces` (to learn each one's promoted `current_revision_id`), one for
/// the `airway_pipelines` rows under those revisions. Scoping by
/// `current_revision_id` — rather than taking every row in the table — is what
/// keeps a superseded or in-flight revision's pipelines out of the answer;
/// `airway_pipelines` is keyed by `revision_id` and accumulates a row set per
/// compile.
///
/// Both queries are `select_only`: this runs on every preview click across the
/// whole fleet, and the columns nobody reads are pure transfer cost.
///
/// **`scope_orgs` fences at the query, not at the response.** `Some(orgs)`
/// filters `workspaces` by `org_id` before anything is loaded, so an
/// out-of-scope tenant's `file_path`s and `.airway.yml` parse errors never
/// enter this process at all — the property a post-hoc filter over a fleet-wide
/// result set would not have. It costs a bounded caller exactly one extra
/// query, the joined `COUNT` in [`count_out_of_scope_pipelines`]; `None` pays
/// for none of it and issues byte-for-byte the same two statements as before
/// the fence existed.
async fn load_scan_rows(
    db: &DatabaseConnection,
    scope_orgs: Option<&[Uuid]>,
) -> Result<ScanRows, DbErr> {
    let workspaces = load_workspaces(db, scope_orgs).await?;

    let revision_ids: Vec<Uuid> = workspaces
        .iter()
        .filter_map(|ws| ws.current_revision_id)
        .collect();

    // `is_in([])` is a valid but pointless query; skip the round trip.
    let pipelines = if revision_ids.is_empty() {
        Vec::new()
    } else {
        entity::airway_pipelines::Entity::find()
            .select_only()
            .column(entity::airway_pipelines::Column::RevisionId)
            .column(entity::airway_pipelines::Column::FilePath)
            .column(entity::airway_pipelines::Column::Definition)
            .filter(entity::airway_pipelines::Column::RevisionId.is_in(revision_ids))
            .into_model::<PipelineRow>()
            .all(db)
            .await?
    };

    let out_of_scope_pipelines = match scope_orgs {
        // Unbounded: nothing was withheld, and — deliberately — not one extra
        // round trip to establish that.
        None => 0,
        Some(orgs) => count_out_of_scope_pipelines(db, orgs).await?,
    };

    Ok(ScanRows {
        workspaces,
        pipelines,
        out_of_scope_pipelines,
    })
}

/// The workspaces whose pipelines this caller may see in full.
///
/// A workspace with a NULL `org_id` is **excluded for a bounded grant** — a null
/// org is by definition not in `Scope::Orgs(..)`. Same direction as
/// `handlers::list_airway_config` takes for an org-less override and as
/// `scope::deny_out_of_scope_opt` takes for an org-less workspace: an org that
/// cannot be established means refuse.
async fn load_workspaces(
    db: &DatabaseConnection,
    scope_orgs: Option<&[Uuid]>,
) -> Result<Vec<WorkspaceRow>, DbErr> {
    // `Some(&[])` is a real answer — a grant bounded to nothing — and reaches
    // no workspace at all. Short-circuited rather than sent as `org_id IN ()`.
    if scope_orgs.is_some_and(<[Uuid]>::is_empty) {
        return Ok(Vec::new());
    }
    let mut query = entity::workspaces::Entity::find()
        .select_only()
        .column(entity::workspaces::Column::Id)
        .column(entity::workspaces::Column::CurrentRevisionId);
    if let Some(orgs) = scope_orgs {
        query = query.filter(entity::workspaces::Column::OrgId.is_in(orgs.to_vec()));
    }
    query.into_model::<WorkspaceRow>().all(db).await
}

/// How many compiled pipelines sit under a promoted revision this bounded grant
/// does **not** reach.
///
/// **One joined `COUNT`, never a revision-id list.** This used to read every
/// out-of-scope workspace, collect their `current_revision_id`s, and send them
/// straight back as one `IN (…)` bind list. Unlike the in-scope query in
/// [`load_scan_rows`], this half is unbounded by scope *by definition* — the
/// narrower the grant, the larger the remainder — so the bounded caller paid a
/// full-table read of every workspace it cannot see, and then a bind list that
/// reaches Postgres' 65535-parameter ceiling before it reaches anything else.
/// Joining `airway_pipelines` to `workspaces` on
/// `revision_id = current_revision_id` asks the identical question in one
/// statement whose cost the fleet's size does not drive into a wall:
///
/// ```sql
/// SELECT COUNT(*) AS num_items FROM (
///   SELECT "airway_pipelines"."revision_id" FROM "airway_pipelines"
///   INNER JOIN "workspaces"
///           ON "airway_pipelines"."revision_id" = "workspaces"."current_revision_id"
///   WHERE "workspaces"."org_id" IS NULL OR "workspaces"."org_id" NOT IN ($1, …)
/// ) AS "sub_query"
/// ```
///
/// The predicate is the complement of [`load_workspaces`]' filter, still
/// **spelled out rather than derived by subtraction**: `NOT IN` is not the
/// negation of `IN` when the column is nullable (`NULL NOT IN (..)` is NULL,
/// which no `WHERE` clause keeps), so the org-less workspaces this caller
/// cannot reach have to be named explicitly or they would vanish from both
/// halves and the remainder would under-report.
/// `a_bounded_grant_scans_only_its_own_orgs_and_counts_the_remainder` seeds
/// exactly that workspace.
///
/// The join is on the *promoted* revision, so — exactly as before, and exactly
/// as the in-scope query does — a superseded revision's rows are not counted
/// and a workspace that has never compiled contributes nothing. A revision is
/// the current revision of at most one workspace, so the inner join cannot
/// double-count a row.
///
/// Only a number crosses the wire — see [`Scan::out_of_scope_pipelines`] for
/// why nothing richer ever may.
async fn count_out_of_scope_pipelines(
    db: &DatabaseConnection,
    scope_orgs: &[Uuid],
) -> Result<usize, DbErr> {
    let mut query = entity::airway_pipelines::Entity::find()
        .select_only()
        .column(entity::airway_pipelines::Column::RevisionId)
        .join(
            JoinType::InnerJoin,
            entity::airway_pipelines::Entity::belongs_to(entity::workspaces::Entity)
                .from(entity::airway_pipelines::Column::RevisionId)
                .to(entity::workspaces::Column::CurrentRevisionId)
                .into(),
        );
    // A grant bounded to nothing reaches nothing, so *every* compiled pipeline
    // is the remainder — and an unfiltered join says that without an empty `IN`.
    if !scope_orgs.is_empty() {
        query = query.filter(
            Condition::any()
                .add(entity::workspaces::Column::OrgId.is_null())
                .add(entity::workspaces::Column::OrgId.is_not_in(scope_orgs.to_vec())),
        );
    }
    Ok(query.count(db).await? as usize)
}

/// The CPU half: score every loaded row. No I/O of any kind, which is what
/// makes it safe to hand to `spawn_blocking`.
fn evaluate_rows(
    workspaces: &[WorkspaceRow],
    pipelines: Vec<PipelineRow>,
    source_kind: &str,
    policy: ContractPolicy,
    environment: Environment,
) -> Scan {
    let mut by_revision: HashMap<Uuid, Vec<PipelineRow>> = HashMap::new();
    for row in pipelines {
        by_revision.entry(row.revision_id).or_default().push(row);
    }

    let mut scan = Scan::default();
    for ws in workspaces {
        let Some(revision_id) = ws.current_revision_id else {
            // Counted, not listed, and kept out of `unevaluated` entirely —
            // see `Scan::uncompiled_workspaces` for why that distinction is
            // the whole point of this field.
            scan.uncompiled_workspaces += 1;
            continue;
        };
        for row in by_revision.get(&revision_id).into_iter().flatten() {
            let pipeline_ref = format!("{}:{}", ws.id, row.file_path);
            match evaluate_pipeline(
                &row.definition,
                source_kind,
                &pipeline_ref,
                policy,
                environment,
            ) {
                Ok(Some(mut verdicts)) => scan.resources.append(&mut verdicts),
                Ok(None) => {}
                Err(error) => scan.unevaluated.push(UnevaluatedPipeline {
                    pipeline_ref,
                    error,
                }),
            }
        }
    }

    // Row order is whatever Postgres returned, which nothing guarantees. Sort
    // so two calls against an unchanged revision produce an identical body.
    scan.resources
        .sort_by(|a, b| (&a.pipeline_ref, &a.resource).cmp(&(&b.pipeline_ref, &b.resource)));
    scan.unevaluated
        .sort_by(|a, b| a.pipeline_ref.cmp(&b.pipeline_ref));
    scan
}

/// Parse one compiled `definition`, and — when it is this `source_kind` —
/// build the connector and score every resource it exposes.
///
/// `definition` is the `.airway.yml` document as JSON (`oxy-compile`'s
/// `compile_named_yaml` stores the parsed YAML verbatim), so this deserializes
/// it rather than re-reading and re-parsing text. [`AirwayPipelineSpec::
/// validate`] still runs, so a spec the run path would reject is rejected here
/// too.
///
/// `Ok(None)` is "a pipeline for some other source kind", which this policy
/// cannot affect. `Err` is "could not be evaluated at all", which the caller
/// reports under `unevaluated`: an unreadable pipeline is not a safe one, and
/// dropping it would let the preview render all-clear for a workspace it never
/// actually looked at. A definition that will not deserialize lands here for
/// *every* kind's preview — we cannot know whose it is, and omitting it would
/// understate the blast radius.
///
/// `environment` is passed through to `build_source_connector` rather than
/// hardcoded to `Production`. It is a **real admission axis**, not a cosmetic
/// one: `admit_environment_is_applied` refuses a `sandbox` build for any
/// connector declaring a sandbox host this factory has no arm applying, and a
/// preview computed under `production` says nothing about that. A refusal
/// lands in `unevaluated` exactly like any other unbuildable connector, which
/// is the honest answer — "this policy's impact under `sandbox` is unknown" —
/// rather than a clean scan borrowed from the other environment.
///
/// Synchronous, and that is the point: no filesystem, no network. See
/// [`substitute_secret_vars`] (in `agentic_airway::placeholder`, shared with
/// the workspace previews' Airway change check) for why placeholder
/// credentials are safe.
pub(crate) fn evaluate_pipeline(
    definition: &serde_json::Value,
    source_kind: &str,
    pipeline_ref: &str,
    policy: ContractPolicy,
    environment: Environment,
) -> Result<Option<Vec<ResourceVerdict>>, String> {
    let spec: AirwayPipelineSpec = serde_json::from_value(definition.clone()).map_err(|e| {
        format!("compiled definition does not deserialize as an airway pipeline: {e}")
    })?;
    spec.validate().map_err(|e| e.to_string())?;
    if spec.source.kind != source_kind {
        return Ok(None);
    }
    let mut source = spec.source;
    // Read before substitution — the key is preserved (see
    // `agentic_airway::placeholder::NON_CREDENTIAL_VARS`), but reading here
    // keeps the decision next to the config it is made from.
    let read_only = source
        .config
        .get("access_token_var")
        .and_then(|v| v.as_str())
        .is_some();
    substitute_secret_vars(&mut source.config);
    // A config declaring read-only custody must be handed a matching source or
    // `build_quickbooks` refuses it — correctly, since the alternative is
    // falling back to refreshing. The stub is safe for the same reason the
    // placeholder secrets are: connector construction performs no I/O, and
    // nothing in this scan ever issues a request, so `access_token` is never
    // called.
    let tokens = read_only.then(|| {
        agentic_airway::QuickBooksTokens::ReadOnly(std::sync::Arc::new(PlaceholderAccessToken))
    });
    let connector = build_source_connector(&source, tokens, environment)
        .map_err(|e| format!("connector could not be built: {e}"))?;
    Ok(Some(verdicts(
        source_kind,
        pipeline_ref,
        &connector.resources(),
        &connector.contracts(),
        policy,
    )))
}

/// Stands in for the host's access-token source when previewing a quickbooks
/// pipeline in read-only token custody.
///
/// It can never be called: the preview builds connectors and reads their
/// declared resources/contracts, and issues no request. Returning an error
/// rather than [`agentic_airway::placeholder::PLACEHOLDER_SECRET`] keeps that
/// true by construction — if a future arm ever *does* authenticate at
/// construction time, this fails loudly instead of sending a fake bearer token
/// from a staff admin route. Shared with the workspace previews' Airway change
/// check (`server::previews::analyze`), which builds connectors the same way.
pub(crate) struct PlaceholderAccessToken;

#[async_trait::async_trait]
impl agentic_airway::AccessTokenSource for PlaceholderAccessToken {
    async fn access_token(&self) -> Result<String, String> {
        Err("preview scan never issues requests; no access token is available".into())
    }
}

#[cfg(test)]
#[path = "preview_scan_tests.rs"]
mod scan_tests;
