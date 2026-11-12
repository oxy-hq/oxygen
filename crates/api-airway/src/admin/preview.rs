//! `GET /api/admin/airway/config/{source_kind}/preview` — which resources an
//! admission policy would reject, *before* anyone saves it.
//!
//! Tasks 1-2 made `airway_source_config` editable. The hazard that creates is
//! one click wide: tightening a kind to `require_declared` halts every pipeline
//! whose resources don't satisfy it, and the refusal only surfaces at the next
//! run, as a config error from a queued worker. This endpoint answers what the
//! write endpoints cannot — *which* resources, in *which* pipelines, and
//! whether the operator can do anything about it.
//!
//! Two halves, deliberately separated:
//!
//! * [`verdicts`] is **pure**: resources + declared contracts + policy in,
//!   per-resource verdicts out. It mirrors airway's `ContractPolicy::check`,
//!   which collapses a whole connector to one `Err`, into the per-resource
//!   shape an operator can act on. `preview_tests.rs` pins it against the real
//!   `admit_with` so the two cannot drift on an airway bump.
//! * [`super::preview_scan`] enumerates pipelines from the **compile boundary**
//!   (`airway_pipelines`, scoped to each workspace's promoted revision),
//!   builds each connector, and feeds [`verdicts`].
//!
//! **Why compiled rows and not the working copy.** `agentic_pipeline::
//! airway_run` used to resolve a run's spec with `tokio::fs::read_to_string`,
//! which made it tempting to argue the working copy is "what actually runs"
//! and read that. That argument was rejected on the `oxy-compile-boundary`
//! rule — every per-request read comes from Postgres, and `airway_run`'s FS
//! read was itself the violation, not a precedent to inherit. It now reads the
//! compiled row too, so the argument has no premise left. `airway_pipelines`
//! already
//! carries `file_path` + `definition`, so nothing here needs a working copy,
//! and the route is `FleetOk` like the rest of this surface. **Do not "fix"
//! this back to an FS walk**; that would re-pin a Postgres-only admin surface
//! to the ide node.
//!
//! The accepted cost is that the preview reflects the **promoted revision** and
//! can lag an uncommitted/uncompiled working copy until the next compile. That
//! trade-off was taken deliberately: no staleness banner, no
//! uncompiled-changes detection.
//!
//! **Scope: this route carries its own fence, like every other per-workspace
//! route on the surface.** `platform_cap_guard` decides on
//! `Resource::platform()`, which has no org, so a *bounded* `global_admin`
//! passes the `PlatformOperate` gate and narrowing is the handler's job. This
//! one shipped without it while the neighbouring routes were being fenced, and
//! the gap was the largest of the set: the response is per-tenant detail —
//! [`ResourceVerdict::pipeline_ref`] is `{workspace_id}:{real file path}`,
//! [`ResourceVerdict::resource`] names tables inside another tenant's pipeline,
//! and [`UnevaluatedPipeline::error`] is `serde`/`validate()` text quoting their
//! `.airway.yml`. A two-org operator could enumerate every tenant's airway
//! pipelines through it, which is strictly more than `get_config` ever
//! returned. So [`preview_policy`] resolves the caller's reach with the same
//! `scope_org_filter` `get_config` uses and hands it to
//! [`super::preview_scan::scan_pipelines`].
//!
//! **The withheld portion is reported as a count, never dropped.** A bounded
//! operator can still write the fleet-wide *global* row (the residual recorded
//! at `admin::mod`'s mount point), so a preview that quietly omitted the other
//! tenants would render a short, clean list for a change that reaches all of
//! them — "reads as safe, means unknown", the failure this whole surface's save
//! gate exists to prevent. [`PolicyPreviewResponse::out_of_scope_pipelines`] is
//! a bare number for the same reason the detail is fenced: it says *that* the
//! view is partial without saying *whose*.
//!
//! **Both admission axes are previewed**, `contract_policy` and `environment`.
//! An earlier version scoped this to `contract_policy` alone, reasoning that
//! `environment` is a per-connector yes/no with no per-resource verdict, so
//! previewing it would render a table of identical rows. That is an argument
//! about the *table*, not about which environment the scan is *computed*
//! under, and taking it that far was a bug: the frontend's save gate trusts
//! this body, and a body computed under `Environment::Production` says nothing
//! about a save that sets `sandbox`. `environment` is now a query parameter
//! threaded to `build_source_connector` and echoed back in the response, so a
//! preview is attributable to the exact `(source_kind, contract_policy,
//! environment)` triple it was computed under and the client can verify that
//! rather than assume it.

