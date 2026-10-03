import { describe, expect, it } from "vitest";
import type { UiBlock } from "@/services/api/analytics";
import { buildAnalyticsSteps } from "./analyticsSteps";

// ── helpers ───────────────────────────────────────────────────────────────────

let seq = 0;
const ev = <T extends UiBlock["event_type"]>(
  type: T,
  payload: Extract<UiBlock, { event_type: T }>["payload"]
): UiBlock => ({ seq: seq++, event_type: type, payload }) as UiBlock;

const stepStart = (label: string, subSpecIndex?: number) =>
  ev("step_start", { label, ...(subSpecIndex != null ? { sub_spec_index: subSpecIndex } : {}) });
const stepEnd = (
  outcome: "advanced" | "failed" | "suspended" = "advanced",
  subSpecIndex?: number
) =>
  ev("step_end", {
    label: "",
    outcome,
    ...(subSpecIndex != null ? { sub_spec_index: subSpecIndex } : {})
  });
const fanOutStart = (total: number) => ev("fan_out_start", { total });
const subSpecStart = (index: number, label: string) =>
  ev("sub_spec_start", { index, total: 0, label });
const subSpecEnd = (index: number, success = true) => ev("sub_spec_end", { index, success });
const fanOutEnd = () => ev("fan_out_end", { success: true });
const queryExecuted = (sql = "SELECT 1", source: "semantic" | "llm" | "vendor" = "llm") =>
  ev("query_executed", {
    query: sql,
    row_count: 1,
    duration_ms: 10,
    success: true,
    columns: ["id"],
    rows: [["1"]],
    source
  });

// ── basic step behaviour ──────────────────────────────────────────────────────

describe("buildAnalyticsSteps — basic steps", () => {
  it("returns empty for no events", () => {
    expect(buildAnalyticsSteps([])).toEqual([]);
  });

  it("open step (streaming) appears in result", () => {
    const items = buildAnalyticsSteps([stepStart("Analyzing")]);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "step", label: "Analyzing", isStreaming: true });
  });

  it("closed step is marked not streaming", () => {
    const items = buildAnalyticsSteps([stepStart("Analyzing"), stepEnd()]);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "step", label: "Analyzing", isStreaming: false });
  });

  it("failed step_end sets error", () => {
    const items = buildAnalyticsSteps([stepStart("Solving"), stepEnd("failed")]);
    expect(items[0]).toMatchObject({ kind: "step", error: "Step failed" });
  });

  it("query_executed source is propagated to SqlItem", () => {
    const steps = buildAnalyticsSteps([
      stepStart("Executing"),
      queryExecuted("SELECT 1", "semantic"),
      stepEnd()
    ]);
    const step = steps[0] as { items: { kind: string; source?: string }[] };
    expect(step.items[0]).toMatchObject({ kind: "sql", source: "semantic" });
  });

  it("sequential steps are all in result", () => {
    const items = buildAnalyticsSteps([
      stepStart("A"),
      stepEnd(),
      stepStart("B"),
      stepEnd(),
      stepStart("C")
    ]);
    expect(items).toHaveLength(3);
    expect(items.map((i) => (i as { label: string }).label)).toEqual(["A", "B", "C"]);
  });

  it("thinking items accumulate inside a step", () => {
    const items = buildAnalyticsSteps([
      stepStart("S"),
      ev("thinking_start", {}),
      ev("thinking_token", { token: "hell" }),
      ev("thinking_token", { token: "o" }),
      ev("thinking_end", {}),
      stepEnd()
    ]);
    const step = items[0] as { items: { kind: string; text: string; isStreaming: boolean }[] };
    expect(step.items).toHaveLength(1);
    expect(step.items[0]).toMatchObject({ kind: "thinking", text: "hello", isStreaming: false });
  });

  it("text_delta tokens append to a single text item", () => {
    const items = buildAnalyticsSteps([
      stepStart("S"),
      ev("text_delta", { token: "foo" }),
      ev("text_delta", { token: "bar" }),
      stepEnd()
    ]);
    const step = items[0] as { items: { kind: string; text: string }[] };
    expect(step.items).toHaveLength(1);
    expect(step.items[0]).toMatchObject({ kind: "text", text: "foobar" });
  });

  it("tool_call is paired with tool_result", () => {
    const items = buildAnalyticsSteps([
      stepStart("S"),
      ev("tool_call", { name: "run_sql", input: { sql: "SELECT 1" } }),
      ev("tool_result", { name: "run_sql", output: { rows: [] }, duration_ms: 42 }),
      stepEnd()
    ]);
    const step = items[0] as { items: { kind: string; toolOutput: string; durationMs: number }[] };
    expect(step.items).toHaveLength(1);
    expect(step.items[0]).toMatchObject({ kind: "artifact", durationMs: 42, isStreaming: false });
  });
});

// ── fan-out: completed stream ──────────────────────────────────────────────────

