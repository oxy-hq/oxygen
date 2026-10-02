/**
 * Convert the new agentic-automation event stream into the same nested
 * `LogItem[]` tree the legacy automation UI expected. Each step becomes a
 * single root item with task-type-specific children built from the
 * `subrun_step_output` payload.
 *
 * The aggregation is deliberately defensive: backend payloads are typed as
 * `unknown` because they come over the wire, and the renderer must keep
 * working even if a future task variant ships a partial shape.
 */

import { useMemo } from "react";

import type { AutomationEvent, SubrunStepInfo } from "@/services/api/automations";
import type { LogItem } from "@/services/types/logs";

type StepRow = {
  name: string;
  taskType: string;
  status: "pending" | "running" | "cached" | "completed" | "failed" | "skipped";
  errorMessage?: string;
  body: LogItem[];
  /** Wall-clock time of the latest event for this step — used as the row timestamp. */
  timestamp: string;
  /**
   * Recursive child-task descriptors captured from `subrun_started`.
   * Container types (`loop_sequential`, `automation`) carry their nested
   * task DAG so iteration / sub-automation output can be decomposed by
   * task name and rendered with per-task-type formatters instead of
   * dumped as raw JSON.
   */
  innerTasks?: SubrunStepInfo[];
};

const STATUS_LABEL: Record<StepRow["status"], string> = {
  pending: "Pending",
  running: "Running",
  cached: "Cached",
  completed: "Done",
  failed: "Failed",
  skipped: "Skipped"
};

/**
 * Build the LogItem tree from the SSE event log. Pure: same input → same
 * output, so the memo only re-runs when `events` changes.
 */
export function useLogItems(events: AutomationEvent[]): LogItem[] {
  return useMemo(() => buildLogItems(events), [events]);
}

/**
 * Stateless variant of `useLogItems` for callers outside React (e.g. the
 * chat-thread automation runner, which keeps events in a closure and writes
 * the tree into a zustand store on each SSE message).
 */
export function buildLogItems(events: AutomationEvent[]): LogItem[] {
  const stepsByName = new Map<string, StepRow>();
  const order: string[] = [];

  const upsert = (name: string, taskType: string): StepRow => {
    let row = stepsByName.get(name);
    if (!row) {
      row = {
        name,
        taskType,
        status: "pending",
        body: [],
        timestamp: nowIso()
      };
      stepsByName.set(name, row);
      order.push(name);
    } else if (taskType && !row.taskType) {
      row.taskType = taskType;
    }
    return row;
  };

  for (const event of events) {
    switch (event.type) {
      case "subrun_started": {
        for (const step of event.payload.steps) {
          const row = upsert(step.name, step.task_type);
          if (step.inner_tasks && step.inner_tasks.length > 0) {
            row.innerTasks = step.inner_tasks;
          }
        }
        break;
      }
      case "subrun_step_started": {
        const row = upsert(event.payload.step, "");
        row.status = "running";
        row.timestamp = nowIso();
        break;
      }
      case "subrun_step_cache_hit": {
        const row = upsert(event.payload.step, "");
        row.status = "cached";
        row.timestamp = nowIso();
        // Two cache sources surface as the same event type. The file
        // path is the more useful pointer when the user is the one
        // who's editing it; the prior-run id is what an admin uses to
        // debug a step-hash cascade. Branch the message so neither
        // case shows `undefined`.
        const content =
          event.payload.source === "file" && event.payload.path
            ? `Reused from cached file \`${event.payload.path}\``
            : event.payload.prior_run_id
              ? `Reused from prior run \`${event.payload.prior_run_id}\``
              : "Reused from cache";
        row.body.push({
          content,
          log_type: "info",
          timestamp: row.timestamp,
          append: false
        });
        break;
      }
      case "subrun_step_output": {
        const row = upsert(event.payload.step, event.payload.task_type);
        row.body.push(
          ...renderStepOutput(event.payload.task_type, event.payload.output, row.innerTasks)
        );
        row.timestamp = nowIso();
        break;
      }
      case "subrun_step_completed": {
        const row = upsert(event.payload.step, "");
        if (event.payload.success) {
          if (row.status !== "cached") row.status = "completed";
        } else {
          row.status = "failed";
          if (event.payload.error) {
            row.errorMessage = event.payload.error;
            row.body.push({
              content: `**Error:** ${event.payload.error}`,
              log_type: "error",
              timestamp: nowIso(),
              append: false
            });
          }
        }
        row.timestamp = nowIso();
        break;
      }
      case "subrun_completed": {
        // On failure, sweep orphan rows so the log stops saying
        // "Running" forever when the automation halted mid-step. Distinguish
        // the failure point from downstream steps that never ran:
        //   running → `failed` (was actively in flight when the automation died)
        //   pending → `skipped` (never got its turn — downstream of failure)
        // The two states render differently in the UI: skipped rows are muted
        // and don't get an "**Error:** …" line, while failed rows do.
        if (!event.payload.success) {
          for (const row of stepsByName.values()) {
            if (row.status === "running") {
              row.status = "failed";
              row.timestamp = nowIso();
              if (!row.errorMessage) {
                row.errorMessage = "Automation halted while this step was running";
                row.body.push({
                  content: `**Error:** ${row.errorMessage}`,
                  log_type: "error",
                  timestamp: row.timestamp,
                  append: false
                });
              }
            } else if (row.status === "pending") {
              row.status = "skipped";
              row.timestamp = nowIso();
              if (!row.errorMessage) {
                row.errorMessage = "Skipped — an earlier step failed before reaching this one";
              }
            }
          }
        }
        break;
      }
      default:
        // Deliberately not in the Output log: the loop-iteration events
        // (`subrun_step_iteration_*`) only drive the diagram's progress bar —
        // each iteration's result arrives in the loop step's own
        // `subrun_step_output` — and `task_failed` / `worker_task_claimed` /
        // `waiting_on_children` / `decider_decided` are coordinator
        // diagnostics shown in the admin Trace view.
        break;
    }
  }

  return order.map((name): LogItem => {
    const row = stepsByName.get(name)!;
    return {
      content: stepHeading(row),
      log_type: row.status === "failed" ? "error" : "info",
      timestamp: row.timestamp,
      append: false,
      children: row.body,
      is_streaming: row.status === "running"
    };
  });
}

