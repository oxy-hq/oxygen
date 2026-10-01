import { apiClient } from "./axios";

/**
 * `/api/admin/airway/config` — staff read/write/preview of Airway's
 * per-source-kind admission policy (`airway_source_config`: a global row per
 * kind, plus optional per-workspace overrides). Gated server-side by
 * `Action::PlatformOperate`, not by strict `OXY_OWNER`; every route returning
 * or writing per-tenant rows additionally fences on the caller's platform
 * scope, so a bounded operator sees and writes only their own tenants'
 * overrides — and previews only their own tenants' pipelines, with the
 * withheld remainder reported as {@link
 * AirwayPolicyPreviewResponse.out_of_scope_pipelines}.
 *
 * Wire casing is `snake_case` throughout — none of the backing Rust structs
 * (`crates/app/src/server/api/admin/airway_config/{handlers,preview}.rs`)
 * carry `#[serde(rename_all)]`, so JSON keys are the literal Rust field
 * names. Do not camelCase anything here.
 */

// Shared wire types

/** Wire spellings `AirwayAdmission::from_strings` accepts for `contract_policy`. */
export type AirwayContractPolicy = "permissive" | "require_declared" | "forbid_opaque";

/** Wire spellings `AirwayAdmission::from_strings` accepts for `environment`. */
export type AirwayEnvironment = "production" | "sandbox";

const AIRWAY_CONTRACT_POLICIES: readonly AirwayContractPolicy[] = [
  "permissive",
  "require_declared",
  "forbid_opaque"
];

const AIRWAY_ENVIRONMENTS: readonly AirwayEnvironment[] = ["production", "sandbox"];

/**
 * A stored `contract_policy` / `environment`, narrowed to a spelling this
 * build knows — or `null` when it is not one.
 *
 * A **stored** value is not the same type as one this UI can produce. The
 * column is deliberately free text (`m20260805_000001_airway_source_config`
 * argues why: the valid set lives in the external airway crate and moves, so a
 * CHECK constraint would go stale in the rejecting direction), and nothing but
 * `AirwayAdmission::from_strings` validates it. So a row written by raw SQL, or
 * by a build that knew a spelling this one does not, arrives here as a string
 * outside the union.
 *
 * Typing it as the union anyway is a lie the compiler then defends: the value
 * flows into a `<Select>` that has no matching item, renders a blank
 * `SelectValue`, and the mismatch only surfaces as a 400 on `PUT`. Narrow at
 * the boundary and let the caller decide what to say about an unknown.
 */
export function asContractPolicy(value: string | null | undefined): AirwayContractPolicy | null {
  return isKnown(value, AIRWAY_CONTRACT_POLICIES) ? value : null;
}

export function asEnvironment(value: string | null | undefined): AirwayEnvironment | null {
  return isKnown(value, AIRWAY_ENVIRONMENTS) ? value : null;
}

function isKnown<T extends string>(
  value: string | null | undefined,
  known: readonly T[]
): value is T {
  return typeof value === "string" && (known as readonly string[]).includes(value);
}

/**
 * A config row's two editable fields plus its freshness. Shared shape for
 * both the global row and a workspace override — `updated_at` is what lets a
 * card show staleness without a second call.
 *
 * Both policy fields are typed as the **raw stored string**, not the accepted
 * union — see {@link asContractPolicy} for why the two are different types.
 * Narrow with `asContractPolicy` / `asEnvironment` before feeding a select or a
 * comparison; render the raw value when displaying what is actually stored.
 */
export interface AirwayConfigValues {
  /** `null` means "inherit" — no policy stored on this row. */
  contract_policy: string | null;
  /** `null` means "inherit" — no environment stored on this row. */
  environment: string | null;
  updated_at: string;
}

export interface AirwayWorkspaceOverride {
  workspace_id: string;
  /** `null` when the workspace row is gone — the override still surfaces, just without a display name. */
  workspace_name: string | null;
  values: AirwayConfigValues;
}