use std::collections::{HashMap, HashSet};

use agentic_airway::{
    AirwayAdmission, ContractPolicy, Environment, Mutability, ResourceInfo, SourceContract,
};
use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::Response;
use serde::{Deserialize, Serialize};

use oxy_auth::extractor::AuthenticatedUserExtractor;

use super::KNOWN_SOURCE_KINDS;
use super::preview_scan::scan_pipelines;
use oxy_app::surface::admin_db::{connect, db_err, error_body};
use oxy_app::surface::admin_scope::scope_org_filter;

#[derive(Debug, Deserialize)]
pub struct PreviewQuery {
    /// Wire spelling of the policy to preview. Absent = airway's default
    /// (`permissive`), which is what a kind with no config row runs under
    /// today — so a bare call previews the status quo.
    #[serde(default)]
    pub contract_policy: Option<String>,
    /// Wire spelling of the environment to preview (`production` / `sandbox`).
    /// Absent = airway's default (`production`), same status-quo rule.
    ///
    /// Not cosmetic: `agentic_airway::source_factory` refuses a `sandbox`
    /// build for any connector declaring a sandbox host it has no arm to
    /// apply, so the two environments can produce genuinely different answers.
    #[serde(default)]
    pub environment: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PolicyPreviewResponse {
    pub source_kind: String,
    /// The policy actually previewed, echoed back. With [`Self::environment`]
    /// this makes the body self-describing: a client can check that what it is
    /// looking at was computed for the settings it is about to save, instead
    /// of inferring it from a cache key.
    pub contract_policy: String,
    /// The environment actually previewed, echoed back. See above.
    pub environment: String,
    pub resources: Vec<ResourceVerdict>,
    /// Pipelines whose connector could not be built (bad YAML, unsupported
    /// kind, structurally invalid config, or an environment this factory
    /// cannot apply). Reported, never silently dropped — an unreadable
    /// pipeline is not a passing one. **This is the coverage gap**: every
    /// entry names a real `.airway.yml` whose verdict is unknown, so a
    /// non-empty list means the previewed impact is incomplete.
    pub unevaluated: Vec<UnevaluatedPipeline>,
    /// How many workspaces have no promoted revision at all — nothing
    /// compiled, so no pipelines of any kind to check.
    ///
    /// Deliberately **not** an `unevaluated` entry, which is where it used to
    /// live as a synthetic row. It is honest to report and wrong to gate on:
    /// on any real deployment at least one workspace has never compiled, so
    /// folding it into `unevaluated` made that list permanently non-empty and
    /// pinned the frontend save gate to "coverage incomplete" forever — every
    /// save confirming, which trains operators to click through the very
    /// guardrail the gate is. See `preview_scan::Scan::uncompiled_workspaces`.
    pub uncompiled_workspaces: usize,
    /// How many compiled pipelines the caller's platform scope kept out of this
    /// answer — `0` for an unbounded grant, and for every caller before this
    /// route was fenced.
    ///
    /// Reported so a bounded operator saving the **fleet-wide global row** can
    /// see that the verdict list above describes only part of what the write
    /// reaches. Not gated on, for the same reason `uncompiled_workspaces` is
    /// not: it is non-zero for *every* request a bounded grant ever makes, so a
    /// gate keyed on it would confirm every save forever and stop meaning
    /// anything. See `preview_scan::Scan::out_of_scope_pipelines` for why it is
    /// a count of pipelines of every kind rather than of this one.
    pub out_of_scope_pipelines: usize,
}

#[derive(Debug, Serialize)]
pub struct ResourceVerdict {
    /// `{workspace_id}:{workspace-relative path}`. Qualified because this is a
    /// cross-tenant surface: a bare `pipelines/toast.airway.yml` names a
    /// different file in every workspace that has one.
    pub pipeline_ref: String,
    pub resource: String,
    /// `immutable` / `versioned` / `opaque`, or `undeclared`. See
    /// [`mutability_label`] for why the last one is not spelled `opaque`.
    pub mutability: String,
    pub passes: bool,
    /// Why it fails, in the operator's terms. `None` when it passes.
    pub reason: Option<String>,
    /// True when the failure cannot be fixed from Oxy — no setting on this
    /// surface, and no declaration in the pipeline's YAML, reaches it. Drives
    /// the frontend's upstream warning.
    ///
    /// **Since airway 0.1.24 exactly one thing raises this: an orphaned
    /// contract** (see [`orphan_verdicts`]). The other former source — a
    /// source kind with no way to declare a contract at all — no longer
    /// exists, because #105 gave `rest_api` an `EndpointConfig::contract`
    /// field and every other kind already declared in Rust. The field is kept
    /// rather than deleted because the orphan case is real and unchanged: a
    /// contract naming no resource is a typo in connector *source*, which no
    /// operator action on this page can repair.
    pub not_fixable_here: bool,
}

#[derive(Debug, Serialize)]
pub struct UnevaluatedPipeline {
    /// Always a real `{workspace_id}:{path}` — nothing synthetic is ever
    /// pushed here, because the frontend splits on the first `:` to render it.
    pub pipeline_ref: String,
    pub error: String,
}

/// `GET /api/admin/airway/config/{source_kind}/preview
/// ?contract_policy=<p>&environment=<e>`.
///
/// `FleetOk`, like every other route on this surface: the scan is Postgres
/// queries and no filesystem access at all. See the module doc for why the
/// compiled rows win over the working copy, and why the scan is fenced to the
/// caller's platform scope.
pub async fn preview_policy(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(source_kind): Path<String>,
    Query(query): Query<PreviewQuery>,
) -> Result<Json<PolicyPreviewResponse>, Response> {
    if !KNOWN_SOURCE_KINDS.contains(&source_kind.as_str()) {
        return Err(error_body(
            StatusCode::BAD_REQUEST,
            "unknown_source_kind",
            Some(format!(
                "unknown airway source kind `{source_kind}` \
                 (expected one of {KNOWN_SOURCE_KINDS:?})"
            )),
        ));
    }
    // Same parser the write endpoints use, so a spelling this preview accepts
    // is exactly one a `PUT` would store — and a typo is refused here too,
    // rather than previewing `permissive` under a `require_declared` label.
    let admission = AirwayAdmission::from_strings(
        query.contract_policy.as_deref(),
        query.environment.as_deref(),
    )
    .map_err(|e| {
        error_body(
            StatusCode::BAD_REQUEST,
            "invalid_admission_policy",
            Some(e.to_string()),
        )
    })?;

    let db = connect().await?;
    // The same LENIENT read-path filter `get_config` takes, from the module
    // that owns the rule — not a second spelling of it. `Err` collapsing to
    // "don't filter" is deliberate and is argued there: a read prefers showing
    // rows to presenting an empty console as the truth, while this surface's
    // writes fail closed.
    let scope = scope_org_filter(&db, &actor).await;
    let scan = scan_pipelines(
        &db,
        &source_kind,
        admission.contract_policy,
        admission.environment,
        scope.as_deref(),
    )
    .await
    .map_err(db_err)?;
    Ok(Json(PolicyPreviewResponse {
        source_kind,
        contract_policy: policy_label(admission.contract_policy).to_string(),
        environment: environment_label(admission.environment).to_string(),
        resources: scan.resources,
        unevaluated: scan.unevaluated,
        uncompiled_workspaces: scan.uncompiled_workspaces,
        out_of_scope_pipelines: scan.out_of_scope_pipelines,
    }))
}

/// The wire spelling `ContractPolicy`'s `FromStr` accepts, echoed back so a
/// caller that sent nothing still learns which policy was previewed.
/// Exhaustive on purpose: a new airway variant must be spelled here rather than
/// falling into a catch-all that reports the wrong policy.
fn policy_label(policy: ContractPolicy) -> &'static str {
    match policy {
        ContractPolicy::Permissive => "permissive",
        ContractPolicy::RequireDeclared => "require_declared",
        ContractPolicy::ForbidOpaque => "forbid_opaque",
    }
}

