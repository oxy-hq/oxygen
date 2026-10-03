import type { UiBlock } from "@/services/api/analytics";

// ── Trace item types ──────────────────────────────────────────────────────────

export type ThinkingItem = {
  kind: "thinking";
  id: string;
  text: string;
  isStreaming: boolean;
};

export type ArtifactItem = {
  kind: "artifact";
  id: string;
  /** SSE sequence number of the tool_call event that produced this artifact. */
  seq?: number;
  toolName: string;
  toolInput: string;
  toolOutput?: string;
  /** True when the tool returned an error (is_error=true in the tool_result event). */
  isError?: boolean;
  /** Tool execution time in ms (time spent inside the tool, excluding LLM). */
  durationMs?: number;
  /** LLM inference time for the round that produced this tool call (ms). */
  llmDurationMs?: number;
  isStreaming: boolean;
};

export type SqlItem = {
  kind: "sql";
  id: string;
  sql: string;
  database?: string;
  result?: string[][];
  rowCount?: number;
  durationMs?: number;
  error?: string;
  /** Which code path produced the SQL. "semantic" = compiled by airlayer. "verified_sql" = pre-written SQL file. */
  source?: "semantic" | "llm" | "vendor" | "verified_sql";
  /** True when the query was served from local pre-aggregation Parquet. */
  is_preagg?: boolean;
  /** Structured semantic query attached when source === "semantic". */
  semanticQuery?: import("@/services/api/analytics").SemanticQueryPayload;
  isStreaming: boolean;
};

export type TextItem = {
  kind: "text";
  id: string;
  text: string;
  isStreaming: boolean;
};

export type AutomationItem = {
  kind: "automation";
  id: string;
  automationName: string;
  steps: Array<{ name: string; task_type: string }>;
  stepsDone: number;
  isStreaming: boolean;
};

export type BuilderDelegationItem = {
  kind: "builder_delegation";
  id: string;
  childRunId: string;
  request: string;
  status: "running" | "done" | "failed";
  answer?: string;
  error?: string;
  isStreaming: boolean;
};

export type TraceItem =
  | ThinkingItem
  | ArtifactItem
  | SqlItem
  | TextItem
  | AutomationItem
  | BuilderDelegationItem;

export type SelectableItem = ArtifactItem | SqlItem | AutomationItem | BuilderDelegationItem;

// ── Step / fan-out types ──────────────────────────────────────────────────────

/** Accumulated LLM usage for a step (may span multiple LLM calls). */
export type StepLlmUsage = {
  inputTokens: number;
  outputTokens: number;
  /** Total wall-clock time for the LLM invocation (includes tool execution). */
  durationMs: number;
  /** Sum of all tool execution times within this step. */
  toolDurationMs: number;
  /** Model identifier from the last LLM call in this step. */
  model?: string;
};

export type AnalyticsStep = {
  kind: "step";
  id: string;
  label: string;
  /** One-line description of what this step does; updated dynamically via step_summary_update. */
  summary?: string;
  isStreaming: boolean;
  /** Set when the step ended with a non-successful outcome. */
  error?: string;
  /** Set when the step suspended to await human input. */
  suspended?: boolean;
  items: TraceItem[];
  /** Domain state output attached to step_end. */
  metadata?: Record<string, unknown>;
  /** Accumulated LLM usage across all LLM calls in this step. */
  llmUsage?: StepLlmUsage;
};

/** One card within a fan-out group, containing its own pipeline steps. */
type FanOutCard = {
  id: string;
  index: number;
  label: string;
  steps: AnalyticsStep[];
  isStreaming: boolean;
};

/** A group of parallel sub-spec executions rendered as navigable cards. */
export type FanOutGroup = {
  kind: "fan_out";
  id: string;
  total: number;
  cards: FanOutCard[];
  isStreaming: boolean;
};

export type StepOrGroup = AnalyticsStep | FanOutGroup;

