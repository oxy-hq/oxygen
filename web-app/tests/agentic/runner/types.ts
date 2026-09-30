import type { Page } from "@playwright/test";

export type FlowTarget = "chat" | "ide" | "threads" | "onboarding" | "any";

/**
 * Which oxy backend mode the flow exercises.
 *
 * - `cloud` (the default): enterprise mode, the production path. Spawned via
 *   `oxy start --enterprise`, then `oxy seed --workspace-path demo_project`
 *   (runner/backend.ts). The runner drives the authenticated public port
 *   (3000) signed in as `flow@oxy.local` and prefixes workspace paths with
 *   `/local/workspaces/<demo id>` (runner/session.ts).
 * - `local`: the legacy `--local` single-workspace, no-auth mode
 *   (`oxy start --local --enterprise` from `demo_project/`). Unmaintained;
 *   opt in only for a flow that tests legacy local mode itself.
 *
 * A single `pnpm test:agentic` invocation must load flows that all agree
 * on backend mode — the runner refuses to mix in one boot.
 */
export type BackendMode = "local" | "cloud";

export interface FlowSettings {
  runs: number;
  model: string;
  judge_model: string;
  trace: "on-failure" | "always" | "never";
  cache_actions: boolean;
  max_steps: number;
  backend_mode: BackendMode;
}

export interface FlowStep {
  act?: string;
  wait_for?: string;
  /**
   * Cache scope for this step's recorded actions. `shared` lets two flows
   * that describe the exact same canonical prompt reuse a single recording
   * (intended for prelude steps copy-pasted from the canonical-prompts
   * library). Defaults to `flow` — recording is keyed on flow + case +
   * step index, so flows stay independent.
   */
  cache_scope?: "flow" | "shared";
}

export interface FlowExpect {
  assert?: string;
  judge?: string;
}

export interface FlowCase {
  name: string;
  tags: string[];
  steps: FlowStep[];
  expect: FlowExpect[];
}

export interface FlowTest {
  name: string;
  file: string;
  target: FlowTarget;
  settings: FlowSettings;
  setup: string[];
  cases: FlowCase[];
}

export interface ToolDefinition {
  name: string;
  description: string;
  // Pass-through JSON schema (forwarded to the Anthropic SDK's
  // input_schema). Property shapes use the full draft-07 vocabulary —
  // e.g. browser_file_upload uses an array property with `items` and
  // `minItems`. Keep the top-level `type` and `properties` typed since
  // every tool needs them; let each property be a generic JSON schema
  // fragment so we don't have to enumerate the schema vocab here.
  inputSchema: {
    type: "object";
    properties: Record<string, Record<string, unknown>>;
    required?: string[];
  };
  invoke: (args: Record<string, unknown>, page: Page) => Promise<unknown>;
}

export interface TokenUsage {
  /** Uncached input tokens (1× rate). */
  input: number;
  /** Cache-read input tokens (0.1× rate on Anthropic). */
  cached_input: number;
  /** Cache-creation input tokens (1.25× rate on Anthropic). */
  cache_creation: number;
  /** Output tokens. */
  output: number;
}

export function emptyTokens(): TokenUsage {
  return { input: 0, cached_input: 0, cache_creation: 0, output: 0 };
}

export function addTokens(into: TokenUsage, from: TokenUsage): void {
  into.input += from.input;
  into.cached_input += from.cached_input;
  into.cache_creation += from.cache_creation;
  into.output += from.output;
}

export interface ToolCallDebug {
  name: string;
  ms: number;
  /** Optional captured args for diagnosis. May be omitted on long calls to
   *  avoid bloating the JSON. */
  args?: Record<string, unknown>;
  error?: string;
}

/**
 * Per-step debug record. One entry per step in `testCase.steps`, in order.
 * Designed to be machine-readable enough that an agent (or human) can
 * diagnose a failed run from the JSON output alone.
 */