describe("buildAnalyticsSteps — fan-out (complete)", () => {
  it("fan-out group appears in result after fan_out_end", () => {
    const items = buildAnalyticsSteps([
      fanOutStart(2),
      subSpecStart(0, "Q1"),
      stepStart("Solving", 0),
      stepEnd("advanced", 0),
      subSpecEnd(0),
      subSpecStart(1, "Q2"),
      stepStart("Solving", 1),
      stepEnd("advanced", 1),
      subSpecEnd(1),
      fanOutEnd()
    ]);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "fan_out", total: 2, isStreaming: false });
  });

  it("each card contains its own steps", () => {
    const items = buildAnalyticsSteps([
      fanOutStart(2),
      subSpecStart(0, "Q1"),
      stepStart("Solving", 0),
      stepEnd("advanced", 0),
      queryExecuted("SELECT 1"),
      subSpecEnd(0),
      subSpecStart(1, "Q2"),
      stepStart("Solving", 1),
      stepEnd("advanced", 1),
      queryExecuted("SELECT 2"),
      subSpecEnd(1),
      fanOutEnd()
    ]);
    const group = items[0] as { cards: { steps: { kind: string }[]; label: string }[] };
    expect(group.cards).toHaveLength(2);
    expect(group.cards[0].label).toBe("Q1");
    expect(group.cards[0].steps).toHaveLength(1);
    expect(group.cards[1].label).toBe("Q2");
    expect(group.cards[1].steps).toHaveLength(1);
  });

  it("steps that closed inside a card do NOT appear in top-level result", () => {
    const items = buildAnalyticsSteps([
      fanOutStart(1),
      subSpecStart(0, "Q1"),
      stepStart("Solving", 0),
      stepEnd("advanced", 0),
      subSpecEnd(0),
      fanOutEnd()
    ]);
    // Only the fan_out group should be at the top level
    expect(items).toHaveLength(1);
    expect(items[0].kind).toBe("fan_out");
  });

  it("card steps contain domain items when routed via sub_spec_index", () => {
    const items = buildAnalyticsSteps([
      fanOutStart(1),
      subSpecStart(0, "Q1"),
      stepStart("Executing", 0),
      ev("text_delta", { token: "analysis result", sub_spec_index: 0 }),
      stepEnd("advanced", 0),
      subSpecEnd(0),
      fanOutEnd()
    ]);
    const group = items[0] as { cards: { steps: { items: { kind: string }[] }[] }[] };
    const cardStep = group.cards[0].steps[0];
    expect(cardStep.items).toHaveLength(1);
    expect(cardStep.items[0].kind).toBe("text");
  });
});

// ── fan-out: mid-stream (flush) ────────────────────────────────────────────────

describe("buildAnalyticsSteps — fan-out (streaming / flush)", () => {
  it("active card is visible before sub_spec_end (flush path)", () => {
    // sub_spec_start fired, step inside it has already closed, sub_spec_end not yet
    const items = buildAnalyticsSteps([
      fanOutStart(2),
      subSpecStart(0, "Q1"),
      stepStart("Solving", 0),
      stepEnd("advanced", 0)
      // ← sub_spec_end has NOT arrived yet
    ]);
    expect(items).toHaveLength(1);
    expect(items[0].kind).toBe("fan_out");

    const group = items[0] as { cards: { steps: unknown[]; label: string }[] };
    expect(group.cards).toHaveLength(1);
    expect(group.cards[0].label).toBe("Q1");
    expect(group.cards[0].steps).toHaveLength(1);
  });

  it("open step inside streaming card is visible (flush path)", () => {
    const items = buildAnalyticsSteps([
      fanOutStart(1),
      subSpecStart(0, "Q1"),
      stepStart("Solving", 0)
      // step_end has NOT arrived
    ]);
    const group = items[0] as { cards: { steps: { isStreaming: boolean }[] }[] };
    expect(group.cards[0].steps).toHaveLength(1);
    expect(group.cards[0].steps[0].isStreaming).toBe(true);
  });

  it("no sub_spec_start yet: fan_out group has no cards", () => {
    const items = buildAnalyticsSteps([fanOutStart(3)]);
    expect(items).toHaveLength(1);
    const group = items[0] as { cards: unknown[] };
    expect(group.cards).toHaveLength(0);
  });

  it("completed card + in-progress card both visible while streaming", () => {
    const items = buildAnalyticsSteps([
      fanOutStart(2),
      subSpecStart(0, "Q1"),
      stepStart("Solving", 0),
      stepEnd("advanced", 0),
      subSpecEnd(0),
      subSpecStart(1, "Q2"),
      stepStart("Solving", 1)
      // Q2's step_end and sub_spec_end not yet received
    ]);
    const group = items[0] as { cards: { label: string; steps: unknown[] }[] };
    expect(group.cards).toHaveLength(2);
    expect(group.cards[0].label).toBe("Q1");
    expect(group.cards[0].steps).toHaveLength(1);
    expect(group.cards[1].label).toBe("Q2");
    expect(group.cards[1].steps).toHaveLength(1); // open step flushed into card
  });
});

// ── automation steps ───────────────────────────────────────────────────────────