// ── Scope ─────────────────────────────────────────────────────────────────────
//
// Encapsulates all mutable builder state and exposes explicit lifecycle methods.
// stepStack holds only the steps for the *current* scope. When a fan-out card
// begins, outer open steps are saved aside and the stack is cleared; on card end
// they are restored. Completed steps are routed to currentCard or result based
// solely on whether a card is active — no depth arithmetic needed.

function createScope() {
  const result: StepOrGroup[] = [];
  let counter = 0;
  const nextId = (prefix: string) => `${prefix}-${counter++}`;

  // Outer (non-fan-out) step stack.
  const stepStack: AnalyticsStep[] = [];

  // ── Concurrent fan-out state ────────────────────────────────────────────
  // Multiple cards can be active simultaneously during concurrent fan-out.
  // Each card has its own step stack, keyed by sub-spec index.
  let savedOuterSteps: AnalyticsStep[] | null = null;
  const activeCards = new Map<number, { card: FanOutCard; stepStack: AnalyticsStep[] }>();
  let currentFanOut: FanOutGroup | null = null;

  // ── Routing helpers ─────────────────────────────────────────────────────
  // When sub_spec_index is set, route to the card's step stack; otherwise outer.
  const getStepStack = (subSpecIndex?: number | null): AnalyticsStep[] => {
    if (subSpecIndex != null) {
      const entry = activeCards.get(subSpecIndex);
      if (entry) return entry.stepStack;
    }
    return stepStack;
  };

  const currentStep = (subSpecIndex?: number | null) => getStepStack(subSpecIndex).at(-1);

  const pushItem = (item: TraceItem, subSpecIndex?: number | null) => {
    const step = currentStep(subSpecIndex);
    if (step) {
      step.items.push(item);
      return;
    }
    // Fallback: if no current step (e.g. after recovery flush), find the
    // last flushed step that contains an automation item and push there.
    for (let i = result.length - 1; i >= 0; i--) {
      const r = result[i];
      if (r.kind === "step" && r.items.some((it) => it.kind === "automation")) {
        r.items.push(item);
        return;
      }
    }
  };

  const findLastStreaming = <T extends TraceItem>(
    kind: T["kind"],
    subSpecIndex?: number | null
  ): T | undefined => {
    const items = currentStep(subSpecIndex)?.items;
    if (!items) return undefined;
    for (let i = items.length - 1; i >= 0; i--) {
      const item = items[i];
      if (item.kind === kind && item.isStreaming) return item as T;
    }
    return undefined;
  };

  /** Find the last streaming artifact item whose toolName matches `name`. */
  const findLastStreamingArtifactByName = (
    name: string,
    subSpecIndex?: number | null
  ): ArtifactItem | undefined => {
    const items = currentStep(subSpecIndex)?.items;
    if (items) {
      for (let i = items.length - 1; i >= 0; i--) {
        const item = items[i];
        if (item.kind === "artifact" && item.isStreaming && item.toolName === name) {
          return item;
        }
      }
    }
    // Fallback: search flushed results (recovery case — step was closed
    // but automation step artifacts were pushed into it afterwards).
    for (let i = result.length - 1; i >= 0; i--) {
      const r = result[i];
      if (r.kind !== "step") continue;
      for (let j = r.items.length - 1; j >= 0; j--) {
        const item = r.items[j];
        if (item.kind === "artifact" && item.isStreaming && item.toolName === name) {
          return item;
        }
      }
    }
    return undefined;
  };

  /** Find ALL streaming artifact items whose toolName is in `names`. */
  const findAllStreamingArtifactsByName = (
    names: string[],
    subSpecIndex?: number | null
  ): ArtifactItem[] => {
    const items = currentStep(subSpecIndex)?.items;
    if (!items) return [];
    return items.filter(
      (item): item is ArtifactItem =>
        item.kind === "artifact" && item.isStreaming && names.includes(item.toolName)
    );
  };

  /** Search completed steps in `result` for the last ask_user artifact and set its answer. */
  const updateAskUserAnswer = (answer: string) => {
    for (let i = result.length - 1; i >= 0; i--) {
      const step = result[i];
      if (step.kind !== "step") continue;
      for (let j = step.items.length - 1; j >= 0; j--) {
        const item = step.items[j];
        if (item.kind === "artifact" && item.toolName === "ask_user") {
          item.toolOutput = JSON.stringify({ answer });
          return;
        }
      }
    }
  };

  /** Search completed steps in `result` for the last artifact with `toolName` and update its output. */
  const updateCompletedArtifactOutput = (toolName: string, newOutput: string) => {
    for (let i = result.length - 1; i >= 0; i--) {
      const step = result[i];
      if (step.kind !== "step") continue;
      for (let j = step.items.length - 1; j >= 0; j--) {
        const item = step.items[j];
        if (item.kind === "artifact" && item.toolName === toolName) {
          item.toolOutput = newOutput;
          return true;
        }
      }
    }
    return false;
  };

  const completeStep = (
    outcome: string,
    metadata?: Record<string, unknown> | null,
    subSpecIndex?: number | null
  ) => {
    const stack = getStepStack(subSpecIndex);
    const step = stack.pop();
    if (!step) return;
    step.isStreaming = false;
    // `retry` and `backtracked` are a stage that errored and was recovered by diagnose:
    // the run goes on, to the same stage or an earlier one (agentic-core run_loop.rs).
    // Only `failed` ends it.
    if (outcome === "suspended") {
      step.suspended = true;
    } else if (outcome !== "advanced" && outcome !== "retry" && outcome !== "backtracked") {
      step.error = "Step failed";
    }
    if (metadata && !Array.isArray(metadata)) step.metadata = metadata;
    // Route completed step: if inside a card, push to card.steps; else to result.
    // Routing is by sub_spec_index alone — the backend tags every step a fan-out
    // sub-spec runs, so an untagged step is an outer one even while a card is open.
    if (subSpecIndex != null) {
      const entry = activeCards.get(subSpecIndex);
      if (entry) {
        entry.card.steps.push(step);
        return;
      }
    }
    result.push(step);
  };

  return {
    nextId,
    pushItem,
    findLastStreaming,
    findLastStreamingArtifactByName,
    findAllStreamingArtifactsByName,
    updateAskUserAnswer,
    updateCompletedArtifactOutput,

    // ── Step lifecycle ───────────────────────────────────────────────────────
    openStep(label: string, summary?: string, subSpecIndex?: number | null) {
      getStepStack(subSpecIndex).push({
        kind: "step",
        id: nextId("step"),
        label,
        summary,
        isStreaming: true,
        items: []
      });
    },
    closeStep: completeStep,
    updateSummary(summary: string, subSpecIndex?: number | null) {
      const top = currentStep(subSpecIndex);
      if (top) top.summary = summary;
    },

    // ── LLM usage accumulation ──────────────────────────────────────────────
    accumulateLlmUsage(
      promptTokens: number,
      outputTokens: number,
      durationMs: number,
      subSpecIndex?: number | null,
      model?: string
    ) {
      const step = currentStep(subSpecIndex);
      if (!step) return;
      if (!step.llmUsage) {
        step.llmUsage = { inputTokens: 0, outputTokens: 0, durationMs: 0, toolDurationMs: 0 };
      }
      step.llmUsage.inputTokens += promptTokens || 0;
      step.llmUsage.outputTokens += outputTokens || 0;
      step.llmUsage.durationMs += durationMs || 0;
      if (model) step.llmUsage.model = model;
    },

    accumulateToolDuration(durationMs: number, subSpecIndex?: number | null) {
      const step = currentStep(subSpecIndex);
      if (!step) return;
      if (!step.llmUsage) {
        step.llmUsage = { inputTokens: 0, outputTokens: 0, durationMs: 0, toolDurationMs: 0 };
      }
      step.llmUsage.toolDurationMs += durationMs || 0;
    },

    // ── Text streaming ───────────────────────────────────────────────────────
    appendTextDelta(token: string, subSpecIndex?: number | null) {
      const last = currentStep(subSpecIndex)?.items.at(-1);
      if (last?.kind === "text" && last.isStreaming) {
        last.text += token;
      } else {
        pushItem({ kind: "text", id: nextId("txt"), text: token, isStreaming: true }, subSpecIndex);
      }
    },

    // ── Fan-out lifecycle ────────────────────────────────────────────────────
    openFanOut(total: number) {
      currentFanOut = {
        kind: "fan_out",
        id: nextId("fan-out"),
        total,
        cards: [],
        isStreaming: true
      };
    },
    openCard(index: number, label: string) {
      // On first card, save the outer step stack.
      if (activeCards.size === 0) {
        savedOuterSteps = [...stepStack];
        stepStack.length = 0;
      }
      activeCards.set(index, {
        card: { id: nextId("card"), index, label, steps: [], isStreaming: true },
        stepStack: []
      });
    },
    closeCard(index: number, success: boolean) {
      const entry = activeCards.get(index);
      if (!entry) return;
      // Drain any open steps on this card's stack.
      while (entry.stepStack.length > 0) {
        const step = entry.stepStack.pop();
        if (!step) break;
        step.isStreaming = false;
        if (!success) step.error = "Step failed";
        entry.card.steps.push(step);
      }
      entry.card.isStreaming = false;
      currentFanOut?.cards.push(entry.card);
      activeCards.delete(index);
      // When all cards are closed, restore outer steps.
      if (activeCards.size === 0) {
        stepStack.push(...(savedOuterSteps ?? []));
        savedOuterSteps = null;
      }
    },
    closeFanOut() {
      if (!currentFanOut) return;
      currentFanOut.isStreaming = false;
      result.push(currentFanOut);
      currentFanOut = null;
    },

    // ── Automation lookup across flushed results ────────────────────────────────
    // Search flushed results for an automation item with a matching name.
    // Used on recovery to reuse an existing automation instead of creating a
    // duplicate.
    findExistingAutomation(name?: string): AutomationItem | undefined {
      for (let i = result.length - 1; i >= 0; i--) {
        const item = result[i];
        if (item.kind !== "step") continue;
        for (const child of item.items) {
          if (child.kind === "automation" && (!name || child.automationName === name)) return child;
        }
      }
      return undefined;
    },

    // ── Attempt boundary ──────────────────────────────────────────────────────
    // Close any open (streaming) steps when a new recovery attempt begins.
    // Steps are flushed to result so subsequent events from the new attempt
    // appear after them. Automation items are NOT marked as failed — they
    // will be updated by events from the new attempt to show aggregate
    // progress across all attempts.
    closeInterruptedSteps() {
      for (const step of stepStack.splice(0)) {
        step.isStreaming = false;
        // Only mark non-automation steps as interrupted. Automation steps
        // keep their current state so the new attempt's events can
        // continue updating them (e.g., more subrun_step_completed).
        const hasAutomation = step.items.some((it) => it.kind === "automation");
        if (!hasAutomation) {
          step.error = "Interrupted by server restart";
        }
        result.push(step);
      }
    },

    // ── Flush ────────────────────────────────────────────────────────────────
    // Drains any open state left by a still-streaming response.
    flush(): StepOrGroup[] {
      for (const [, entry] of activeCards) {
        for (const step of entry.stepStack.splice(0).reverse()) entry.card.steps.push(step);
        currentFanOut?.cards.push(entry.card);
      }
      activeCards.clear();
      if (savedOuterSteps) {
        stepStack.push(...savedOuterSteps);
        savedOuterSteps = null;
      }
      if (currentFanOut) result.push(currentFanOut);
      for (const step of stepStack.splice(0).reverse()) result.push(step);
      return result;
    }
  };
}