/// The wire spelling `Environment`'s `FromStr` accepts. Exhaustive for the
/// same reason as [`policy_label`] — a preview labelled with the wrong
/// environment is worse than none, since the client trusts this echo to decide
/// whether the body on screen describes the save it is about to make.
fn environment_label(environment: Environment) -> &'static str {
    match environment {
        Environment::Production => "production",
        Environment::Sandbox => "sandbox",
    }
}

// The verdict (pure)

/// Score every resource a connector exposes against `policy`.
///
/// The per-resource restatement of airway's `ContractPolicy::check`, which
/// answers only "does this whole connector build". Same rules, same order of
/// concerns — `permissive` never refuses, uncursored resources are exempt,
/// an orphaned declaration is an error above `permissive` — restated so the
/// operator sees which rows are the problem. `preview_tests.rs` asserts the two
/// agree on the yes/no, which is what keeps this from drifting.
pub(crate) fn verdicts(
    source_kind: &str,
    pipeline_ref: &str,
    resources: &[ResourceInfo],
    contracts: &HashMap<String, SourceContract>,
    policy: ContractPolicy,
) -> Vec<ResourceVerdict> {
    let mut out: Vec<ResourceVerdict> = resources
        .iter()
        .map(|resource| resource_verdict(source_kind, pipeline_ref, resource, contracts, policy))
        .collect();
    out.sort_by(|a, b| a.resource.cmp(&b.resource));
    out.extend(orphan_verdicts(
        source_kind,
        pipeline_ref,
        resources,
        contracts,
        policy,
    ));
    out
}

