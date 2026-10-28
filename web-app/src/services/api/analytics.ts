import { isAxiosError } from "axios";
import { apiBaseURL } from "../env";
import { apiClient } from "./axios";

export type ThinkingMode = "auto" | "extended_thinking";

export interface CreateAnalyticsRunRequest {
  agent_id: string;
  question: string;
  thread_id?: string;
  thinking_mode?: ThinkingMode;
  /** When true, the backend auto-accepts all file_change tool calls. */
  auto_accept?: boolean;
  /** `"builder"` starts the Builder copilot instead of an analytics agent. */
  domain?: string;
  model?: string;
  /** Onboarding's structured context; the backend builds the prompt from it. */
  onboarding_context?: unknown;
}

/**
 * A Builder or onboarding run edits workspace files, so it starts and is
 * answered on routes only the Factory serves. A chat starts and is answered on
 * routes any server answers.
 */
const startsABuilderRun = (body: CreateAnalyticsRunRequest) =>
  body.domain === "builder" || body.onboarding_context != null;

/**
 * A chat's answer can land on a server that is not the one driving the run (with
 * queued starts off, a chat is driven in the memory of the server that took its
 * start). That server answers `503` with `retryable` and `Retry-After`, and a
 * retry that reaches the driving server, or arrives once its lease has lapsed,
 * goes through. Bounded so a run nobody can resume still fails.
 */
const ANSWER_RETRY_ATTEMPTS = 15;
const ANSWER_RETRY_MAX_DELAY_MS = 5_000;

/** How long to wait before retrying `err`, or `null` when it is not retryable. */
const answerRetryDelayMs = (err: unknown): number | null => {
  if (!isAxiosError(err) || err.response?.status !== 503) return null;
  // Only the server's own "another server drives this run" answer: that one
  // took nothing, so posting again cannot deliver the answer twice. A 503
  // from anything else (a proxy, a gateway) may have reached the run.
  const retryable = (err.response.data as { retryable?: unknown } | undefined)?.retryable;
  if (retryable !== true) return null;
  const header: unknown = err.response.headers?.["retry-after"];
  const seconds = Number(header);
  const ms = Number.isFinite(seconds) && seconds >= 0 ? seconds * 1_000 : 1_000;
  return Math.min(ms, ANSWER_RETRY_MAX_DELAY_MS);
};

const waitFor = (ms: number, signal?: AbortSignal) =>
  new Promise<void>((resolve) => {
    if (signal?.aborted) return resolve();
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener(
      "abort",
      () => {
        clearTimeout(timer);
        resolve();
      },
      { once: true }
    );
  });

/** Post a chat answer, retrying while the server says another server drives it. */
async function postChatAnswer(
  url: string,
  answer: string,
  signal?: AbortSignal
): Promise<{ ok: boolean; resumed?: boolean }> {
  for (let attempt = 1; ; attempt++) {
    try {
      const res = await apiClient.post(url, { answer });
      return res.data;
    } catch (err: unknown) {
      const delay = answerRetryDelayMs(err);
      if (delay === null || attempt >= ANSWER_RETRY_ATTEMPTS || signal?.aborted) throw err;
      await waitFor(delay, signal);
      if (signal?.aborted) throw err;
    }
  }
}

export interface CreateAnalyticsRunResponse {
  run_id: string;
  thread_id?: string;
}

// ── UiBlock discriminated union ───────────────────────────────────────────────
// Mirrors the Rust UiBlock<AnalyticsEvent> enum from agentic-core/src/ui_stream.rs

type StepStartBlock = {
  seq: number;
  event_type: "step_start";
  payload: { label: string; summary?: string | null; sub_spec_index?: number | null };
};

type StepEndBlock = {
  seq: number;
  event_type: "step_end";
  payload: {
    label: string;
    /** Mirrors the Rust `Outcome` enum: "advanced" | "retry" | "backtracked" | "failed" | "suspended" */
    outcome: "advanced" | "retry" | "backtracked" | "failed" | "suspended";
    metadata?: Record<string, unknown> | null;
    sub_spec_index?: number | null;
  };
};

type StepSummaryUpdateBlock = {
  seq: number;
  event_type: "step_summary_update";
  payload: { summary: string };
};

type TextDeltaBlock = {
  seq: number;
  event_type: "text_delta";
  payload: { token: string; sub_spec_index?: number | null };
};

type ThinkingStartBlock = {
  seq: number;
  event_type: "thinking_start";
  payload: { sub_spec_index?: number | null };
};

type ThinkingTokenBlock = {
  seq: number;
  event_type: "thinking_token";
  payload: { token: string; sub_spec_index?: number | null };
};