// ── Domain item construction ──────────────────────────────────────────────────
//
// Pure function: maps a domain event to the TraceItem it should produce.
// Has no access to scope state — add new domain events here without touching
// the main dispatcher.

function buildDomainItem(ev: UiBlock, nextId: (prefix: string) => string): TraceItem | null {
  switch (ev.event_type) {
    case "schema_resolved": {
      const tables = ev.payload.tables ?? [];
      return {
        kind: "artifact",
        id: nextId("schema"),
        toolName: "resolve_schema",
        toolInput: "",
        toolOutput: tables.length ? `Tables: ${tables.join(", ")}` : "Schema resolved",
        durationMs: ev.payload.duration_ms,
        isStreaming: false
      };
    }

    case "triage_completed":
      return {
        kind: "text",
        id: nextId("triage"),
        text: ev.payload.summary ?? "Triage completed",
        isStreaming: false
      };

    case "intent_clarified": {
      const parts: string[] = [];
      if (ev.payload.metrics?.length) parts.push(`Metrics: ${ev.payload.metrics.join(", ")}`);
      if (ev.payload.dimensions?.length)
        parts.push(`Dimensions: ${ev.payload.dimensions.join(", ")}`);
      if (ev.payload.filters?.length) parts.push(`Filters: ${ev.payload.filters.join(", ")}`);
      return {
        kind: "text",
        id: nextId("intent"),
        text: parts.join(" · ") || "Intent clarified",
        isStreaming: false
      };
    }

    case "semantic_shortcut_attempted": {
      const measures = ev.payload.measures ?? [];
      const dimensions = ev.payload.dimensions ?? [];
      const filters = ev.payload.filters ?? [];
      const time_dimensions = ev.payload.time_dimensions ?? [];
      return {
        kind: "artifact",
        id: nextId("sem-shortcut"),
        toolName: "compile_semantic_query",
        toolInput: JSON.stringify({ measures, dimensions, filters, time_dimensions }),
        toolOutput: JSON.stringify({ success: true }),
        isStreaming: true
      };
    }

    // semantic_shortcut_resolved is handled in the main dispatcher
    // so it can update the artifact's toolOutput with the compiled SQL.

    case "spec_resolved": {
      const metrics = ev.payload.resolved_metrics ?? [];
      const tables = ev.payload.resolved_tables ?? [];
      const parts: string[] = [];
      if (metrics.length) parts.push(`Metrics: ${metrics.join(", ")}`);
      if (tables.length) parts.push(`Tables: ${tables.join(", ")}`);
      return {
        kind: "text",
        id: nextId("spec"),
        text: parts.join(" · ") || "Spec resolved",
        isStreaming: false
      };
    }

    case "query_generated":
      return { kind: "sql", id: nextId("sql"), sql: ev.payload.sql, isStreaming: false };

    case "query_executed":
      return {
        kind: "sql",
        id: nextId("sql"),
        sql: ev.payload.query,
        isStreaming: false,
        rowCount: ev.payload.row_count,
        durationMs: ev.payload.duration_ms,
        error: ev.payload.success ? undefined : (ev.payload.error ?? "unknown error"),
        result: [ev.payload.columns].concat(ev.payload.rows ?? []),
        source: ev.payload.source,
        is_preagg: ev.payload.is_preagg ?? false,
        semanticQuery: ev.payload.semantic_query
      };

    case "analytics_validation_failed":
      return {
        kind: "text",
        id: nextId("val"),
        text: ev.payload.reason ?? "Validation failed",
        isStreaming: false
      };

    // ── App-builder domain events ─────────────────────────────────────────
    case "task_plan_ready":
      return {
        kind: "text",
        id: nextId("plan"),
        text: `Plan ready: ${ev.payload.task_count ?? 0} tasks, ${ev.payload.control_count ?? 0} controls`,
        isStreaming: false
      };

    case "task_sql_resolved":
      return {
        kind: "sql",
        id: nextId("sql-ok"),
        sql: ev.payload.sql ?? "",
        isStreaming: false
      };

    case "task_executed":
      return {
        kind: "sql",
        id: nextId("exec-ok"),
        sql: ev.payload.sql ?? "",
        rowCount: ev.payload.row_count,
        result:
          ev.payload.columns && ev.payload.sample_rows
            ? [ev.payload.columns, ...(ev.payload.sample_rows ?? [])]
            : undefined,
        isStreaming: false
      };

    case "app_yaml_ready":
      return {
        kind: "text",
        id: nextId("yaml"),
        text: "App YAML generated",
        isStreaming: false
      };

    case "llm_usage": {
      const parts: string[] = [];
      if (ev.payload.prompt_tokens) parts.push(`${ev.payload.prompt_tokens} input tokens`);
      if (ev.payload.output_tokens) parts.push(`${ev.payload.output_tokens} output tokens`);
      if (ev.payload.duration_ms) parts.push(`${(ev.payload.duration_ms / 1000).toFixed(1)}s`);
      return parts.length
        ? {
            kind: "text",
            id: nextId("usage"),
            text: `LLM: ${parts.join(", ")}`,
            isStreaming: false
          }
        : null;
    }

    default:
      return null;
  }
}

