import type { WorkspaceRole } from "@/types/organization";

type GitMode = "none" | "local" | "connected";

export interface GitCapabilities {
  can_commit: boolean;
  can_browse_history: boolean;
  can_reset_to_commit: boolean;
  can_switch_branch: boolean;
  can_diff: boolean;
  can_push: boolean;
  can_pull: boolean;
  can_fetch: boolean;
  can_force_push: boolean;
  can_rebase: boolean;
  can_open_pr: boolean;
  auto_feature_branch_on_protected: boolean;
}

export interface Workspace {
  id: string;
  name: string;
  workspace_id: string;
  active_branch: WorkspaceBranch | null;
  created_at: string;
  updated_at: string;

  workspace_error?: string;
  git_mode: GitMode;
  capabilities: GitCapabilities;
  default_branch: string;
  protected_branches: string[];

  /** True when this workspace is in local mode and has no config.yml yet. */
  requires_local_setup?: boolean;

  /** Authenticated user's effective role in this workspace. Optional so
   * existing callers that build a partial Workspace don't have to fabricate
   * a value; consumers that need the role should fall back to "viewer". */
  current_user_role?: WorkspaceRole;

  /** Namespace for per-workspace browser state (onboarding wizard
   * localStorage). UUID in cloud; `local:{path-hash}` in local — see
   * `compute_workspace_storage_key` in the Rust side for the contract.
   * Optional only because some legacy callers fabricate partial
   * Workspaces; real server responses always populate it. */
  storage_key?: string;
}

type BranchOrigin = "local_only" | "remote_only" | "both";

export interface WorkspaceBranch {
  name: string;
  revision: string;
  id: string;
  created_at: string;
  updated_at: string;
  branch_type: "local" | "remote";
  /** Where this branch lives. Drives badges + the switch flow:
   *  `remote_only` requires the server to create a local tracking branch
   *  before the worktree can be created. */
  origin: BranchOrigin;
}

export interface WorkspaceBranchesResponse {
  branches: WorkspaceBranch[];
}

/**
 * Where a workspace preview is. Only `ready` is opened. `stale` means the
 * branch head moved since this revision compiled — Refresh makes a new one.
 */
export type WorkspacePreviewStatus = "compiling" | "ready" | "failed" | "stale";

/**
 * A branch of the workspace compiled so the real product can be opened on it,
 * on real data, without it being live. Served by
 * `GET /api/workspaces/{id}/previews`.
 *
 * Timestamps arrive as ISO-8601 UTC strings; read them through
 * `parseUtcTimestamp`, never `new Date(..)` directly.
 */
/** Status of the Airway pipeline-change analysis for a preview's current revision. */
export type PreviewChecksStatus = "pending" | "done" | "failed";

/**
 * The cheap, embedded verdict carried on every list/create/refresh row —
 * `null` when no analyze run exists yet for the preview's current revision
 * (still compiling, or the analyzer hasn't picked it up). The per-pipeline
 * detail behind this summary is a separate fetch: `GET …/previews/checks`.
 */
export interface PreviewChecksSummary {
  status: PreviewChecksStatus;
  needs_reset: number;
  warnings: number;
  /** Changed transforms (automations), regardless of `auto`/`manual` build. */
  transforms: number;
}

export interface WorkspacePreview {
  branch: string;
  status: WorkspacePreviewStatus;
  /**
   * The immutable staging revision this preview serves — what a preview link
   * pins (`?preview=<revision_id>`). Null until the first compile lands.
   */
  revision_id: string | null;
  /** The commit the revision was compiled from; null until known. */
  sha: string | null;
  /** The compile error when `status` is `failed`. */
  error: string | null;
  created_by: { id: string; name: string } | null;
  updated_at: string;
  compiled_at: string | null;
  /** See `PreviewChecksSummary`. */
  checks: PreviewChecksSummary | null;
}

export interface WorkspacePreviewListResponse {
  items: WorkspacePreview[];
}