function stepHeading(row: StepRow): string {
  const label = STATUS_LABEL[row.status];
  // The expandable parent renders the heading as plain text — markdown chars
  // would leak into it, so we keep it terse.
  if (row.taskType) {
    return `${row.name} — ${row.taskType} · ${label}`;
  }
  return `${row.name} · ${label}`;
}

// ── Per-task content renderers ─────────────────────────────────────────────
//
// These produce the *children* of the step's parent log item. They mirror the
// legacy block→LogItem mapping in `useAutomationRun.ts`'s `logSelector`.

function renderStepOutput(
  taskType: string,
  output: unknown,
  innerTasks?: SubrunStepInfo[]
): LogItem[] {
  const ts = nowIso();
  switch (taskType) {
    case "execute_sql":
      return renderSqlOutput(output, ts);
    case "semantic_query":
      return renderSemanticQueryOutput(output, ts);
    case "looker_query":
      return renderLookerQueryOutput(output, ts);
    case "omni_query":
      return renderLookerQueryOutput(output, ts);
    case "agent":
      return renderAgentOutput(output, ts);
    case "formatter":
      return renderTextOutput(output, ts);
    case "conditional":
      return renderConditionalOutput(output, ts);
    case "loop_sequential":
      return renderLoopOutput(output, ts, innerTasks);
    case "workflow":
      return renderSubAutomationOutput(output, ts, innerTasks);
    default:
      return renderRawJson(output, ts);
  }
}

type QueryShape = {
  sql?: string;
  columns?: unknown;
  rows?: unknown;
  row_count?: number;
  truncated?: boolean;
};

function renderSqlOutput(output: unknown, ts: string): LogItem[] {
  const o = output as QueryShape;
  const items: LogItem[] = [];
  if (o.sql) {
    items.push({
      content: ["**SQL Query**", "", "```sql", o.sql, "```"].join("\n"),
      log_type: "info",
      timestamp: ts,
      append: false
    });
  }
  const table = renderResultsTable(o);
  if (table) {
    items.push({ content: table, log_type: "info", timestamp: ts, append: false });
  }
  return items;
}