export interface AirwaySourceKindConfig {
  source_kind: string;
  /** The `workspace_id IS NULL` row, if one exists. `null` means no row has ever been written for this kind. */
  global: AirwayConfigValues | null;
  overrides: AirwayWorkspaceOverride[];
}

export interface AirwayConfigResponse {
  /** Every known source kind (see `KNOWN_SOURCE_KINDS` server-side), even ones with no config row yet. */
  kinds: AirwaySourceKindConfig[];
}

/**
 * Body for both `PUT` routes (global and per-workspace override). `null` on
 * either field clears it back to "inherit" — this is a replace, not a patch.
 * There is no partial-update mode: always send both fields, even if only one
 * is changing, or the other silently reverts to inherit.
 */
export interface UpsertAirwayConfigBody {
  contract_policy: AirwayContractPolicy | null;
  environment: AirwayEnvironment | null;
}

// Preview

/**
 * `immutable` / `versioned` / `opaque`, or `undeclared`. `undeclared` is
 * deliberately distinct from `opaque`: one is a checked vendor fact, the
 * other is a gap nobody has filled, and only the second is fixable by
 * declaring a contract.
 */
type AirwayMutability = "immutable" | "versioned" | "opaque" | "undeclared";

export interface AirwayResourceVerdict {
  /**
   * `{workspace_id}:{workspace-relative path}` — qualified because preview is
   * cross-tenant. Split on the first `:` before displaying; do not show the
   * raw UUID-prefixed string.
   */
  pipeline_ref: string;
  resource: string;
  mutability: AirwayMutability;
  passes: boolean;
  /** Why it fails, in the operator's terms. `null` when it passes. */
  reason: string | null;
  /**
   * True when the failure cannot be fixed from Oxy — no setting on the admin
   * surface, and nothing in the pipeline's YAML, reaches it.
   *
   * Since airway 0.1.24 the only cause is an *orphaned* contract (declared for
   * a name that is not one of the connector's resources). The former cause — a
   * source kind with no way to declare at all — is gone: #105 added a
   * per-endpoint `EndpointConfig::contract` field, so every kind can declare.
   *
   * Note `reason` names *where* the declaration goes, and that differs per
   * kind: `rest_api` declares on the endpoint in the pipeline's own
   * `.airway.yml`, while every other kind declares in airway's Rust source.
   * (Named as a split rather than a list of connectors — the enumeration that
   * used to sit here went stale each time a kind was added.) Render `reason`
   * verbatim rather than substituting a generic line of copy for it.
   */
  not_fixable_here: boolean;
}

export interface AirwayUnevaluatedPipeline {
  /** Always a real `{workspace_id}:{path}` — the server never puts anything synthetic here. */
  pipeline_ref: string;
  error: string;
}

export interface AirwayPolicyPreviewResponse {
  source_kind: string;
  /**
   * The policy actually previewed — echoed back so a caller that sent nothing
   * still learns the default, and so `computeSaveGate` can *verify* that the
   * body on screen describes the save about to happen rather than inferring it
   * from a cache key.
   */
  contract_policy: AirwayContractPolicy;
  /** The environment actually previewed. Same echo-and-verify contract as `contract_policy`. */
  environment: AirwayEnvironment;
  resources: AirwayResourceVerdict[];
  /**
   * Pipelines whose connector could not be built. Reported, never silently
   * dropped. **This is the coverage gap the save gate keys on**: every entry
   * names a real `.airway.yml` whose verdict is unknown.
   */
  unevaluated: AirwayUnevaluatedPipeline[];
  /**
   * How many workspaces have no promoted revision at all — nothing compiled,
   * so no pipelines of this kind to check.
   *
   * Deliberately a **separate field, not an `unevaluated` entry**, which is
   * where the server used to fold it. Reported (hiding it once let an operator
   * believe coverage was complete) but never gated on: on any real deployment
   * at least one workspace has never compiled, so gating on it made every save
   * confirm, forever — a confirmation that always fires is one operators learn
   * to click through.
   */
  uncompiled_workspaces: number;
  /**
   * How many compiled pipelines the caller's platform scope kept out of this
   * answer. `0` for a Global Owner or a `scope_all` grant.
   *
   * The scan is fenced to the orgs the caller reaches — a `pipeline_ref` names
   * a tenant's workspace id and a real file path, so an unfenced scan let a
   * two-org operator enumerate every tenant's airway pipelines. But the
   * **global** row a bounded operator can still write is fleet-wide, so a
   * preview that quietly omitted the rest would show a short, clean list for a
   * change that reaches all of it. This number is what makes the omission
   * visible; render it wherever the verdict counts are read.
   *
   * A count of pipelines of **every** source kind, so it over-states the
   * remainder for any one kind — `source.kind` lives inside the compiled JSON
   * the fence keeps out of the server's hands. Say "pipelines", never
   * "`<kind>` pipelines". Like `uncompiled_workspaces`, it is reported and
   * deliberately **not** gated on: it is non-zero for every request a bounded
   * grant ever makes.
   */
  out_of_scope_pipelines: number;
}