/** The `202` body of create and refresh: the row as it stands after the request. */
export interface WorkspacePreviewItemResponse {
  item: WorkspacePreview;
}

// ── Checks (per-pipeline detail) ────────────────────────────────────────────

/** Whether a pipeline definition was added, modified or removed on the branch. */
export type PreviewPipelineChange = "added" | "modified" | "removed";

/** How severe a pipeline's (or a single finding's) change is for prod. */
export type PreviewCheckVerdict = "additive" | "warning" | "needs_reset";

/** The full set the analyzer emits — see `internal-docs` for what each detects. */
export type PreviewCheckFindingKind =
  | "ResourceAdded"
  | "TableAdded"
  | "ColumnAdded"
  | "ConfigOnly"
  | "PipelineRenamed"
  | "DestinationMoved"
  | "SchemaSeparatorChanged"
  | "SourceKindChanged"
  | "WriteDispositionChanged"
  | "PrimaryKeyChanged"
  | "TableRenamed"
  | "ColumnTypeChanged"
  | "ColumnRemoved"
  | "ResourceRemoved"
  | "PipelineRemoved"
  | "LiveDrift"
  | "Unevaluated";

export interface PreviewCheckFinding {
  kind: PreviewCheckFindingKind;
  verdict: PreviewCheckVerdict;
  /** Human-readable description of what changed, e.g. "orders: append → merge". */
  detail: string;
  /** What to do in prod before this change ships. `null` for `additive` — nothing to do. */
  prod_action: string | null;
}

export interface PreviewPipelineCheck {
  name: string;
  file_path: string;
  change: PreviewPipelineChange;
  verdict: PreviewCheckVerdict;
  findings: PreviewCheckFinding[];
}

// ── Transform builds (S10, per-branch checks) ───────────────────────────

/** Whether a pure-Airhouse transform definition was added or modified on the branch. */
export type PreviewTransformChange = "added" | "modified";

/**
 * `auto` = built in the preview and compared with live automatically; `manual`
 * = an operator has to run it by hand (`reason` says why — e.g. it needs
 * variables the analyzer can't supply, or writes couldn't be scoped).
 */
export type PreviewTransformBuildMode = "auto" | "manual";

export interface PreviewTransformCheck {
  name: string;
  file_path: string;
  change: PreviewTransformChange;
  build: PreviewTransformBuildMode;
  /**
   * Why a `manual` transform can't build itself, or why an `auto` one has no
   * `build_run_id` yet (e.g. "builds skipped: …", "needs variables: …").
   * `null` for an `auto` transform that queued cleanly.
   */
  reason: string | null;
  /** The queued `transform_build` run, once one exists for this transform. */
  build_run_id: string | null;
}

/** `GET /api/{workspace_id}/previews/checks?branch=<b>` */
export interface PreviewChecksResponse {
  branch: string;
  /** `null` while the staging revision is still compiling (`status: "pending"`). */
  revision_id: string | null;
  status: PreviewChecksStatus;
  error: string | null;
  pipelines: PreviewPipelineCheck[];
  /** See `PreviewTransformCheck`. */
  transforms: PreviewTransformCheck[];
}

// ── Held procedure runs ──────────────────────────────────────────────────

export type PreviewRunKind = "procedure" | "transform_build" | "compare" | "airway_sample";
export type PreviewRunState = "queued" | "running" | "finished";
export type PreviewRunOutcome = "succeeded" | "failed" | "cancelled";

/** An `airway_sample` run's date window: RFC-3339, `[from, to)`. */
export interface PreviewRunWindow {
  from: string;
  to: string;
}