describe("buildAnalyticsSteps — automation steps", () => {
  it("subrun_step_started creates a streaming artifact with toolInput 'Running…'", () => {
    const items = buildAnalyticsSteps([
      stepStart("Executing"),
      ev("subrun_step_started", { step: "Run monthly report" })
    ]);
    const step = items[0] as {
      items: { kind: string; toolName: string; toolInput: string; isStreaming: boolean }[];
    };
    expect(step.items).toHaveLength(1);
    expect(step.items[0]).toMatchObject({
      kind: "artifact",
      toolName: "Run monthly report",
      toolInput: "Running\u2026",
      isStreaming: true
    });
  });

  it("subrun_step_completed (success) closes the item with 'Completed'", () => {
    const items = buildAnalyticsSteps([
      stepStart("Executing"),
      ev("subrun_step_started", { step: "Run monthly report" }),
      ev("subrun_step_completed", { step: "Run monthly report", success: true })
    ]);
    const step = items[0] as { items: { toolOutput: string; isStreaming: boolean }[] };
    expect(step.items[0]).toMatchObject({ toolOutput: "Completed", isStreaming: false });
  });

  it("subrun_step_completed (failure) sets error string as toolOutput", () => {
    const items = buildAnalyticsSteps([
      stepStart("Executing"),
      ev("subrun_step_started", { step: "Run monthly report" }),
      ev("subrun_step_completed", {
        step: "Run monthly report",
        success: false,
        error: "Connection refused"
      })
    ]);
    const step = items[0] as { items: { toolOutput: string; isStreaming: boolean }[] };
    expect(step.items[0]).toMatchObject({ toolOutput: "Connection refused", isStreaming: false });
  });

  it("subrun_step_completed with no error message falls back to 'Failed'", () => {
    const items = buildAnalyticsSteps([
      stepStart("S"),
      ev("subrun_step_started", { step: "Step A" }),
      ev("subrun_step_completed", { step: "Step A", success: false })
    ]);
    const step = items[0] as { items: { toolOutput: string }[] };
    expect(step.items[0]).toMatchObject({ toolOutput: "Failed" });
  });

  it("multiple concurrent steps are independently paired by name", () => {
    const items = buildAnalyticsSteps([
      stepStart("S"),
      ev("subrun_step_started", { step: "Step A" }),
      ev("subrun_step_started", { step: "Step B" }),
      ev("subrun_step_completed", { step: "Step A", success: true }),
      ev("subrun_step_completed", { step: "Step B", success: false, error: "oops" })
    ]);
    const step = items[0] as {
      items: { toolName: string; toolOutput: string; isStreaming: boolean }[];
    };
    expect(step.items).toHaveLength(2);
    expect(step.items[0]).toMatchObject({
      toolName: "Step A",
      toolOutput: "Completed",
      isStreaming: false
    });
    expect(step.items[1]).toMatchObject({
      toolName: "Step B",
      toolOutput: "oops",
      isStreaming: false
    });
  });

  it("unmatched subrun_step_completed (no prior start) is a no-op", () => {
    const items = buildAnalyticsSteps([
      stepStart("S"),
      ev("subrun_step_completed", { step: "Ghost step", success: true })
    ]);
    const step = items[0] as { items: unknown[] };
    expect(step.items).toHaveLength(0);
  });

  it("still-streaming step (no completed yet) stays streaming with no toolOutput", () => {
    const items = buildAnalyticsSteps([
      stepStart("S"),
      ev("subrun_step_started", { step: "Long running step" })
    ]);
    const step = items[0] as { items: { toolOutput: unknown; isStreaming: boolean }[] };
    expect(step.items[0]).toMatchObject({ isStreaming: true });
    expect(step.items[0].toolOutput).toBeUndefined();
  });
});

// ── automation item stepsDone tracking ─────────────────────────────────────────

type AutomationItemShape = {
  kind: "automation";
  automationName: string;
  steps: { name: string; task_type: string }[];
  stepsDone: number;
  isStreaming: boolean;
};

const MAIN_STEPS = [
  { name: "fetch_data", task_type: "execute_sql" },
  { name: "process_data", task_type: "execute_sql" },
  { name: "generate_report", task_type: "formatter" }
];

const automationStarted = (name = "my_proc", steps = MAIN_STEPS) =>
  ev("subrun_started", { subrun_name: name, steps });

const procStepStarted = (step: string) => ev("subrun_step_started", { step });

const procStepCompleted = (step: string, success = true, error?: string) =>
  ev("subrun_step_completed", { step, success, ...(error ? { error } : {}) });

const procCompleted = (name = "my_proc", success = true) =>
  ev("subrun_completed", { subrun_name: name, success });

const getProcItem = (items: ReturnType<typeof buildAnalyticsSteps>): AutomationItemShape => {
  for (const node of items) {
    if (node.kind !== "step") continue;
    const found = (node as { items: unknown[] }).items.find(
      (i) => (i as { kind: string }).kind === "automation"
    );
    if (found) return found as AutomationItemShape;
  }
  throw new Error("no automation item found");
};