/// A refusal: why, and whether the operator can act on it.
struct Failure {
    reason: String,
    not_fixable_here: bool,
}

fn resource_verdict(
    source_kind: &str,
    pipeline_ref: &str,
    resource: &ResourceInfo,
    contracts: &HashMap<String, SourceContract>,
    policy: ContractPolicy,
) -> ResourceVerdict {
    let declared = contracts.get(&resource.name);
    let failure = failure_for(source_kind, resource, declared, policy);
    ResourceVerdict {
        pipeline_ref: pipeline_ref.to_string(),
        resource: resource.name.clone(),
        mutability: mutability_label(declared).to_string(),
        passes: failure.is_none(),
        reason: failure.as_ref().map(|f| f.reason.clone()),
        not_fixable_here: failure.is_some_and(|f| f.not_fixable_here),
    }
}

/// `None` = admitted. Mirrors `ContractPolicy::check`'s two refusals.
fn failure_for(
    source_kind: &str,
    resource: &ResourceInfo,
    declared: Option<&SourceContract>,
    policy: ContractPolicy,
) -> Option<Failure> {
    // `permissive` never returns `Err` upstream — it warns and succeeds — so
    // nothing can fail here, orphans included.
    if policy == ContractPolicy::Permissive {
        return None;
    }
    // Cursored only, exactly as upstream. A resource with no cursor has no
    // incremental window for a contract to constrain, so requiring one would
    // refuse resources with nothing to declare.
    resource.cursor_field.as_ref()?;

    let name = &resource.name;
    match declared {
        // **Always fixable since airway 0.1.24.** Every source kind Oxy knows
        // can now carry a declaration: `toast`, `quickbooks`, `weather` and
        // `netsuite` implement `contracts()` in Rust, and #105 added
        // `EndpointConfig::contract` so the ~24 `rest_api`-backed connectors
        // declare per endpoint. There is no longer a kind for which
        // "undeclared" names an action the operator cannot take, which is why
        // the kind allow-list that used to gate this is gone rather than
        // emptied.
        //
        // *Where* the declaration goes still differs per kind, though, so the
        // second sentence comes from [`declaration_site`] rather than being
        // one generic line that is wrong for somebody.
        None => Some(Failure {
            reason: format!(
                "`{name}` is cursored and declares no contract. `{}` requires one for every \
                 cursored resource. {}",
                policy_label(policy),
                declaration_site(source_kind, name),
            ),
            not_fixable_here: false,
        }),
        Some(contract)
            if policy == ContractPolicy::ForbidOpaque
                && matches!(contract.mutability(), Mutability::Opaque) =>
        {
            Some(Failure {
                reason: format!(
                    "`{name}` declares `opaque` mutability. `forbid_opaque` accepts only \
                     `immutable` or `versioned` — this was declared, not merely omitted, so the \
                     vendor fact itself is what the policy rejects. Leave `{source_kind}` on a \
                     looser policy unless the vendor really does expose a version."
                ),
                // Declared-opaque is a checked vendor fact, not a missing slot.
                // The operator's move — pick a policy this kind can meet — is
                // one they make in this very UI, so it is emphatically fixable
                // here. Flagging it "upstream" would send them to file a bug
                // against a connector that behaved correctly.
                not_fixable_here: false,
            })
        }
        Some(_) => None,
    }
}