type ThinkingEndBlock = {
  seq: number;
  event_type: "thinking_end";
  payload: { sub_spec_index?: number | null };
};

type ToolCallBlock = {
  seq: number;
  event_type: "tool_call";
  payload: {
    name: string;
    input: unknown;
    llm_duration_ms?: number;
    sub_spec_index?: number | null;
  };
};

type ToolResultBlock = {
  seq: number;
  event_type: "tool_result";
  payload: {
    name: string;
    output: unknown;
    duration_ms: number;
    /** The tool failed. Always sent today; absent on runs persisted before the field existed. */
    is_error?: boolean;
    sub_spec_index?: number | null;
  };
};

export type HumanInputQuestion = {
  prompt: string;
  suggestions: string[];
};

type AwaitingInputBlock = {
  seq: number;
  event_type: "awaiting_input";
  payload: { questions: HumanInputQuestion[] };
};

type InputResolvedBlock = {
  seq: number;
  event_type: "input_resolved";
  payload: { answer?: string; trace_id?: string };
};

type DoneBlock = {
  seq: number;
  event_type: "done";
  payload: { duration_ms?: number };
};

type ErrorBlock = {
  seq: number;
  event_type: "error";
  payload: { message: string };
};

type FanOutStartBlock = {
  seq: number;
  event_type: "fan_out_start";
  payload: { total: number };
};

type SubSpecStartBlock = {
  seq: number;
  event_type: "sub_spec_start";
  payload: { index: number; total: number; label: string };
};

type SubSpecEndBlock = {
  seq: number;
  event_type: "sub_spec_end";
  payload: { index: number; success: boolean };
};

type FanOutEndBlock = {
  seq: number;
  event_type: "fan_out_end";
  payload: { success: boolean };
};

// ── Domain events ─────────────────────────────────────────────────────────────

type SchemaResolvedBlock = {
  seq: number;
  event_type: "schema_resolved";
  payload: { tables: string[]; duration_ms: number };
};

type TriageCompletedBlock = {
  seq: number;
  event_type: "triage_completed";
  payload: {
    summary: string;
    relevant_tables: string[];
    question_type: string;
    confidence: number;
    ambiguities: string[];
  };
};

type IntentClarifiedBlock = {
  seq: number;
  event_type: "intent_clarified";
  payload: {
    question_type: string;
    metrics: string[];
    dimensions: string[];
    filters: string[];
    selected_automation?: string;
    /** Legacy field name, kept so runs persisted before the rename still type-check. */
    selected_procedure?: string;
  };
};

type SpecResolvedBlock = {
  seq: number;
  event_type: "spec_resolved";
  payload: {
    resolved_metrics: string[];
    resolved_tables: string[];
    join_path: [string, string, string][];
    result_shape: string;
    assumptions: string[];
    solution_source: string;
  };
};

type QueryGeneratedBlock = {
  seq: number;
  event_type: "query_generated";
  payload: { sql: string; sub_spec_index?: number | null };
};

export type SemanticQueryPayload = {
  measures?: string[];
  dimensions?: string[];
  filters?: Array<{ member: string; operator: string; values?: string[] }>;
  time_dimensions?: Array<{
    dimension: string;
    granularity?: string | null;
    date_range?: string[] | null;
  }>;
  order?: Array<{ id: string; desc?: boolean }>;
  limit?: number | null;
  assumptions?: string[];
};

type QueryExecutedBlock = {
  seq: number;
  event_type: "query_executed";
  payload: {
    query: string;
    row_count: number;
    duration_ms: number;
    success: boolean;
    error?: string;
    columns: string[];
    rows: string[][];
    source?: "semantic" | "llm" | "vendor";
    is_preagg?: boolean;
    sub_spec_index?: number | null;
    semantic_query?: SemanticQueryPayload;
  };
};

type AnalyticsValidationFailedBlock = {
  seq: number;
  event_type: "analytics_validation_failed";
  payload: { state: string; reason: string; model_response: string };
};

export type ChartConfig = {
  chart_type: "line_chart" | "bar_chart" | "pie_chart" | "table";
  x?: string;
  y?: string;
  series?: string;
  name?: string;
  value?: string;
  title?: string;
  x_axis_label?: string;
  y_axis_label?: string;
};

type SemanticShortcutAttemptedBlock = {
  seq: number;
  event_type: "semantic_shortcut_attempted";
  payload: {
    measures: string[];
    dimensions: string[];
    filters: string[];
    time_dimensions: string[];
  };
};