export const AirwayConfigService = {
  /** `GET /admin/airway/config`. */
  async getConfig(): Promise<AirwayConfigResponse> {
    const res = await apiClient.get<AirwayConfigResponse>("/admin/airway/config");
    return res.data;
  },

  /** `PUT /admin/airway/config/{source_kind}` — create or replace the global row. */
  async upsertGlobal(sourceKind: string, body: UpsertAirwayConfigBody): Promise<void> {
    await apiClient.put(`/admin/airway/config/${sourceKind}`, body);
  },

  /** `DELETE /admin/airway/config/{source_kind}` — remove the global row entirely (idempotent). */
  async deleteGlobal(sourceKind: string): Promise<void> {
    await apiClient.delete(`/admin/airway/config/${sourceKind}`);
  },

  /** `PUT /admin/airway/config/{source_kind}/workspaces/{workspace_id}` — create or replace the override. */
  async upsertOverride(
    sourceKind: string,
    workspaceId: string,
    body: UpsertAirwayConfigBody
  ): Promise<void> {
    await apiClient.put(`/admin/airway/config/${sourceKind}/workspaces/${workspaceId}`, body);
  },

  /** `DELETE /admin/airway/config/{source_kind}/workspaces/{workspace_id}` — remove the override (idempotent). */
  async deleteOverride(sourceKind: string, workspaceId: string): Promise<void> {
    await apiClient.delete(`/admin/airway/config/${sourceKind}/workspaces/${workspaceId}`);
  },

  /**
   * `GET /admin/airway/config/{source_kind}/preview?contract_policy=<p>&environment=<e>`.
   * Omitting either previews airway's default (`permissive` / `production`).
   *
   * **Send both admission axes.** `environment` is not cosmetic — the source
   * factory refuses connectors under `sandbox` that it has no arm to apply, so
   * a scan run under `production` does not describe a `sandbox` save.
   *
   * Scans every compiled pipeline of `sourceKind` across every workspace, so
   * this is not free — never call it on mount or in a loop over source
   * kinds; it must always be an explicit, on-demand action.
   *
   * Both axes are plain `string`, not the accepted unions: an override
   * previews under the value it will *inherit*, which comes from a stored row
   * and may therefore be a spelling this build doesn't know. Sending it
   * unchanged makes the server answer `400`, which the save gate reads as
   * `unknown` and confirms — the safe direction. Silently substituting a
   * default here would score the wrong policy and call it clean.
   */
  async previewPolicy(
    sourceKind: string,
    contractPolicy?: string,
    environment?: string
  ): Promise<AirwayPolicyPreviewResponse> {
    const params: Record<string, string> = {};
    if (contractPolicy) params.contract_policy = contractPolicy;
    if (environment) params.environment = environment;
    const res = await apiClient.get<AirwayPolicyPreviewResponse>(
      `/admin/airway/config/${sourceKind}/preview`,
      { params: Object.keys(params).length > 0 ? params : undefined }
    );
    return res.data;
  }
};

// Deployment (operational) tier