/** `POST /api/{workspace_id}/previews/runs` request body. */
export interface StartPreviewRunRequest {
  branch: string;
  kind: PreviewRunKind;
  ref: string;
  variables?: Record<string, unknown>;
  /**
   * Default `false`. `true` reads live tables even where the preview holds a
   * copy — writes still land in the preview either way. Omit the key rather
   * than sending an explicit `false`.
   */
  read_live_only?: boolean;
  /**
   * `airway_sample` only. Omitted = the last 7 days, and only meaningful for
   * a windowed source (Toast, QuickBooks) — sending one on a source with no
   * date window is refused (`window_not_supported`).
   */
  window?: PreviewRunWindow;
  /** `airway_sample` only — which of the source's resources to sample. */
  resources?: string[];
}

/** `202` body of `POST /api/{workspace_id}/previews/runs`. */
export interface StartPreviewRunResponse {
  run_id: string;
  state: PreviewRunState;
}

/** One row of `GET /api/{workspace_id}/previews/runs?branch=<b>`. */
export interface PreviewRunSummary {
  run_id: string;
  branch: string;
  kind: PreviewRunKind;
  target_ref: string;
  revision_id: string;
  state: PreviewRunState;
  outcome: PreviewRunOutcome | null;
  /** Number of writes held (would-have-written) so far in this run. Always 0 for `compare`. */
  held_count: number;
  requested_by: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
  /**
   * A `compare`'s `transform_build`, or a `transform_build`'s analyze run
   * (which the runs list never lists — such a build reads as top-level).
   * `null` for a `procedure` run.
   */
  parent_run_id: string | null;
}

export type PreviewRunStepKind = "execute_sql" | "http_request" | "airway" | "agent" | "other";
export type PreviewRunStepStatus = "succeeded" | "failed" | "held" | "running" | "pending";

/**
 * What a held step would have done, mirroring the step result's `preview`
 * note. `sql` is only populated for SQL steps — the rendered statement that
 * was never sent.
 */
export interface PreviewRunHeldInfo {
  verb: string;
  targets: string[];
  reason: string;
  sql: string | null;
}

/** One table a redirected step's SQL touched, live name paired with the preview copy it hit instead. */
export interface PreviewRunRedirectedTable {
  live: string;
  preview: string;
}

/** One live table the preview copied on demand to serve a redirected read or write. */
export interface PreviewRunRedirectedCopy {
  live: string;
  /** `partial` = the live table was over `OXY_PREVIEW_COW_MAX_ROWS`: the copy started empty. */
  state: "shadow" | "partial";
}

/**
 * Set when a step sent managed-Airhouse SQL into the preview's own schemas
 * instead of holding it — the step's `status` is `succeeded`, not `held`, and
 * it does not count toward `held_count`. Merged across a loop's iterations,
 * each entry appearing once.
 */
export interface PreviewRunRedirected {
  writes: PreviewRunRedirectedTable[];
  reads: PreviewRunRedirectedTable[];
  copies: PreviewRunRedirectedCopy[];
}

export interface PreviewRunStep {
  name: string;
  kind: PreviewRunStepKind;
  status: PreviewRunStepStatus;
  held: PreviewRunHeldInfo | null;
  /** `null` unless this step's reads/writes were redirected to the preview's copies. */
  redirected: PreviewRunRedirected | null;
}

/** One column this compare found retyped between live and the preview's build. */
export interface PreviewCompareColumnRetype {
  column: string;
  live: string;
  preview: string;
}

/**
 * One table a `compare` run diffed. Counts and names only — never a row
 * value; "show rows" is a separate, out-of-scope action that would run the
 * `EXCEPT ALL` bodies live, bounded, on demand.
 */