function renderSemanticQueryOutput(output: unknown, ts: string): LogItem[] {
  const o = output as QueryShape & { semantic_query?: string };
  const items: LogItem[] = [];
  if (o.semantic_query) {
    items.push({
      content: ["**Semantic Query**", "", "```yaml", o.semantic_query, "```"].join("\n"),
      log_type: "info",
      timestamp: ts,
      append: false
    });
  }
  if (o.sql) {
    items.push({
      content: ["**Generated SQL**", "", "```sql", o.sql, "```"].join("\n"),
      log_type: "info",
      timestamp: ts,
      append: false
    });
  }
  const table = renderResultsTable(o);
  if (table) {
    items.push({ content: table, log_type: "info", timestamp: ts, append: false });
  }
  return items;
}

function renderLookerQueryOutput(output: unknown, ts: string): LogItem[] {
  const o = output as QueryShape & {
    model?: string;
    explore?: string;
    fields?: string[];
    integration?: string;
  };
  const items: LogItem[] = [];
  const summaryParts: string[] = [];
  if (o.integration) summaryParts.push(`integration: \`${o.integration}\``);
  if (o.model) summaryParts.push(`model: \`${o.model}\``);
  if (o.explore) summaryParts.push(`explore: \`${o.explore}\``);
  if (Array.isArray(o.fields) && o.fields.length > 0) {
    summaryParts.push(`fields: ${o.fields.map((f) => `\`${f}\``).join(", ")}`);
  }
  if (summaryParts.length > 0) {
    items.push({
      content: summaryParts.join(" · "),
      log_type: "info",
      timestamp: ts,
      append: false
    });
  }
  if (o.sql) {
    items.push({
      content: ["**Generated SQL**", "", "```sql", o.sql, "```"].join("\n"),
      log_type: "info",
      timestamp: ts,
      append: false
    });
  }
  const table = renderResultsTable(o);
  if (table) {
    items.push({ content: table, log_type: "info", timestamp: ts, append: false });
  }
  return items;
}

function renderAgentOutput(output: unknown, ts: string): LogItem[] {
  if (typeof output === "string") {
    return [{ content: output, log_type: "info", timestamp: ts, append: false }];
  }
  if (output && typeof output === "object") {
    const text = (output as { text?: unknown; answer?: unknown }).text;
    const answer = (output as { answer?: unknown }).answer;
    const body = typeof text === "string" ? text : typeof answer === "string" ? answer : null;
    if (body) {
      return [{ content: body, log_type: "info", timestamp: ts, append: false }];
    }
  }
  return renderRawJson(output, ts);
}

function renderTextOutput(output: unknown, ts: string): LogItem[] {
  if (typeof output === "string") {
    return [{ content: output, log_type: "info", timestamp: ts, append: false }];
  }
  if (output && typeof output === "object") {
    const text = (output as { text?: unknown }).text;
    if (typeof text === "string") {
      return [{ content: text, log_type: "info", timestamp: ts, append: false }];
    }
  }
  return renderRawJson(output, ts);
}

function renderConditionalOutput(output: unknown, ts: string): LogItem[] {
  const o = output as { branch?: string; condition?: string; tasks?: string[] };
  const parts: string[] = [];
  if (o.branch) parts.push(`branch: \`${o.branch}\``);
  if (o.condition) parts.push(`condition: \`${o.condition}\``);
  if (Array.isArray(o.tasks) && o.tasks.length > 0) {
    parts.push(`tasks: ${o.tasks.map((t) => `\`${t}\``).join(", ")}`);
  }
  if (parts.length === 0) return renderRawJson(output, ts);
  return [{ content: parts.join(" · "), log_type: "info", timestamp: ts, append: false }];
}

function renderLoopOutput(output: unknown, ts: string, innerTasks?: SubrunStepInfo[]): LogItem[] {
  if (Array.isArray(output)) {
    return [
      {
        content: `${output.length} iteration${output.length === 1 ? "" : "s"} completed`,
        log_type: "info",
        timestamp: ts,
        append: false
      }
    ];
  }
  // Loop-sequential output is an object: `{ inline-0: {…}, inline-1: {…},
  // …, iterations: { <hash>: {…} } }`. We render one collapsible child
  // per iteration so the user can drill in instead of staring at a wall
  // of JSON. Each iteration becomes a parent LogItem whose children are
  // its inner tasks (rendered with the same per-task-type formatters
  // top-level steps use).
  if (output && typeof output === "object" && !Array.isArray(output)) {
    const entries = extractIterationEntries(output);
    if (entries.length === 0) return renderRawJson(output, ts);
    return entries.map((iter) => {
      const label = iter.status === "failed" ? "Failed" : "Done";
      const valueLabel = formatIterationValue(iter.value);
      return {
        content: `Iteration ${iter.index}${valueLabel ? ` — ${valueLabel}` : ""} · ${label}`,
        log_type: iter.status === "failed" ? "error" : "info",
        timestamp: ts,
        append: false,
        children: renderIterationBody(iter, ts, innerTasks)
      };
    });
  }
  return renderRawJson(output, ts);
}