/**
 * airway's process-wide `GlobalConfig`, stored in the singleton
 * `airway_deployment_config` row. Eight settings over eleven fields (`tls` is
 * one setting spread over four).
 *
 * **`null` means "airway's built-in default", never 0 and never "disabled".**
 * An input left empty must serialize to `null`, and a `null` must render as
 * "airway default" — not as `0`, not as an empty cell.
 *
 * Durations carry their unit in the field name and are stored in it:
 * `timeout_secs` / `retry_max_delay_secs` in whole seconds,
 * `retry_initial_delay_ms` in milliseconds. Do not convert anywhere; the unit
 * is the same from the column to the field label.
 */
export interface AirwayDeploymentValues {
  timeout_secs: number | null;
  /** One count for both airway transports — there is no separate retry-layer count. */
  max_retries: number | null;
  user_agent: string | null;
  retry_initial_delay_ms: number | null;
  retry_max_delay_secs: number | null;
  retry_backoff_factor: number | null;
  /**
   * A **floor** under every resource's declared cursor lag, in whole seconds.
   *
   * `null` is *no floor* and is the only way to say that: `0` raises no
   * resource's lag, so airway refuses it rather than reading it as absence and
   * the API answers `400`. Do not "helpfully" send `0` for an emptied input —
   * that is the one field where the usual empty-to-`null` rule is not merely
   * tidy but the difference between saving and a rejected request.
   */
  cursor_lag_floor_secs: number | null;
  tls_ca_cert: string | null;
  tls_client_cert: string | null;
  tls_client_key_file: string | null;
  tls_danger_accept_invalid_certs: boolean | null;
}

/**
 * Which process the `installed` half of the response describes.
 *
 * `installed` is read from one process's `OnceLock`. On a split fleet the
 * replica answering this request is usually NOT the worker that installed, so
 * this must be surfaced to the operator rather than dropped — a `null`
 * `installed` from a `serve` replica says nothing about the deployment.
 */
export interface AirwayInstalledScope {
  /** Always `"answering_process"`. Never read this payload as deployment-wide. */
  scope: string;
  /** `OXY_ROLE` as the answering process resolved it. */
  process_role: string;
  pid: number;
  hostname: string | null;
  /** Whether a node in this role runs airway pipelines at all. */
  process_runs_airway: boolean;
  installed_in_this_process: boolean;
}

type AirwayDriftStatus = "in_sync" | "drifted" | "unknown";

/**
 * `unknown` is a real answer and must never render as a green tick: "this
 * replica installed nothing" and "this replica installed exactly what is
 * configured" are different facts.
 */
export interface AirwayDriftReport {
  status: AirwayDriftStatus;
  /** airway key names of the settings that differ. Empty unless `drifted`. */
  fields: string[];
  reason: "not_installed_in_this_process" | "configured_values_invalid" | null;
}

export interface AirwayDeploymentConfigResponse {
  configured: AirwayDeploymentValues;
  /** Distinguishes "a row exists with everything cleared" from "never configured". */
  configured_row_exists: boolean;
  updated_at: string | null;
  installed: AirwayDeploymentValues | null;
  installed_scope: AirwayInstalledScope;
  drift: AirwayDriftReport;
}

export const AirwayDeploymentService = {
  /** `GET /admin/airway/deployment-config`. */
  async get(): Promise<AirwayDeploymentConfigResponse> {
    const res = await apiClient.get<AirwayDeploymentConfigResponse>(
      "/admin/airway/deployment-config"
    );
    return res.data;
  },

  /**
   * `PUT /admin/airway/deployment-config` — a replace, not a patch. Send all
   * eleven fields; `null` clears a setting back to airway's default.
   *
   * **This does not take effect until the airway worker process restarts.**
   * airway's install is one-shot per process, so a successful save changes
   * what the next process installs and nothing about the running one.
   */
  async upsert(body: AirwayDeploymentValues): Promise<void> {
    await apiClient.put("/admin/airway/deployment-config", body);
  },

  /** `DELETE /admin/airway/deployment-config` — remove the row entirely (idempotent). */
  async clear(): Promise<void> {
    await apiClient.delete("/admin/airway/deployment-config");
  }
};