type SemanticShortcutResolvedBlock = {
  seq: number;
  event_type: "semantic_shortcut_resolved";
  payload: { sql: string };
};

type ChartRenderedBlock = {
  seq: number;
  event_type: "chart_rendered";
  payload: {
    config: ChartConfig;
    columns: string[];
    rows: unknown[][];
  };
};

type SubrunStartedBlock = {
  seq: number;
  event_type: "subrun_started";
  payload: {
    /** Human-readable automation name (file stem without `.procedure` suffix). */
    subrun_name: string;
    /** Ordered list of top-level task descriptors from the automation definition. */
    steps: Array<{ name: string; task_type: string }>;
  };
};

type SubrunCompletedBlock = {
  seq: number;
  event_type: "subrun_completed";
  payload: {
    /** Human-readable automation name, matching the paired `subrun_started`. */
    subrun_name: string;
    /** `true` when the automation completed without error. */
    success: boolean;
    /** Error message when `success` is false. */
    error?: string;
  };
};

type SubrunStepStartedBlock = {
  seq: number;
  event_type: "subrun_step_started";
  payload: {
    /** Human-readable task name from the automation YAML. */
    step: string;
  };
};

type SubrunStepCompletedBlock = {
  seq: number;
  event_type: "subrun_step_completed";
  payload: {
    /** Matches the `step` field of the paired `subrun_step_started` event. */
    step: string;
    success: boolean;
    error?: string;
    /** True when the step result was served from local pre-aggregation Parquet. */
    is_preagg?: boolean;
  };
};

// ── Builder copilot events ────────────────────────────────────────────────────

type ToolUsedBlock = {
  seq: number;
  event_type: "tool_used";
  payload: { tool_name: string; summary: string };
};

export type FileChangePendingBlock = {
  seq: number;
  event_type: "file_change_pending";
  payload: {
    file_path: string;
    description: string;
    new_content: string;
    old_content: string;
    delete?: boolean;
  };
};

export type FileChangedBlock = {
  seq: number;
  event_type: "file_changed";
  payload: {
    file_path: string;
    description: string;
    new_content: string;
    old_content: string;
    is_deletion: boolean;
  };
};

// ── App-builder domain events ────────────────────────────────────────────────

type TaskPlanReadyBlock = {
  seq: number;
  event_type: "task_plan_ready";
  payload: { task_count: number; control_count: number; spec?: unknown };
};

type TaskSqlResolvedBlock = {
  seq: number;
  event_type: "task_sql_resolved";
  payload: { task_name: string; sql: string };
};

type TaskExecutedBlock = {
  seq: number;
  event_type: "task_executed";
  payload: {
    task_name: string;
    sql: string;
    row_count: number;
    columns: string[];
    sample_rows: string[][];
  };
};

type AppYamlReadyBlock = {
  seq: number;
  event_type: "app_yaml_ready";
  payload: { char_count: number };
};

type LlmUsageBlock = {
  seq: number;
  event_type: "llm_usage";
  payload: {
    prompt_tokens: number;
    output_tokens: number;
    duration_ms: number;
    model?: string;
    sub_spec_index?: number | null;
  };
};

type RecoveryResumedBlock = {
  seq: number;
  event_type: "recovery_resumed";
  payload: Record<string, unknown>;
};

type DelegationStartedBlock = {
  seq: number;
  event_type: "delegation_started";
  payload: {
    child_task_id: string;
    target: string;
    request: string;
  };
};

type DelegationCompletedBlock = {
  seq: number;
  event_type: "delegation_completed";
  payload: {
    child_task_id: string;
    success: boolean;
    answer?: string;
    error?: string;
  };
};

/** Strict discriminated union of all UI blocks emitted by the agentic pipeline.
 *  Mirrors the Rust `UiBlock<AnalyticsEvent>` enum. */
export type UiBlock =
  | StepStartBlock
  | StepEndBlock
  | StepSummaryUpdateBlock
  | TextDeltaBlock
  | ThinkingStartBlock
  | ThinkingTokenBlock
  | ThinkingEndBlock
  | ToolCallBlock
  | ToolResultBlock
  | AwaitingInputBlock
  | InputResolvedBlock
  | DoneBlock
  | ErrorBlock
  | FanOutStartBlock
  | SubSpecStartBlock
  | SubSpecEndBlock
  | FanOutEndBlock
  | SchemaResolvedBlock
  | TriageCompletedBlock
  | IntentClarifiedBlock
  | SpecResolvedBlock
  | QueryGeneratedBlock
  | QueryExecutedBlock
  | AnalyticsValidationFailedBlock
  | ChartRenderedBlock
  | SubrunStartedBlock
  | SubrunCompletedBlock
  | SubrunStepStartedBlock
  | SubrunStepCompletedBlock
  | TaskPlanReadyBlock
  | TaskSqlResolvedBlock
  | TaskExecutedBlock
  | AppYamlReadyBlock
  | SemanticShortcutAttemptedBlock
  | SemanticShortcutResolvedBlock
  | LlmUsageBlock
  | ToolUsedBlock
  | FileChangePendingBlock
  | FileChangedBlock
  | RecoveryResumedBlock
  | DelegationStartedBlock
  | DelegationCompletedBlock;