describe("buildAnalyticsSteps — automation item stepsDone", () => {
  it("starts at 0 with no step events", () => {
    const items = buildAnalyticsSteps([stepStart("Running"), automationStarted()]);
    expect(getProcItem(items).stepsDone).toBe(0);
  });

  it("increments for each successful main-step completion", () => {
    const items = buildAnalyticsSteps([
      stepStart("Running"),
      automationStarted(),
      procStepStarted("fetch_data"),
      procStepCompleted("fetch_data"),
      procStepStarted("process_data"),
      procStepCompleted("process_data")
    ]);
    expect(getProcItem(items).stepsDone).toBe(2);
  });

  it("does NOT increment for a failed main step", () => {
    const items = buildAnalyticsSteps([
      stepStart("Running"),
      automationStarted(),
      procStepStarted("fetch_data"),
      procStepCompleted("fetch_data", false, "timeout")
    ]);
    expect(getProcItem(items).stepsDone).toBe(0);
  });

  it("does NOT increment for loop_sequential sub-step completions", () => {
    const steps = [{ name: "loop_step", task_type: "loop_sequential" }];
    const items = buildAnalyticsSteps([
      stepStart("Running"),
      automationStarted("p", steps),
      procStepStarted("loop_step"),
      // sub-steps have names not in the top-level list
      procStepStarted("sub_1"),
      procStepCompleted("sub_1"),
      procStepStarted("sub_2"),
      procStepCompleted("sub_2")
    ]);
    expect(getProcItem(items).stepsDone).toBe(0);
  });

  it("increments only for main-step completions when sub-steps are mixed in", () => {
    const steps = [
      { name: "prepare", task_type: "execute_sql" },
      { name: "loop_step", task_type: "loop_sequential" },
      { name: "finalize", task_type: "execute_sql" }
    ];
    const items = buildAnalyticsSteps([
      stepStart("Running"),
      automationStarted("p", steps),
      procStepStarted("prepare"),
      procStepCompleted("prepare"), // +1
      procStepStarted("loop_step"),
      procStepStarted("sub_1"),
      procStepCompleted("sub_1"), // sub-step — no increment
      procStepStarted("sub_2"),
      procStepCompleted("sub_2"), // sub-step — no increment
      procStepCompleted("loop_step"), // +1
      procStepStarted("finalize"),
      procStepCompleted("finalize") // +1
    ]);
    expect(getProcItem(items).stepsDone).toBe(3);
  });

  it("reaches total steps when all main steps complete", () => {
    const items = buildAnalyticsSteps([
      stepStart("Running"),
      automationStarted(),
      procStepStarted("fetch_data"),
      procStepCompleted("fetch_data"),
      procStepStarted("process_data"),
      procStepCompleted("process_data"),
      procStepStarted("generate_report"),
      procStepCompleted("generate_report"),
      procCompleted(),
      stepEnd()
    ]);
    const proc = getProcItem(items);
    expect(proc.stepsDone).toBe(MAIN_STEPS.length);
    expect(proc.isStreaming).toBe(false);
  });

  it("stepsDone is stable after subrun_completed (no further increments)", () => {
    const items = buildAnalyticsSteps([
      stepStart("Running"),
      automationStarted(),
      procStepStarted("fetch_data"),
      procStepCompleted("fetch_data"),
      procCompleted(),
      // ghost completion after automation is done — should not increment
      procStepCompleted("fetch_data")
    ]);
    // automation is now not streaming, but stepsDone should still be 1
    expect(getProcItem(items).stepsDone).toBe(1);
  });
});

// ── outer steps around fan-out ─────────────────────────────────────────────────

describe("buildAnalyticsSteps — outer steps around fan-out", () => {
  it("outer step before fan-out appears in result alongside the group", () => {
    const items = buildAnalyticsSteps([
      stepStart("Outer"),
      fanOutStart(1),
      subSpecStart(0, "Q1"),
      stepStart("Inner", 0),
      stepEnd("advanced", 0),
      subSpecEnd(0),
      fanOutEnd(),
      stepEnd() // outer closes after fan-out
    ]);
    expect(items).toHaveLength(2);
    expect(items[0].kind).toBe("fan_out");
    expect(items[1]).toMatchObject({ kind: "step", label: "Outer" });
  });

  it("outer step items accumulated before fan-out are preserved", () => {
    const items = buildAnalyticsSteps([
      stepStart("Outer"),
      ev("text_delta", { token: "prefix" }),
      fanOutStart(1),
      subSpecStart(0, "Q1"),
      stepStart("Inner", 0),
      stepEnd("advanced", 0),
      subSpecEnd(0),
      fanOutEnd(),
      stepEnd()
    ]);
    const outerStep = items[1] as { items: { kind: string; text: string }[] };
    expect(outerStep.items).toHaveLength(1);
    expect(outerStep.items[0]).toMatchObject({ kind: "text", text: "prefix" });
  });

  it("inner card items do NOT bleed into outer step", () => {
    const items = buildAnalyticsSteps([
      stepStart("Outer"),
      fanOutStart(1),
      subSpecStart(0, "Q1"),
      stepStart("Inner", 0),
      ev("text_delta", { token: "inner-text", sub_spec_index: 0 }),
      stepEnd("advanced", 0),
      subSpecEnd(0),
      fanOutEnd(),
      stepEnd()
    ]);
    const outerStep = items[1] as { items: unknown[] };
    expect(outerStep.items).toHaveLength(0);
  });
});

// ── concurrent fan-out ──────────────────────────────────────────────────────