type LoopIterationEntry = {
  index: number;
  status: string;
  value: unknown;
  answer: unknown;
  error: unknown;
};

function extractIterationEntries(output: object): LoopIterationEntry[] {
  const obj = output as Record<string, unknown>;
  const iterationsMap = obj.iterations;

  const fromIterationsMap: LoopIterationEntry[] =
    iterationsMap && typeof iterationsMap === "object" && !Array.isArray(iterationsMap)
      ? Object.values(iterationsMap as Record<string, unknown>)
          .filter((e): e is Record<string, unknown> => !!e && typeof e === "object")
          .map((e) => ({
            index: typeof e.index === "number" ? e.index : 0,
            status: typeof e.status === "string" ? e.status : "done",
            value: e.value,
            answer: e.answer,
            error: e.error
          }))
      : [];

  if (fromIterationsMap.length > 0) {
    return fromIterationsMap.sort((a, b) => a.index - b.index);
  }

  // Fallback: walk `inline-N` keys (decider's pre-iterations-map shape
  // or non-loop child shape). Value isn't carried here so the child
  // header omits it gracefully.
  const inlineEntries = Object.entries(obj)
    .filter(([k]) => k.startsWith("inline-"))
    .map(([_, v]) => v)
    .filter((v): v is Record<string, unknown> => !!v && typeof v === "object")
    .map((e) => ({
      index: typeof e.index === "number" ? e.index : 0,
      status: typeof e.status === "string" ? e.status : "done",
      value: undefined,
      answer: e.answer,
      error: e.error
    }));
  return inlineEntries.sort((a, b) => a.index - b.index);
}

function renderIterationBody(
  iter: LoopIterationEntry,
  ts: string,
  innerTasks?: SubrunStepInfo[]
): LogItem[] {
  if (iter.status === "failed") {
    const msg = typeof iter.error === "string" ? iter.error : JSON.stringify(iter.error ?? null);
    return [{ content: `**Error:** ${msg}`, log_type: "error", timestamp: ts, append: false }];
  }
  if (iter.answer === null || iter.answer === undefined) return [];

  // The iteration's `answer` is `build_final_answer(child_state)` —
  // either a `{task_name: value}` object (current shape) or a JSON
  // array of per-task outputs in declaration order (older shape).
  // Decompose into one nested LogItem per inner task, dispatched
  // through the right per-task-type renderer so each iteration looks
  // like a mini-automation run instead of a wall of JSON.
  const decomposed = decomposeAnswer(iter.answer, innerTasks);
  if (decomposed) return decomposed;

  // Fallback for shapes that don't match either form: string → plain
  // text, otherwise dump as JSON.
  if (typeof iter.answer === "string") {
    return [{ content: iter.answer, log_type: "info", timestamp: ts, append: false }];
  }
  return renderRawJson(iter.answer, ts);
}

/**
 * Decompose `build_final_answer`-shaped output into per-task LogItems.
 *
 * Accepts both the object form (`{task_name: value, …}` — current
 * backend) and the legacy array form (positional by `innerTasks` order).
 * Strings that JSON-parse into either form are tolerated. Returns null
 * when the shape doesn't match and the caller should fall back.
 *
 * `innerTasks` is required to know each child task's type; without it
 * we can still render headings but lose per-type formatting (SQL
 * highlighting, etc.).
 */