export interface AnalyticsRunSummary {
  run_id: string;
  status: "running" | "suspended" | "done" | "failed" | "cancelled";
  agent_id: string;
  question: string;
  answer?: string;
  error_message?: string;
  thinking_mode?: ThinkingMode;
  ui_events?: UiBlock[];
}

export class AnalyticsService {
  static async createRun(
    projectId: string,
    body: CreateAnalyticsRunRequest
  ): Promise<CreateAnalyticsRunResponse> {
    const path = startsABuilderRun(body) ? "analytics/builder/runs" : "analytics/runs";
    const response = await apiClient.post(`/${projectId}/${path}`, body);
    return response.data;
  }

  static async getRunsByThread(
    projectId: string,
    threadId: string
  ): Promise<AnalyticsRunSummary[]> {
    const response = await apiClient.get(`/${projectId}/analytics/threads/${threadId}/runs`);
    return response.data;
  }

  static async getRunByThread(
    projectId: string,
    threadId: string
  ): Promise<AnalyticsRunSummary | null> {
    try {
      const response = await apiClient.get(`/${projectId}/analytics/threads/${threadId}/run`);
      return response.data;
    } catch (err: unknown) {
      if (
        typeof err === "object" &&
        err !== null &&
        "response" in err &&
        (err as { response?: { status?: number } }).response?.status === 404
      ) {
        return null;
      }
      throw err;
    }
  }

  /**
   * `builder`: the run is a Builder or onboarding run, so the answer goes straight to
   * the Builder answer route (the chat route refuses one on every server). Without
   * it (a run whose kind the caller does not know) the chat route is tried first and
   * its `421` names the Builder route — a second round trip that a page navigating
   * away in between would lose.
   *
   * A chat answer is retried while its server says another server drives the run
   * (see `postChatAnswer`); `signal` stops those retries.
   */
  static async submitAnswer(
    projectId: string,
    runId: string,
    answer: string,
    { builder = false, signal }: { builder?: boolean; signal?: AbortSignal } = {}
  ): Promise<{ ok: boolean; resumed?: boolean }> {
    if (builder) {
      const res = await apiClient.post(`/${projectId}/analytics/builder/runs/${runId}/answer`, {
        answer
      });
      return res.data;
    }
    try {
      return await postChatAnswer(`/${projectId}/analytics/runs/${runId}/answer`, answer, signal);
    } catch (err: unknown) {
      // A Builder run is answered on its own route, which the Factory serves. The
      // chat route says so with 421 and the Builder answer route.
      if (isAxiosError(err) && err.response?.status === 421) {
        const res = await apiClient.post(`/${projectId}/analytics/builder/runs/${runId}/answer`, {
          answer
        });
        return res.data;
      }
      throw err;
    }
  }

  static async cancelRun(projectId: string, runId: string): Promise<void> {
    await apiClient.post(`/${projectId}/analytics/runs/${runId}/cancel`);
  }

  /**
   * Revert builder-applied file change(s) for a run. Pass `filePaths` to
   * revert specific files; omit (or pass an empty array) to revert every
   * file the builder changed in this run.
   */
  static async revertBuilderFileChanges(
    projectId: string,
    runId: string,
    filePaths?: string[]
  ): Promise<{ ok: boolean; reverted: { file_path: string; action: string }[] }> {
    const res = await apiClient.post(`/${projectId}/analytics/runs/${runId}/revert-file-changes`, {
      file_paths: filePaths ?? []
    });
    return res.data;
  }

  static async updateThinkingMode(
    projectId: string,
    runId: string,
    thinkingMode: ThinkingMode
  ): Promise<void> {
    await apiClient.patch(`/${projectId}/analytics/runs/${runId}/thinking_mode`, {
      thinking_mode: thinkingMode === "auto" ? null : thinkingMode
    });
  }

  /** Returns the URL for the SSE event stream (callers open it with fetchEventSource). */
  static eventsUrl(projectId: string, runId: string): string {
    return `${apiBaseURL}/${projectId}/analytics/runs/${runId}/events`;
  }
}