// ── Entry point ───────────────────────────────────────────────────────────────

export function buildAnalyticsSteps(events: UiBlock[]): StepOrGroup[] {
  const scope = createScope();

  // Track whether the most recent awaiting_input was an automation delegation
  // (not human input).  When true, the subsequent step_end "suspended" should
  // NOT close the step — automation events need an open step to attach to.
  let lastAwaitingIsDelegation = false;
  let delegationStepOpen = false;

  for (const ev of events) {
    // Extract sub_spec_index for routing events to the correct card.
    const ssi =
      "sub_spec_index" in ev.payload
        ? (ev.payload as { sub_spec_index?: number | null }).sub_spec_index
        : undefined;

    switch (ev.event_type) {
      // ── Recovery marker (transparent — close any interrupted steps) ──────
      case "recovery_resumed":
        scope.closeInterruptedSteps();
        // Push a completed info step so the user knows we're resuming.
        scope.openStep(
          "Resuming",
          (ev.payload as { message?: string }).message || "Resuming from server restart"
        );
        scope.closeStep("advanced");
        break;

      // ── Step lifecycle ─────────────────────────────────────────────────────
      case "step_start":
        scope.openStep(ev.payload.label, ev.payload.summary ?? undefined, ssi);
        break;
      case "step_end":
        // When the step suspended for a delegation, keep it open so
        // subrun_started / subrun_step_* events can attach to it.
        if (ev.payload.outcome === "suspended" && lastAwaitingIsDelegation) {
          lastAwaitingIsDelegation = false;
          delegationStepOpen = true;
          break;
        }
        scope.closeStep(ev.payload.outcome, ev.payload.metadata, ssi);
        break;
      case "step_summary_update":
        scope.updateSummary(ev.payload.summary, ssi);
        break;

      // ── Thinking stream ────────────────────────────────────────────────────
      case "thinking_start":
        scope.pushItem(
          {
            kind: "thinking",
            id: scope.nextId("think"),
            text: "",
            isStreaming: true
          },
          ssi
        );
        break;
      case "thinking_token": {
        const t = scope.findLastStreaming<ThinkingItem>("thinking", ssi);
        if (t) t.text += ev.payload.token;
        break;
      }
      case "thinking_end": {
        const t = scope.findLastStreaming<ThinkingItem>("thinking", ssi);
        if (t) t.isStreaming = false;
        break;
      }

      // ── Tool calls ─────────────────────────────────────────────────────────
      case "tool_call":
        scope.pushItem(
          {
            kind: "artifact",
            id: scope.nextId("tool"),
            seq: ev.seq,
            toolName: ev.payload.name,
            toolInput: JSON.stringify(ev.payload.input ?? ""),
            llmDurationMs: ev.payload.llm_duration_ms,
            isStreaming: true
          },
          ssi
        );
        break;
      case "tool_result": {
        const a = scope.findLastStreaming<ArtifactItem>("artifact", ssi);
        if (a) {
          a.toolOutput = JSON.stringify(ev.payload.output ?? "");
          a.isError = ev.payload.is_error === true;
          a.durationMs = ev.payload.duration_ms;
          a.isStreaming = false;
        }
        scope.accumulateToolDuration(ev.payload.duration_ms, ssi);
        break;
      }

      // ── Human-in-the-loop ──────────────────────────────────────────────────
      case "awaiting_input": {
        // Detect delegation suspensions: prompts starting with "Executing step:"
        // or "Execute automation" indicate automation delegation, not human input.
        const questions: Array<{ prompt: string }> = ev.payload.questions ?? [];
        const prompt = questions[0]?.prompt ?? "";
        lastAwaitingIsDelegation =
          prompt.startsWith("Executing step:") ||
          // Match the legacy prefix too so runs persisted before the rename
          // still register as automation delegation.
          prompt.startsWith("Execute automation") ||
          prompt.startsWith("Execute procedure") ||
          prompt.startsWith("Delegating to builder") ||
          prompt.startsWith("The analytics pipeline could not");

        if (!lastAwaitingIsDelegation) {
          // These tools suspend before a tool_result is emitted. Mark them as done here
          // so the spinner stops.
          for (const toolName of [
            "ask_user",
            "file_change",
            "write_file",
            "edit_file",
            "delete_file",
            "init_dbt_project",
            "manage_directory"
          ]) {
            const artifact = scope.findLastStreamingArtifactByName(toolName);
            if (artifact) {
              artifact.toolOutput = JSON.stringify({ status: "awaiting_response" });
              artifact.isStreaming = false;
            }
          }
        }
        break;
      }
      case "input_resolved": {
        // If we kept a step open for a delegation, close it now.
        if (delegationStepOpen) {
          delegationStepOpen = false;
          scope.closeStep("advanced", null, ssi);
        }
        // The ask_user artifact lives in a completed (suspended) step that
        // has already been popped from stepStack into result.  Search there.
        if (ev.payload.answer) {
          scope.updateAskUserAnswer(ev.payload.answer);
          scope.updateCompletedArtifactOutput(
            "manage_directory",
            JSON.stringify({ answer: ev.payload.answer })
          );
        }
        break;
      }
      case "file_changed": {
        // init_dbt_project suspends before emitting a tool_result, so its artifact
        // toolOutput stays as {"status":"awaiting_response"} after resume.
        // The first file_changed for a scaffold file (dbt_project.yml) signals
        // success — update the artifact with the real output so the view can render.
        if (ev.payload.file_path.endsWith("/dbt_project.yml")) {
          const pathParts = ev.payload.file_path.split("/");
          const projectName = pathParts[pathParts.length - 2] ?? "";
          const projectDir = pathParts.slice(0, -1).join("/");
          scope.updateCompletedArtifactOutput(
            "init_dbt_project",
            JSON.stringify({ ok: true, project_name: projectName, project_dir: projectDir })
          );
        }
        break;
      }

      // ── Streaming text ─────────────────────────────────────────────────────
      case "text_delta":
        scope.appendTextDelta(ev.payload.token, ssi);
        break;

      // ── Fan-out lifecycle ──────────────────────────────────────────────────
      case "fan_out_start":
        scope.openFanOut(ev.payload.total);
        break;
      case "sub_spec_start":
        scope.openCard(ev.payload.index, ev.payload.label);
        break;
      case "sub_spec_end":
        scope.closeCard(ev.payload.index, ev.payload.success);
        break;
      case "fan_out_end":
        scope.closeFanOut();
        break;

      // ── Automation lifecycle ────────────────────────────────────────────────
      case "subrun_started": {
        // On recovery, a subrun_started event fires again for the same
        // automation. Find the existing item in flushed results and re-open
        // it instead of creating a duplicate.
        const existingProc = scope.findExistingAutomation(ev.payload.subrun_name);
        if (existingProc) {
          existingProc.isStreaming = true;
        } else {
          scope.pushItem({
            kind: "automation",
            id: scope.nextId("proc-run"),
            automationName: ev.payload.subrun_name,
            steps: ev.payload.steps,
            stepsDone: 0,
            isStreaming: true
          });
        }
        break;
      }

      case "subrun_completed": {
        // Check current steps first, then flushed results (recovery case).
        const p = scope.findLastStreaming<AutomationItem>("automation");
        if (p) {
          p.isStreaming = false;
        } else {
          // On recovery the automation item lives in flushed results.
          const existing = scope.findExistingAutomation(ev.payload.subrun_name ?? "");
          if (existing) existing.isStreaming = false;
        }
        break;
      }

      // ── Automation execution progress ───────────────────────────────────────
      case "subrun_step_started":
        scope.pushItem({
          kind: "artifact",
          id: scope.nextId("proc-step"),
          toolName: ev.payload.step,
          toolInput: "Running\u2026",
          isStreaming: true
        });
        break;

      case "subrun_step_completed": {
        // Update the paired streaming artifact by step name
        const artifact = scope.findLastStreamingArtifactByName(ev.payload.step);
        if (artifact) {
          artifact.isStreaming = false;
          artifact.toolOutput = ev.payload.success ? "Completed" : (ev.payload.error ?? "Failed");
        }
        // Also update stepsDone on the automation item.
        // Check current steps first, then flushed results (recovery case).
        if (ev.payload.success) {
          let p = scope.findLastStreaming<AutomationItem>("automation");
          if (!p) {
            // On recovery the automation was flushed — find the most recent one.
            p = scope.findExistingAutomation("") ?? undefined;
          }
          if (p?.steps.some((s) => s.name === ev.payload.step)) p.stepsDone += 1;
        }
        break;
      }

      // ── LLM usage accumulation ──────────────────────────────────────────────
      case "llm_usage":
        scope.accumulateLlmUsage(
          ev.payload.prompt_tokens,
          ev.payload.output_tokens,
          ev.payload.duration_ms,
          ssi,
          ev.payload.model
        );
        break;

      // ── Semantic shortcut resolved — update the artifact with compiled SQL ──
      case "semantic_shortcut_resolved": {
        const sem = scope.findLastStreaming<ArtifactItem>("artifact", ssi);
        if (sem) {
          sem.toolOutput = JSON.stringify({ success: true, sql: ev.payload.sql });
          sem.isStreaming = false;
        }
        break;
      }

      // ── Builder delegation lifecycle ─────────────────────────────────────────
      case "delegation_started": {
        if (typeof ev.payload.target === "string" && ev.payload.target.startsWith("agent:")) {
          scope.pushItem({
            kind: "builder_delegation",
            id: scope.nextId("delegation"),
            childRunId: ev.payload.child_task_id,
            request: ev.payload.request,
            status: "running",
            isStreaming: true
          });
        }
        break;
      }

      case "delegation_completed": {
        const d = scope.findLastStreaming<BuilderDelegationItem>("builder_delegation");
        if (d) {
          d.status = ev.payload.success ? "done" : "failed";
          d.answer = ev.payload.answer;
          d.error = ev.payload.error;
          d.isStreaming = false;
        }
        break;
      }

      // ── Builder tool_used: intercept init_dbt_project error signal ───────────
      case "tool_used": {
        // Error summaries from the init_dbt_project resume path are encoded as
        // "error:<project_name>:<message>" so we can update the artifact output
        // without a dedicated event type.
        if (
          ev.payload.tool_name === "init_dbt_project" &&
          ev.payload.summary.startsWith("error:")
        ) {
          const parts = ev.payload.summary.split(":");
          const errorMsg = parts.slice(2).join(":");
          scope.updateCompletedArtifactOutput(
            "init_dbt_project",
            JSON.stringify({ ok: false, error: errorMsg })
          );
        } else {
          const item = buildDomainItem(ev, scope.nextId);
          if (item) scope.pushItem(item, ssi);
        }
        break;
      }

      // ── Domain events (pure item construction) ─────────────────────────────
      default: {
        const item = buildDomainItem(ev, scope.nextId);
        if (item) scope.pushItem(item, ssi);
      }
    }
  }

  return scope.flush();
}