describe("buildAnalyticsSteps — concurrent fan-out", () => {
  it("interleaved fan-out cards build correctly", () => {
    const items = buildAnalyticsSteps([
      fanOutStart(3),
      subSpecStart(0, "Query 1 of 3"),
      subSpecStart(1, "Query 2 of 3"),
      stepStart("solving", 0),
      stepStart("solving", 1),
      // Card 1 finishes first
      stepEnd("advanced", 1),
      subSpecEnd(1, true),
      stepEnd("advanced", 0),
      subSpecEnd(0, true),
      subSpecStart(2, "Query 3 of 3"),
      stepStart("solving", 2),
      stepEnd("advanced", 2),
      subSpecEnd(2, true),
      fanOutEnd()
    ]);

    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "fan_out", isStreaming: false });

    const group = items[0] as { cards: { label: string; steps: { label: string }[] }[] };
    expect(group.cards).toHaveLength(3);

    // Card 1 closed first, so it appears first in cards array
    expect(group.cards[0].label).toBe("Query 2 of 3");
    expect(group.cards[0].steps).toHaveLength(1);
    expect(group.cards[0].steps[0].label).toBe("solving");

    expect(group.cards[1].label).toBe("Query 1 of 3");
    expect(group.cards[1].steps).toHaveLength(1);
    expect(group.cards[1].steps[0].label).toBe("solving");

    expect(group.cards[2].label).toBe("Query 3 of 3");
    expect(group.cards[2].steps).toHaveLength(1);
    expect(group.cards[2].steps[0].label).toBe("solving");
  });

  it("concurrent cards mid-stream flush", () => {
    // Open 2 cards but don't close them, then flush
    const items = buildAnalyticsSteps([
      fanOutStart(3),
      subSpecStart(0, "Card A"),
      stepStart("analyzing", 0),
      subSpecStart(1, "Card B"),
      stepStart("analyzing", 1)
      // Neither card is closed — flush happens at end of buildAnalyticsSteps
    ]);

    expect(items).toHaveLength(1);
    expect(items[0].kind).toBe("fan_out");

    const group = items[0] as {
      cards: { label: string; isStreaming: boolean; steps: { isStreaming: boolean }[] }[];
    };
    expect(group.cards).toHaveLength(2);

    expect(group.cards[0].isStreaming).toBe(true);
    expect(group.cards[0].steps).toHaveLength(1);
    expect(group.cards[0].steps[0].isStreaming).toBe(true);

    expect(group.cards[1].isStreaming).toBe(true);
    expect(group.cards[1].steps).toHaveLength(1);
    expect(group.cards[1].steps[0].isStreaming).toBe(true);
  });

  it("events without sub_spec_index go to outer scope", () => {
    const items = buildAnalyticsSteps([
      stepStart("Before fan-out"),
      ev("text_delta", { token: "before" }),
      stepEnd(),
      fanOutStart(1),
      subSpecStart(0, "Q1"),
      stepStart("solving", 0),
      stepEnd("advanced", 0),
      subSpecEnd(0),
      fanOutEnd(),
      stepStart("After fan-out"),
      ev("text_delta", { token: "after" }),
      stepEnd()
    ]);

    // Should have: outer step "Before fan-out", fan_out group, outer step "After fan-out"
    expect(items).toHaveLength(3);

    const kinds = items.map((i) => i.kind);
    expect(kinds).toContain("fan_out");

    const beforeStep = items.find(
      (i) => i.kind === "step" && (i as { label: string }).label === "Before fan-out"
    ) as { kind: string; label: string; items: { kind: string; text: string }[] };
    expect(beforeStep).toBeDefined();
    expect(beforeStep.items).toHaveLength(1);
    expect(beforeStep.items[0]).toMatchObject({ kind: "text", text: "before" });

    const afterStep = items.find(
      (i) => i.kind === "step" && (i as { label: string }).label === "After fan-out"
    ) as { kind: string; label: string; items: { kind: string; text: string }[] };
    expect(afterStep).toBeDefined();
    expect(afterStep.items).toHaveLength(1);
    expect(afterStep.items[0]).toMatchObject({ kind: "text", text: "after" });
  });

  it("backward compatible — serial fan-out still works", () => {
    // Serial pattern: events inside sub_spec use sub_spec_index routing
    // This mirrors the existing serial test pattern but with correct helpers
    const items = buildAnalyticsSteps([
      fanOutStart(2),
      subSpecStart(0, "Q1"),
      stepStart("Solving", 0),
      stepEnd("advanced", 0),
      subSpecEnd(0),
      subSpecStart(1, "Q2"),
      stepStart("Solving", 1),
      stepEnd("advanced", 1),
      subSpecEnd(1),
      fanOutEnd()
    ]);

    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "fan_out", total: 2, isStreaming: false });

    const group = items[0] as { cards: { label: string; steps: { label: string }[] }[] };
    expect(group.cards).toHaveLength(2);
    expect(group.cards[0].label).toBe("Q1");
    expect(group.cards[0].steps).toHaveLength(1);
    expect(group.cards[0].steps[0].label).toBe("Solving");
    expect(group.cards[1].label).toBe("Q2");
    expect(group.cards[1].steps).toHaveLength(1);
    expect(group.cards[1].steps[0].label).toBe("Solving");
  });
});

// ── delegation suspension — automation events attach to the open step ─────────

