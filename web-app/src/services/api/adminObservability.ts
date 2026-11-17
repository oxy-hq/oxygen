import { apiClient } from "./axios";

/**
 * Admin → Observability: what an org costs us, and how its custom apps perform.
 *
 * Served by `crates/api-observability/src/admin`. Design, including why there
 * are no per-org infrastructure dollars:
 * `internal-docs/2026-11-16-admin-observability-design.md`.
 *
 * **`null` is not zero anywhere in these shapes.** A ratio over an empty window
 * is `null` because no traffic has no opinion about reliability, and an
 * app-side field is `null` when this deployment has no observability store at
 * all. The UI renders the two differently — a dash for the first, a named
 * notice for the second — and collapsing them is what makes an unmeasured org
 * read as an idle one.
 */

/** Window totals. Tokens are always real; dollars cover only priced models. */
export interface CostTotals {
  cost_usd: number;
  input_tokens: number;
  output_tokens: number;
  cache_creation_tokens: number;
  cache_read_tokens: number;
  run_count: number;
  /** Runs on a model with no known rate: tokens counted, dollars excluded. */
  unpriced_runs: number;
}

export interface CostDay {
  day: string;
  cost_usd: number;
  input_tokens: number;
  output_tokens: number;
  run_count: number;
}

/** One value of a dimension — a model, a workspace, a surface, an attendance class. */
export interface CostBucket {
  /** The raw value; stable, and what React keys off. */
  key: string;
  /** What to show — the workspace's name, or the key itself. */
  label: string;
  cost_usd: number;
  input_tokens: number;
  output_tokens: number;
  run_count: number;
  /** Fraction of the window's priced cost, or `null` when nothing was priced. */
  share: number | null;
}

export interface CostDerived {
  monthly_run_rate_usd: number | null;
  cost_per_run_usd: number | null;
  cache_hit_share: number | null;
  unattended_share: number | null;
}

/** The org a per-org response is about — enough to name it and build links. */
export interface OrgIdentity {
  org_id: string;
  name: string;
  slug: string;
}

export interface OrgCost {
  window_days: number;
  org: OrgIdentity;
  total: CostTotals;
  derived: CostDerived;
  by_day: CostDay[];
  /** The window before, day by day; `null` when it could not be read. */
  prior_by_day: CostDay[] | null;
  /** The prior window's priced spend; `null` alongside `prior_by_day`. */
  prior_cost_usd: number | null;
  by_model: CostBucket[];
  by_workspace: CostBucket[];
  by_surface: CostBucket[];
  by_attendance: CostBucket[];
}

export interface AppTotals {
  requests: number;
  failed: number;
  sessions: number;
  people: number;
  apps: number;
  bytes: number;
  host_calls: number;
  p50_ms: number;
  p95_ms: number;
  p99_ms: number;
  cold_start_p95_ms: number;
  /** Exact summed wall time; `wall_ms - init_ms` is the app's own code. */
  wall_ms: number;
  init_ms: number;
  failure_rate: number | null;
}

/** One failure class and cause. `share` is of ALL traffic, not of all failures. */
export interface AppFailure {
  outcome: string;
  error_kind: string;
  requests: number;
  share: number | null;
}

export interface AppDay {
  day: string;
  requests: number;
  failed: number;
  p95_ms: number;
  wall_ms: number;
  init_ms: number;
  failure_rate: number | null;
}

export interface AppRow {
  app_id: string;
  name: string;
  /** Empty when the app is no longer in the catalog but its events survive. */
  slug: string;
  requests: number;
  failed: number;
  sessions: number;
  bytes: number;
  p50_ms: number;
  p95_ms: number;
  cold_start_p95_ms: number;
  failure_rate: number | null;
}

export interface AppRoute {
  app_id: string;
  app_name: string;
  route: string;
  requests: number;
  failed: number;
  p95_ms: number;
}

export interface OrgApps {
  window_days: number;
  /** False when no observability store is configured — nothing below was measured. */
  measured: boolean;
  total: AppTotals;
  failures: AppFailure[];
  by_day: AppDay[];
  by_app: AppRow[];
  routes: AppRoute[];
}

/**
 * How far we can stand behind a meter's figure.
 *
 * `direct` — this org is the only consumer and the rate is public.
 * `apportioned` — a share of a bill many orgs cause, split by a measured driver.
 * `unmeasured` — consumed, and not measurable here yet; named rather than hidden.
 */