function decomposeAnswer(answer: unknown, innerTasks?: SubrunStepInfo[]): LogItem[] | null {
  const ts = nowIso();
  const value = typeof answer === "string" ? tryParseJson(answer) : answer;

  // Object form: keys are task names, values are task outputs.
  if (value && typeof value === "object" && !Array.isArray(value)) {
    const tasksByName = new Map((innerTasks ?? []).map((t) => [t.name, t]));
    const entries = Object.entries(value as Record<string, unknown>);
    if (entries.length === 0) return null;
    return entries.map(([name, taskOutput]) => {
      const task = tasksByName.get(name);
      const taskType = task?.task_type ?? "";
      const heading = `${name}${taskType ? ` — ${taskType}` : ""}`;
      return {
        content: heading,
        log_type: "info",
        timestamp: ts,
        append: false,
        children: renderStepOutput(taskType, taskOutput, task?.inner_tasks)
      };
    });
  }

  // Legacy array form: positional by inner-task index.
  if (Array.isArray(value) && innerTasks && innerTasks.length > 0) {
    return value.map((taskOutput, i) => {
      const task = innerTasks[i];
      const taskType = task?.task_type ?? "";
      const taskName = task?.name ?? `task ${i}`;
      const heading = `${taskName} — ${taskType || "output"}`;
      return {
        content: heading,
        log_type: "info",
        timestamp: ts,
        append: false,
        children: renderStepOutput(taskType, taskOutput, task?.inner_tasks)
      };
    });
  }

  return null;
}

function tryParseJson(s: string): unknown {
  try {
    return JSON.parse(s);
  } catch {
    return s;
  }
}

function formatIterationValue(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") {
    const s = value.length > 40 ? `${value.slice(0, 37)}…` : value;
    return `\`${s}\``;
  }
  if (typeof value === "number" || typeof value === "boolean") return `\`${value}\``;
  try {
    const s = JSON.stringify(value);
    return s.length > 40 ? `\`${s.slice(0, 37)}…\`` : `\`${s}\``;
  } catch {
    return "";
  }
}

/**
 * Render a sub-automation step's output.
 *
 * The output is the child automation's `build_final_answer` —
 * `{child_task_name: value, …}`. With `innerTasks` populated by the
 * recursive resolver in `subrun_started`, each child task gets a
 * collapsible nested LogItem with the right per-type renderer (and
 * recurses for grandchildren). Without `innerTasks` (cycle / load
 * failure on the BE), we still decompose by key but lose type-aware
 * rendering.
 */
function renderSubAutomationOutput(
  output: unknown,
  ts: string,
  innerTasks?: SubrunStepInfo[]
): LogItem[] {
  const decomposed = decomposeAnswer(output, innerTasks);
  if (decomposed) return decomposed;

  // Older synthetic shapes carried just `{run_id, workflow_ref}` —
  // keep that compatibility path for backfill / replay.
  const o = output as { run_id?: string; workflow_ref?: string };
  const parts: string[] = [];
  if (o.workflow_ref) parts.push(`ref: \`${o.workflow_ref}\``);
  if (o.run_id) parts.push(`run: \`${o.run_id}\``);
  if (parts.length === 0) return renderRawJson(output, ts);
  return [{ content: parts.join(" · "), log_type: "info", timestamp: ts, append: false }];
}

/**
 * Render a `{ columns, rows }` payload as a markdown table. Returns null when
 * either side is empty so callers can suppress an empty results section.
 */
function renderResultsTable(o: QueryShape): string | null {
  const columns = Array.isArray(o.columns) ? (o.columns as unknown[]).map(toCellString) : null;
  const rows = Array.isArray(o.rows) ? (o.rows as unknown[]) : null;
  if (!columns || columns.length === 0 || !rows || rows.length === 0) return null;

  const headerRow = `|${columns.join("|")}|`;
  const separator = `|${columns.map(() => "---").join("|")}|`;
  const bodyRows = rows
    .map((row) => {
      const cells = Array.isArray(row) ? (row as unknown[]).map(toCellString) : [String(row)];
      return `|${cells.join("|")}|`;
    })
    .join("\n");

  const truncated = o.truncated
    ? `\n\n_Showing ${rows.length} of ${o.row_count ?? "?"} rows (truncated)_`
    : "";
  return `**Results**\n\n${headerRow}\n${separator}\n${bodyRows}${truncated}`;
}

function renderRawJson(output: unknown, ts: string): LogItem[] {
  if (output === null || output === undefined) return [];
  return [
    {
      content: ["```json", JSON.stringify(output, null, 2), "```"].join("\n"),
      log_type: "info",
      timestamp: ts,
      append: false
    }
  ];
}

function toCellString(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return escapePipes(value);
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return escapePipes(JSON.stringify(value));
}

function escapePipes(s: string): string {
  return s.replace(/\|/g, "\\|");
}

function nowIso(): string {
  return new Date().toISOString();
}