const awaitingDelegation = (prompt = "Executing step: run_proc") =>
  ev("awaiting_input", { questions: [{ prompt, suggestions: [] }] });

const awaitingHuman = (prompt = "What database?") =>
  ev("awaiting_input", { questions: [{ prompt, suggestions: [] }] });

const inputResolved = (answer = "done") => ev("input_resolved", { answer });

describe("buildAnalyticsSteps — delegation suspension", () => {
  it("automation events attach to step kept open during delegation", () => {
    const items = buildAnalyticsSteps([
      stepStart("Executing"),
      awaitingDelegation(),
      stepEnd("suspended"),
      automationStarted(),
      procStepStarted("fetch_data"),
      procStepCompleted("fetch_data"),
      procStepStarted("process_data"),
      procStepCompleted("process_data"),
      procStepStarted("generate_report"),
      procStepCompleted("generate_report"),
      procCompleted(),
      inputResolved()
    ]);

    expect(items).toHaveLength(1);
    const step = items[0] as { kind: string; items: { kind: string }[] };
    expect(step.kind).toBe("step");
    const procItems = step.items.filter((i) => i.kind === "automation");
    expect(procItems).toHaveLength(1);
    expect(getProcItem(items).stepsDone).toBe(3);
  });

  it("automation pill appears in collapsed step row", () => {
    const items = buildAnalyticsSteps([
      stepStart("Executing"),
      awaitingDelegation(),
      stepEnd("suspended"),
      automationStarted("my_proc"),
      procCompleted("my_proc"),
      inputResolved()
    ]);

    const step = items[0] as { items: { kind: string; automationName?: string }[] };
    const proc = step.items.find((i) => i.kind === "automation");
    expect(proc).toBeDefined();
    expect(proc?.automationName).toBe("my_proc");
  });

  it("human input suspension still closes the step normally", () => {
    const items = buildAnalyticsSteps([
      stepStart("Clarifying"),
      awaitingHuman(),
      stepEnd("suspended"),
      // After human responds, a new step starts
      inputResolved("PostgreSQL"),
      stepStart("Solving"),
      stepEnd()
    ]);

    // Both steps should be closed (not kept open)
    expect(items).toHaveLength(2);
    expect(items[0]).toMatchObject({ kind: "step", label: "Clarifying", isStreaming: false });
    expect(items[1]).toMatchObject({ kind: "step", label: "Solving", isStreaming: false });
  });

  it("delegation step closes on input_resolved", () => {
    const items = buildAnalyticsSteps([
      stepStart("Executing"),
      awaitingDelegation(),
      stepEnd("suspended"),
      automationStarted(),
      procCompleted(),
      inputResolved()
    ]);

    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "step", isStreaming: false });
  });

  it("delegation step stays open (streaming) while automation is running", () => {
    const items = buildAnalyticsSteps([
      stepStart("Executing"),
      awaitingDelegation(),
      stepEnd("suspended"),
      automationStarted(),
      procStepStarted("fetch_data")
      // no input_resolved yet — still running
    ]);

    expect(items).toHaveLength(1);
    // Step is still streaming (kept open)
    const step = items[0] as { kind: string; isStreaming: boolean; items: { kind: string }[] };
    expect(step.isStreaming).toBe(true);
    expect(step.items.some((i) => i.kind === "automation")).toBe(true);
  });
});

// ── recovery attempt boundary ────────────────────────────────────────────────

// The payload the backend writes for the marker (agentic/pipeline/src/recovery.rs).
// The attempt number is a column on the event row, not part of the payload.
const RESUME_MESSAGE = "Resuming from server restart";
const recoveryResumed = (message = RESUME_MESSAGE) => ev("recovery_resumed", { message });
// What a recovery marker renders as: a closed, non-failed info step.
const resumingStep = (summary = RESUME_MESSAGE) => ({
  kind: "step",
  label: "Resuming",
  summary,
  isStreaming: false
});

