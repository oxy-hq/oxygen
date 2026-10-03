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
    const response = await apiClient.post(`/${projectId}/analytics/runs`, body);
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

  static async submitAnswer(
    projectId: string,
    runId: string,
    answer: string
  ): Promise<{ ok: boolean; resumed?: boolean }> {
    const res = await apiClient.post(`/${projectId}/analytics/runs/${runId}/answer`, {
      answer
    });
    return res.data;
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