export type Attribution = "direct" | "apportioned" | "unmeasured";

/** One resource the org consumes. */
export interface Meter {
  key: string;
  label: string;
  /** `null` when unmeasured — never 0, which would read as "uses none". */
  quantity: number | null;
  unit: string;
  cost_usd: number | null;
  attribution: Attribution;
  /** Always present for anything not `direct`. */
  note: string | null;
}

/** What every figure on the Resources panel assumes. */
export interface ResourceBasis {
  /** The isolate ceiling GB·s are computed at. `null` when disabled. */
  provisioned_memory_mb: number | null;
  fleet_cost_usd_month: number | null;
  fleet_cost_env: string;
  storage_usd_per_gb_month: number;
  egress_usd_per_gb: number;
}

export interface OrgResources {
  window_days: number;
  basis: ResourceBasis;
  meters: Meter[];
  /** Priced meters summed. **Excludes LLM**, which the Cost panel owns. */
  infra_cost_usd: number | null;
  /** True when something could not be measured, so the total is a floor. */
  partial: boolean;
}

/** One org's row on the fleet list. App-side fields are `null` when unmeasured. */
export interface FleetRow {
  org_id: string;
  org_name: string;
  org_slug: string;
  /** Priced LLM spend over the window. */
  cost_usd: number;
  run_count: number;
  unpriced_runs: number;
  input_tokens: number;
  output_tokens: number;
  cost_per_run_usd: number | null;
  monthly_run_rate_usd: number | null;
  unattended_run_share: number | null;
  cache_hit_share: number | null;
  requests: number | null;
  sessions: number | null;
  people: number | null;
  apps: number | null;
  p95_ms: number | null;
  failure_rate: number | null;
  /** Provisioned GB·s of function compute. */
  function_compute_gbs: number | null;
  /** Decimal GB, the unit egress is billed in. */
  egress_gb: number | null;
  /** `null` means no storage readings, not an empty bucket. */
  storage_gib_month: number | null;
  /** App storage at S3 list. `null` when there are no readings. */
  storage_usd: number | null;
  /** App egress at AWS list. `null` when apps are unmeasured. */
  egress_usd: number | null;
  /** Share of the operator-supplied serving bill. `null` without one. */
  compute_usd: number | null;
  /** LLM spend plus every priced component. */
  cost_to_serve_usd: number;
  /**
   * The same total over the window before this one. `null` when the prior
   * window could not be read; `0` when the org cost nothing then.
   */
  prior_cost_to_serve_usd: number | null;
}

export interface Fleet {
  window_days: number;
  apps_measured: boolean;
  /**
   * False when the storage read itself failed — a fault, not the sweeper
   * being behind. The two look identical in the rows (`null` either way), so
   * the page has to be told which it is.
   */
  storage_read: boolean;
  /** True when an operator-supplied fleet bill prices function compute. */
  compute_priced: boolean;
  /**
   * False when there is no prior window to compare against — it was past
   * retention, a read of it failed, or it priced different components. No
   * comparison is drawn.
   */
  prior_read: boolean;
  /** False when the window before reaches past retention: nothing failed. */
  prior_retained: boolean;
  /** What a compute figure assumes. */
  provisioned_memory_mb: number | null;
  rows: FleetRow[];
}

export const AdminObservabilityService = {
  async fleet(days: number): Promise<Fleet> {
    const res = await apiClient.get<Fleet>("/admin/observability/fleet", { params: { days } });
    return res.data;
  },

  async orgCost(orgId: string, days: number): Promise<OrgCost> {
    const res = await apiClient.get<OrgCost>(`/admin/observability/orgs/${orgId}/cost`, {
      params: { days }
    });
    return res.data;
  },

  async orgApps(orgId: string, days: number): Promise<OrgApps> {
    const res = await apiClient.get<OrgApps>(`/admin/observability/orgs/${orgId}/apps`, {
      params: { days }
    });
    return res.data;
  },

  async orgResources(orgId: string, days: number): Promise<OrgResources> {
    const res = await apiClient.get<OrgResources>(`/admin/observability/orgs/${orgId}/resources`, {
      params: { days }
    });
    return res.data;
  }
};