describe("buildAnalyticsSteps — recovery_resumed (recovery)", () => {
  it("open non-automation step is marked interrupted on recovery_resumed", () => {
    const items = buildAnalyticsSteps([
      stepStart("Solving"),
      ev("text_delta", { token: "partial" }),
      recoveryResumed(),
      stepStart("Solving again"),
      stepEnd()
    ]);

    expect(items).toHaveLength(3);
    expect(items[0]).toMatchObject({
      kind: "step",
      label: "Solving",
      isStreaming: false,
      error: "Interrupted by server restart"
    });
    // The marker itself shows up as an info step between the two attempts.
    expect(items[1]).toMatchObject(resumingStep());
    expect((items[1] as { error?: string }).error).toBeUndefined();
    expect(items[2]).toMatchObject({
      kind: "step",
      label: "Solving again",
      isStreaming: false
    });
  });

  it("the Resuming step shows the backend's message, with a fallback when it is absent", () => {
    const [custom] = buildAnalyticsSteps([recoveryResumed("Picking up after a deploy")]);
    expect(custom).toMatchObject(resumingStep("Picking up after a deploy"));

    const [fallback] = buildAnalyticsSteps([ev("recovery_resumed", {})]);
    expect(fallback).toMatchObject(resumingStep());
  });

  it("automation step is NOT marked interrupted — kept for aggregation", () => {
    const items = buildAnalyticsSteps([
      stepStart("Executing"),
      awaitingDelegation(),
      stepEnd("suspended"),
      automationStarted(),
      procStepStarted("fetch_data"),
      procStepCompleted("fetch_data"),
      procStepStarted("process_data"),
      // crash mid-step
      recoveryResumed(),
      // new attempt re-emits subrun_started + continues
      automationStarted(),
      procStepStarted("process_data"),
      procStepCompleted("process_data"),
      procStepStarted("generate_report"),
      procStepCompleted("generate_report"),
      procCompleted(),
      inputResolved()
    ]);

    // The step containing the automation should NOT have an error
    const step = items[0] as { kind: string; error?: string; items: { kind: string }[] };
    expect(step.error).toBeUndefined();
    // The automation item should aggregate steps from both attempts
    const proc = getProcItem(items);
    expect(proc.stepsDone).toBe(3); // fetch_data + process_data + generate_report
    expect(proc.isStreaming).toBe(false);
  });

  it("automation stepsDone does not double-count steps completed before and after recovery", () => {
    const items = buildAnalyticsSteps([
      stepStart("Executing"),
      awaitingDelegation(),
      stepEnd("suspended"),
      automationStarted(),
      procStepStarted("fetch_data"),
      procStepCompleted("fetch_data"), // +1
      // crash
      recoveryResumed(),
      // recovery re-emits subrun_started, resumes from fetch_data
      automationStarted(),
      procStepStarted("fetch_data"),
      procStepCompleted("fetch_data"), // already counted — but stepsDone increments again
      procStepStarted("process_data"),
      procStepCompleted("process_data"), // +1
      procCompleted(),
      inputResolved()
    ]);

    const proc = getProcItem(items);
    // fetch_data counted twice (once per attempt) + process_data = 3
    // This is acceptable: the automation item shows progress, not unique completions
    expect(proc.stepsDone).toBe(3);
  });

  it("automation steps after recovery (no new subrun_started) still update progress", () => {
    // Real payload: subrun_started at attempt 0, some steps complete,
    // then recovery_resumed(1), recovery_resumed(2), then more subrun_step_*
    // events WITHOUT a new subrun_started event.
    const steps = [
      { name: "temperature_correlation", task_type: "execute_sql" },
      { name: "fuel_price_impact", task_type: "execute_sql" },
      { name: "unemployment_impact", task_type: "execute_sql" },
      { name: "correlation_matrix", task_type: "execute_sql" },
      { name: "combined_factors_analysis", task_type: "execute_sql" },
      { name: "factor_significance", task_type: "execute_sql" },
      { name: "external_factors_summary", task_type: "formatter" }
    ];
    const items = buildAnalyticsSteps([
      stepStart("Executing"),
      awaitingDelegation(),
      stepEnd("suspended"),
      automationStarted("external_factors_correlation", steps),
      procStepStarted("temperature_correlation"),
      procStepCompleted("temperature_correlation"),
      procStepStarted("fuel_price_impact"),
      // crash mid fuel_price_impact
      recoveryResumed(),
      recoveryResumed(),
      // recovery continues without new subrun_started
      procStepStarted("fuel_price_impact"),
      procStepCompleted("fuel_price_impact"),
      procStepStarted("unemployment_impact"),
      procStepCompleted("unemployment_impact"),
      procStepStarted("correlation_matrix"),
      procStepCompleted("correlation_matrix"),
      procStepStarted("combined_factors_analysis"),
      procStepCompleted("combined_factors_analysis"),
      procStepStarted("factor_significance"),
      procStepCompleted("factor_significance"),
      procStepStarted("external_factors_summary"),
      procStepCompleted("external_factors_summary"),
      procCompleted("external_factors_correlation"),
      inputResolved()
    ]);

    const proc = getProcItem(items);
    // temperature_correlation(attempt 0) + all 6 remaining from attempt 2 = 7
    expect(proc.stepsDone).toBe(7);
    expect(proc.isStreaming).toBe(false);

    // Verify artifacts exist for automation steps — the step containing
    // the automation should have artifact items for each proc step.
    const procStep = items.find(
      (i) =>
        i.kind === "step" &&
        (i as { items: { kind: string }[] }).items.some((it) => it.kind === "automation")
    ) as { items: { kind: string; toolName?: string; isStreaming: boolean }[] };
    const artifacts = procStep.items.filter((i) => i.kind === "artifact");
    // Should have artifacts for: temperature_correlation(attempt 0),
    // fuel_price_impact(started attempt 0 but not completed),
    // then from attempt 2: fuel_price_impact, unemployment_impact,
    // correlation_matrix, combined_factors_analysis, factor_significance,
    // external_factors_summary
    // Total: at least 7 completed artifacts
    const completedArtifacts = artifacts.filter((a) => !a.isStreaming);
    expect(completedArtifacts.length).toBeGreaterThanOrEqual(7);
  });

  it("recovery_resumed with no open steps leaves closed steps untouched", () => {
    const items = buildAnalyticsSteps([
      stepStart("A"),
      stepEnd(),
      recoveryResumed(),
      stepStart("B"),
      stepEnd()
    ]);

    // Nothing was open, so nothing is marked interrupted — only the marker is added.
    expect(items).toHaveLength(3);
    expect(items[0]).toMatchObject({ kind: "step", label: "A" });
    expect((items[0] as { error?: string }).error).toBeUndefined();
    expect(items[1]).toMatchObject(resumingStep());
    expect(items[2]).toMatchObject({ kind: "step", label: "B" });
    expect((items[2] as { error?: string }).error).toBeUndefined();
  });

  it("multiple attempt boundaries work correctly", () => {
    const items = buildAnalyticsSteps([
      stepStart("Attempt 0"),
      recoveryResumed(),
      stepStart("Attempt 1"),
      recoveryResumed(),
      stepStart("Attempt 2"),
      stepEnd()
    ]);

    expect(items).toHaveLength(5);
    expect(items[0]).toMatchObject({ label: "Attempt 0", error: "Interrupted by server restart" });
    expect(items[1]).toMatchObject(resumingStep());
    expect(items[2]).toMatchObject({ label: "Attempt 1", error: "Interrupted by server restart" });
    expect(items[3]).toMatchObject(resumingStep());
    expect(items[4]).toMatchObject({ label: "Attempt 2", isStreaming: false });
    expect((items[4] as { error?: string }).error).toBeUndefined();
  });
});