/// Where a contract for `resource` actually goes, for `source_kind`.
///
/// The second sentence of the undeclared-resource diagnostic, and it has to be
/// per-kind: [`KNOWN_SOURCE_KINDS`](super::KNOWN_SOURCE_KINDS) splits into two
/// halves that declare in **different files**, so a single generic line is
/// necessarily wrong for one of them — and sending an operator to edit a file
/// that cannot carry the answer is the same failure `not_fixable_here` exists
/// to prevent, one level down.
///
/// * **`rest_api` is config-defined.** airway 0.1.24's #105 inlined the
///   declaration on `EndpointConfig::contract` — *singular*, one contract per
///   endpoint, **not** a name-keyed map on `RestApiConfig`. Upstream chose the
///   inline field so a contract rides its endpoint through the `--resources`
///   filter (a side map would not be filtered with it, and every unselected
///   resource would then read as an orphan). `source_factory::build_rest_api`
///   deserializes `RestApiConfig` straight out of `source.config`, so the field
///   is reachable from the pipeline's own `.airway.yml` — this is the one kind
///   an operator fixes without touching connector code.
/// * **Every other kind declares in Rust**, by implementing
///   `SourceConnector::contracts()`. Deliberately not a list of names: the
///   previous wording enumerated four connectors and went stale the next time
///   one was added. They have no YAML slot at all, and `EndpointConfig` does
///   not `deny_unknown_fields`, so a `contract:` key invented in one of *their*
///   pipeline files would be silently ignored rather than refused.
///
/// Both halves are *fixable* — this sets no flag; see
/// [`ResourceVerdict::not_fixable_here`].
fn declaration_site(source_kind: &str, resource: &str) -> String {
    if source_kind == "rest_api" {
        return format!(
            "Declare it in this pipeline's `.airway.yml`, on the `{resource}` entry under \
             `source.config.endpoints`: a `contract:` block with at least `mutability:` \
             (`immutable` / `versioned` / `opaque`). No connector change is needed."
        );
    }
    format!(
        "`{source_kind}` declares contracts in airway's Rust source, not in YAML: add \
         `{resource}` to that connector's `SourceConnector::contracts()`. Nothing in this \
         pipeline's `.airway.yml` reaches it."
    )
}

/// Contract names that answer to no resource.
///
/// Upstream, an orphan fails a tightened policy *before* any resource is
/// scored (`check_contracts()?` runs first), so a preview that skipped them
/// would report all-clear for a pipeline the policy halts. It also means the
/// resource the declaration was meant for is silently on the default. Empty
/// under `permissive`, which only warns.
///
/// Unreachable through today's factory — `contracts()` and `resources()` come
/// from the same connector's code, and upstream tests pin them together — so
/// this is a guard against a future connector, not a live case. airway 0.1.24
/// narrowed it further for `rest_api`: #105 inlined the declaration on
/// `EndpointConfig`, so a contract now rides its endpoint through the
/// `--resources` filter and an orphan is unrepresentable there rather than
/// merely detected (upstream's `selecting_a_resource_subset_leaves_no_orphans`).
///
/// **This is now the only thing that sets [`ResourceVerdict::not_fixable_here`]**
/// — see that field's doc for why the flag survived the removal of the
/// kind-level allow-list.
fn orphan_verdicts(
    source_kind: &str,
    pipeline_ref: &str,
    resources: &[ResourceInfo],
    contracts: &HashMap<String, SourceContract>,
    policy: ContractPolicy,
) -> Vec<ResourceVerdict> {
    if policy == ContractPolicy::Permissive {
        return Vec::new();
    }
    let known: HashSet<&str> = resources.iter().map(|r| r.name.as_str()).collect();
    let mut orphans: Vec<&String> = contracts
        .keys()
        .filter(|name| !known.contains(name.as_str()))
        .collect();
    orphans.sort();
    orphans
        .into_iter()
        .map(|name| ResourceVerdict {
            pipeline_ref: pipeline_ref.to_string(),
            resource: name.clone(),
            mutability: mutability_label(contracts.get(name)).to_string(),
            passes: false,
            reason: Some(format!(
                "`{source_kind}` declares a contract for `{name}`, which is not one of its \
                 resources. Above `permissive` that orphan is an error, and it also means the \
                 resource it was meant for is silently running on the default."
            )),
            // A typo in connector source; no Oxy-side setting reaches it.
            not_fixable_here: true,
        })
        .collect()
}

/// `undeclared` is deliberately **not** `opaque`.
///
/// Airway reaches `Opaque` for an undeclared resource through
/// `unwrap_or_default()`, and its own doc calls that "honestly labelled
/// `Opaque` ('we haven't said')". To an operator the two are different states:
/// one is a checked vendor fact, the other is a gap nobody has filled — and
/// `require_declared` rejects only the second. Blurring them here would make
/// the preview's own `not_fixable_here` column unreadable.
fn mutability_label(declared: Option<&SourceContract>) -> &'static str {
    match declared.map(SourceContract::mutability) {
        None => "undeclared",
        Some(Mutability::Immutable) => "immutable",
        Some(Mutability::Versioned { .. }) => "versioned",
        Some(Mutability::Opaque) => "opaque",
    }
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