export interface PreviewCompareTable {
  table: string;
  /** `null` = no live table exists (the build created it; every row is `only_in_preview`). */
  live_rows: number | null;
  preview_rows: number | null;
  equal: boolean;
  /** `null` = not computed: over `OXY_PREVIEW_DIFF_MAX_ROWS` (`skipped_reason` says so). */
  only_in_preview: number | null;
  /** `null` also when the table is `partial` (the copy started empty; not meaningful). */
  only_in_live: number | null;
  columns_added: string[];
  columns_removed: string[];
  columns_retyped: PreviewCompareColumnRetype[];
  /** The live table was over `OXY_PREVIEW_COW_MAX_ROWS`: the copy started empty. */
  partial: boolean;
  /** The build dropped this table: `preview_rows` is `null`, `live_rows` is live's count. */
  dropped: boolean;
  /** The preview already held this table before the build started (an earlier run or build wrote it). */
  preexisting: boolean;
  /** Our own words, never an engine message — see the contract for the fixed set. */
  skipped_reason: string | null;
}

/**
 * A `transform_build`'s linked compare (once queued), or a `compare` run's
 * own detail. `tables` is `[]` until the compare succeeded.
 */
export interface PreviewCompare {
  run_id: string;
  state: PreviewRunState;
  outcome: PreviewRunOutcome | null;
  error: string | null;
  diff_max_rows: number | null;
  /** Fixed sentences, e.g. the freshness caveat; empty until the compare is done. */
  caveats: string[];
  tables: PreviewCompareTable[];
}

// ── Airway samples (S11) ─────────────────────────────────────────────────

/**
 * An `airway_sample` run's detail — `null` for every other kind.
 * `pipeline`/`dataset`/`window`/`resources`/`wall_clock_capped` reflect the
 * request from the moment the run is created; the rest fill in once the
 * sample itself has run (all `null`/`false`/`[]` until then).
 */
export interface PreviewRunSample {
  pipeline: string;
  dataset: string;
  window: PreviewRunWindow | null;
  resources: string[];
  wall_clock_capped: boolean;
  preview_pipeline: string | null;
  tables: string[] | null;
  compared_with_live: boolean | null;
  /** Uses the checks' `FindingView` shape and kinds — see `PreviewCheckFinding`. */
  verdict: PreviewCheckVerdict | null;
  findings: PreviewCheckFinding[];
  /** Cut off at the wall-clock cap or the run ceiling; `outcome` is still `succeeded`. */
  partial: boolean;
  partial_reason: string | null;
  /** Set only when recording the sample failed. */
  record_error: string | null;
}

/** `GET /api/{workspace_id}/previews/runs/{run_id}` */
export interface PreviewRunDetail extends PreviewRunSummary {
  agentic_run_id: string | null;
  error: string | null;
  steps: PreviewRunStep[];
  /** `null` unless this run is a `transform_build` (its compare) or a `compare` (itself). */
  compare: PreviewCompare | null;
  /** `null` unless this run is an `airway_sample`. */
  sample: PreviewRunSample | null;
}

// ── Sandbox sources (S11 Airway samples) ────────────────────────────────

/** The only environment `/previews/sources` accepts today. */
export type PreviewSourceEnvironment = "sandbox";

/**
 * Sandbox credentials for a rotate-on-use pipeline (QuickBooks: one sandbox
 * company per customer). Every secret is named, never carried by value — the
 * name must already exist as a workspace secret. `realm_id` and `client_id`
 * are not secrets and may be given directly; this UI only ever sends the
 * `_var` spellings so a raw secret value never passes through the form.
 */
export interface PreviewSourceOverrides {
  realm_id: string;
  refresh_token_var?: string;
  access_token_var?: string;
  client_secret_var?: string;
  client_id?: string;
  client_id_var?: string;
}

/** `GET`/`PUT /api/{workspace_id}/previews/sources` item, keyed by pipeline. */
export interface PreviewSourceItem {
  pipeline: string;
  environment: PreviewSourceEnvironment;
  overrides: PreviewSourceOverrides;
  updated_by: string | null;
  updated_at: string;
}

/** `PUT /api/{workspace_id}/previews/sources` request body (upsert by pipeline). */
export interface UpsertPreviewSourceRequest {
  pipeline: string;
  environment: PreviewSourceEnvironment;
  overrides: PreviewSourceOverrides;
}