// ── Builder delegation tests ────────────────────────────────────────────────

describe("builder delegation", () => {
  it("delegation_started with agent target creates BuilderDelegationItem", () => {
    seq = 0;
    // Real event order: step opens → awaiting_input (delegation) → step_end(suspended)
    // keeps step open → delegation_started attaches to the open step.
    const items = buildAnalyticsSteps([
      stepStart("Analyzing"),
      ev("awaiting_input", {
        questions: [
          {
            prompt: "Delegating to builder: creating 1 missing semantic member(s)",
            suggestions: []
          }
        ]
      }),
      stepEnd("suspended"),
      ev("delegation_started", {
        child_task_id: "root.1",
        target: "agent:__builder__",
        request: "create missing metric revenue_per_customer"
      })
    ]);

    expect(items).toHaveLength(1);
    const step = items[0];
    expect(step).toMatchObject({ label: "Analyzing" });
    if (!("items" in step)) throw new Error("expected items");
    const delegation = step.items.find((c: { kind: string }) => c.kind === "builder_delegation");
    expect(delegation).toBeDefined();
    expect(delegation).toMatchObject({
      kind: "builder_delegation",
      childRunId: "root.1",
      request: "create missing metric revenue_per_customer",
      status: "running",
      isStreaming: true
    });
  });

  it("delegation_completed updates status to done", () => {
    seq = 0;
    const items = buildAnalyticsSteps([
      stepStart("Analyzing"),
      ev("awaiting_input", {
        questions: [
          {
            prompt: "Delegating to builder: creating 1 missing semantic member(s)",
            suggestions: []
          }
        ]
      }),
      stepEnd("suspended"),
      ev("delegation_started", {
        child_task_id: "root.1",
        target: "agent:__builder__",
        request: "create metric"
      }),
      ev("delegation_completed", {
        child_task_id: "root.1",
        success: true,
        answer: "metric created"
      })
    ]);

    expect(items).toHaveLength(1);
    const step = items[0];
    if (!("items" in step)) throw new Error("expected items");
    const delegation = step.items.find((c: { kind: string }) => c.kind === "builder_delegation");
    expect(delegation).toMatchObject({
      status: "done",
      answer: "metric created",
      isStreaming: false
    });
  });

  it("delegation_completed with failure sets status to failed", () => {
    seq = 0;
    const items = buildAnalyticsSteps([
      stepStart("Analyzing"),
      ev("awaiting_input", {
        questions: [
          {
            prompt: "Delegating to builder: creating 1 missing semantic member(s)",
            suggestions: []
          }
        ]
      }),
      stepEnd("suspended"),
      ev("delegation_started", {
        child_task_id: "root.1",
        target: "agent:__builder__",
        request: "create metric"
      }),
      ev("delegation_completed", {
        child_task_id: "root.1",
        success: false,
        error: "builder failed"
      })
    ]);

    expect(items).toHaveLength(1);
    const step = items[0];
    if (!("items" in step)) throw new Error("expected items");
    const delegation = step.items.find((c: { kind: string }) => c.kind === "builder_delegation");
    expect(delegation).toMatchObject({
      status: "failed",
      error: "builder failed",
      isStreaming: false
    });
  });

  it("automation delegation_started does not create BuilderDelegationItem", () => {
    seq = 0;
    const items = buildAnalyticsSteps([
      stepStart("Running"),
      ev("awaiting_input", {
        questions: [{ prompt: "Execute procedure test.yml", suggestions: [] }]
      }),
      stepEnd("suspended"),
      ev("delegation_started", {
        child_task_id: "root.1",
        target: "workflow:test.procedure.yml",
        request: "execute procedure"
      })
    ]);

    expect(items).toHaveLength(1);
    const step = items[0];
    if (!("items" in step)) {
      return;
    }
    const delegation = step.items.find((c: { kind: string }) => c.kind === "builder_delegation");
    expect(delegation).toBeUndefined();
  });
});