export interface SelectorDriftEvent {
  action_index: number;
  primary_selector: string;
  used_selector: string;
  used_kind: "testid" | "role_name" | "text" | "css";
}

export type RedriveReason =
  | "first_run"
  | "selector_mismatch"
  | "all_strategies_failed"
  | "cache_invalidated"
  | "step_text_changed";

export interface StepDebug {
  step_index: number;
  kind: "act" | "wait_for";
  /** The raw step text (act prompt or wait_for primitive). */
  text: string;
  duration_ms: number;
  /** Number of LLM iterations this step used. 0 for cache hit or wait_for. */
  iterations: number;
  /** Model that handled this step. Undefined for wait_for / cache hit. */
  model?: string;
  tokens: TokenUsage;
  cost_usd: number;
  /** True if the step replayed from the action cache (no LLM call). */
  from_cache: boolean;
  tool_calls: ToolCallDebug[];
  /** Total bytes returned across all browser_snapshot calls this step. */
  snapshot_bytes: number;
  /** Number of browser_snapshot calls (including any served from in-turn cache). */
  snapshot_calls: number;
  /** Calls where the snapshot was served from in-turn cache (no aria recompute). */
  snapshot_cache_hits: number;
  /** True if the step was retried on a stronger model after initial failure. */
  escalated?: boolean;
  /** First model the step ran on (if escalated, the one we started with). */
  initial_model?: string;
  /** Consecutive replays without LLM redrive (post-step). */
  cache_hit_streak?: number;
  /** Why the cache missed (only set when from_cache is false). */
  last_redrive_reason?: RedriveReason;
  /** Tier-1 silent fallback hits. Empty when the primary strategy resolved. */
  selector_drift_events?: SelectorDriftEvent[];
  /**
   * Tier 2 healing event — present when every strategy failed and the
   * runtime did an intent-aware redrive that succeeded. The new actions
   * land in `.cache/healing-staging.json`, not the main cache.
   */
  healed?: HealingEvent;
  error?: string;
}

export interface HealingEvent {
  action_index: number;
  old_primary?: string;
  new_primary?: string;
  old_kind?: SelectorDriftEvent["used_kind"];
  new_kind?: SelectorDriftEvent["used_kind"];
  intent?: string;
}

export interface JudgeUsage {
  model?: string;
  calls: number;
  tokens: TokenUsage;
  cost_usd: number;
}

export function emptyJudgeUsage(): JudgeUsage {
  return { calls: 0, tokens: emptyTokens(), cost_usd: 0 };
}

export interface ExpectResult {
  kind: "assert" | "judge";
  passed: boolean;
  claim: string;
  evidence?: string;
  rationale?: string;
}

export interface CaseRunResult {
  passed: boolean;
  duration_ms: number;
  step_count: number;
  /** Per-step tokens summed (acts only — wait_for has zero). Excludes judge. */
  tokens: TokenUsage;
  cache_hits: boolean[];
  expect_results: ExpectResult[];
  /** One entry per step in case.steps (in order). */
  step_debug: StepDebug[];
  /** Tokens + cost spent on judge: claims for this run. */
  judge_usage: JudgeUsage;
  /** USD cost for this run, including judge. */
  cost_usd: number;
  trace_path?: string;
  error?: string;
  /** Media written under `RuntimeContext.capture`; absent otherwise. */
  capture?: {
    screenshot: string;
    video?: string;
    videoStartMs: number;
  };
}

export interface CaseResult {
  name: string;
  runs: CaseRunResult[];
}

export interface FlowResult {
  name: string;
  file: string;
  cases: CaseResult[];
}

export interface RunResults {
  runtime: "bespoke";
  started_at: string;
  duration_ms: number;
  /** USD cost summed across all runs in this invocation, including judge. */
  cost_usd: number;
  /** Anthropic pricing snapshot used to compute costs (YYYY-MM-DD). */
  pricing_as_of: string;
  flows: FlowResult[];
}
