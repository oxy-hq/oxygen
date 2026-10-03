import { describe, expect, it } from "vitest";
import { buildAnalyticsNodes, groupNodesByStep } from "./analyticsNodes";
import type { SseEvent } from "./useAnalyticsRun";

// ── helpers ───────────────────────────────────────────────────────────────────

// `data` is checked against the wire payload of `type`, so a fixture that drifts
// from the event shape fails the typecheck. The cast only rejoins the pair into
// the union, which TypeScript cannot correlate through a generic.
const ev = <T extends SseEvent["type"]>(
  type: T,
  data: Extract<SseEvent, { type: T }>["data"]
): SseEvent => ({ id: type, type, data }) as SseEvent;

// ── tests ─────────────────────────────────────────────────────────────────────

describe("buildAnalyticsNodes", () => {
  it("returns empty array for no events", () => {
    expect(buildAnalyticsNodes([])).toEqual([]);
  });

  it("step_start only → running step node", () => {
    const nodes = buildAnalyticsNodes([ev("step_start", { label: "Analyzing" })]);
    expect(nodes).toHaveLength(1);
    expect(nodes[0]).toMatchObject({ kind: "step", status: "running", label: "Analyzing" });
  });

  it("step_start + step_end success → done step node", () => {
    const nodes = buildAnalyticsNodes([
      ev("step_start", { label: "Planning" }),
      ev("step_end", { label: "Planning", outcome: "advanced" })
    ]);
    expect(nodes).toHaveLength(1);
    expect(nodes[0]).toMatchObject({ kind: "step", status: "done", label: "Planning" });
  });

  it("step_start + step_end failed → failed step node", () => {
    const nodes = buildAnalyticsNodes([
      ev("step_start", { label: "Running" }),
      ev("step_end", { label: "Running", outcome: "failed" })
    ]);
    expect(nodes).toHaveLength(1);
    expect(nodes[0]).toMatchObject({ kind: "step", status: "failed" });
  });

  it("query_executed success → done domain node with data", () => {
    const nodes = buildAnalyticsNodes([
      ev("query_executed", {
        query: "SELECT 1",
        success: true,
        columns: ["id"],
        rows: [["1"]],
        duration_ms: 42,
        row_count: 1
      })
    ]);
    expect(nodes).toHaveLength(1);
    expect(nodes[0]).toMatchObject({
      kind: "domain",
      status: "done",
      label: "Query Executed",
      data: { query: "SELECT 1", columns: ["id"] }
    });
  });

  it("query_executed failure → failed domain node", () => {
    const nodes = buildAnalyticsNodes([
      ev("query_executed", {
        query: "SELECT bad",
        success: false,
        error: "syntax error",
        columns: [],
        rows: [],
        duration_ms: 5,
        row_count: 0
      })
    ]);
    expect(nodes).toHaveLength(1);
    expect(nodes[0]).toMatchObject({ kind: "domain", status: "failed", label: "Query Failed" });
  });

  it("unknown event type → skipped", () => {
    // Deliberately not a member of SseEvent: the SSE boundary casts whatever event
    // name the server sends, so a newer backend can deliver a type this build has
    // never heard of. The reducer must drop it rather than throw.
    const unknownEvent = {
      id: "llm_token",
      type: "llm_token",
      data: { token: "hello" }
    } as unknown as SseEvent;
    expect(buildAnalyticsNodes([unknownEvent])).toEqual([]);
  });

  it("streaming events that have no node → skipped", () => {
    const nodes = buildAnalyticsNodes([
      ev("thinking_start", {}),
      ev("thinking_token", { token: "hmm" }),
      ev("thinking_end", {}),
      ev("text_delta", { token: "hello" })
    ]);
    expect(nodes).toEqual([]);
  });

  it("interleaved: step + domain events in order", () => {
    const nodes = buildAnalyticsNodes([
      ev("step_start", { label: "Running" }),
      ev("query_generated", { sql: "SELECT 1" }),
      ev("query_executed", {
        query: "SELECT 1",
        success: true,
        columns: [],
        rows: [],
        duration_ms: 10,
        row_count: 0
      }),
      ev("step_end", { label: "Running", outcome: "advanced" })
    ]);
    expect(nodes).toHaveLength(3);
    expect(nodes[0]).toMatchObject({ kind: "step", status: "done", label: "Running" });
    expect(nodes[1]).toMatchObject({ kind: "domain", label: "SQL Generated" });
    expect(nodes[2]).toMatchObject({ kind: "domain", label: "Query Executed" });
  });

  it("schema_resolved → domain node with tables", () => {
    const nodes = buildAnalyticsNodes([
      ev("schema_resolved", { tables: ["orders", "users"], duration_ms: 12 })
    ]);
    expect(nodes).toHaveLength(1);
    expect(nodes[0]).toMatchObject({
      kind: "domain",
      status: "done",
      label: "Schema Resolved",
      data: { tables: ["orders", "users"] }
    });
  });

  it("triage_completed → domain node with summary + question_type + confidence", () => {
    const nodes = buildAnalyticsNodes([
      ev("triage_completed", {
        summary: "Revenue breakdown",
        question_type: "Breakdown",
        confidence: 0.9,
        relevant_tables: ["orders"],
        ambiguities: []
      })
    ]);
    expect(nodes[0]).toMatchObject({
      label: "Triage",
      data: { summary: "Revenue breakdown", question_type: "Breakdown", confidence: 0.9 }
    });
  });

  it("spec_resolved → domain node with result_shape + assumptions", () => {
    const nodes = buildAnalyticsNodes([
      ev("spec_resolved", {
        resolved_metrics: ["SUM(revenue)"],
        resolved_tables: ["orders"],
        join_path: [],
        result_shape: "Table[region, revenue]",
        assumptions: ["revenue = net"],
        solution_source: "Llm"
      })
    ]);
    expect(nodes[0]).toMatchObject({
      label: "Spec Resolved",
      data: { result_shape: "Table[region, revenue]", assumptions: ["revenue = net"] }
    });
  });

  it("fan_out → running step node with count label", () => {
    const nodes = buildAnalyticsNodes([ev("step_start", { label: "Running 3 queries" })]);
    expect(nodes[0]).toMatchObject({ kind: "step", status: "running", label: "Running 3 queries" });
  });

  it("multiple steps in sequence have independent status", () => {
    const nodes = buildAnalyticsNodes([
      ev("step_start", { label: "Analyzing" }),
      ev("step_end", { label: "Analyzing", outcome: "advanced" }),
      ev("step_start", { label: "Planning" }),
      ev("step_end", { label: "Planning", outcome: "advanced" }),
      ev("step_start", { label: "Running" })
    ]);
    expect(nodes).toHaveLength(3);
    expect(nodes[0]).toMatchObject({ label: "Analyzing", status: "done" });
    expect(nodes[1]).toMatchObject({ label: "Planning", status: "done" });
    expect(nodes[2]).toMatchObject({ label: "Running", status: "running" });
  });

  it("analytics_validation_failed → failed domain node", () => {
    const nodes = buildAnalyticsNodes([
      ev("analytics_validation_failed", {
        state: "specifying",
        reason: "Invalid spec",
        model_response: "{}"
      })
    ]);
    expect(nodes[0]).toMatchObject({
      kind: "domain",
      status: "failed",
      label: "Validation Failed",
      data: { state: "specifying", reason: "Invalid spec" }
    });
  });

  it("intent_clarified → domain node with metrics + dimensions + filters", () => {
    const nodes = buildAnalyticsNodes([
      ev("intent_clarified", {
        question_type: "Breakdown",
        metrics: ["revenue"],
        dimensions: ["region"],
        filters: ["date > 2024"]
      })
    ]);
    expect(nodes[0]).toMatchObject({
      label: "Intent Clarified",
      data: { metrics: ["revenue"], dimensions: ["region"], filters: ["date > 2024"] }
    });
  });

  it("done/error events are not rendered as nodes", () => {
    const nodes = buildAnalyticsNodes([
      ev("step_start", { label: "Answering" }),
      ev("step_end", { label: "Answering", outcome: "advanced" }),
      ev("done", { duration_ms: 1234 })
    ]);
    expect(nodes).toHaveLength(1);
  });

  it("each node has a unique string id", () => {
    const nodes = buildAnalyticsNodes([
      ev("step_start", { label: "Analyzing" }),
      ev("schema_resolved", { tables: [], duration_ms: 12 }),
      ev("step_end", { label: "Analyzing", outcome: "advanced" })
    ]);
    const ids = nodes.map((n) => n.id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

// ── groupNodesByStep ──────────────────────────────────────────────────────────

describe("groupNodesByStep", () => {
  it("returns empty array for no nodes", () => {
    expect(groupNodesByStep([])).toEqual([]);
  });

  it("only step nodes → groups with empty children", () => {
    const nodes = buildAnalyticsNodes([
      ev("step_start", { label: "Analyzing" }),
      ev("step_end", { label: "Analyzing", outcome: "advanced" }),
      ev("step_start", { label: "Planning" }),
      ev("step_end", { label: "Planning", outcome: "advanced" })
    ]);
    const groups = groupNodesByStep(nodes);
    expect(groups).toHaveLength(2);
    expect(groups[0].step?.label).toBe("Analyzing");
    expect(groups[0].children).toHaveLength(0);
    expect(groups[1].step?.label).toBe("Planning");
    expect(groups[1].children).toHaveLength(0);
  });

  it("domain nodes before any step → orphan group", () => {
    const nodes = buildAnalyticsNodes([
      ev("schema_resolved", { tables: ["orders"], duration_ms: 12 }),
      ev("step_start", { label: "Analyzing" })
    ]);
    const groups = groupNodesByStep(nodes);
    // orphan group has no step — domain node attached to it
    expect(groups).toHaveLength(2);
    expect(groups[0].step).toBeUndefined();
    expect(groups[0].children).toHaveLength(1);
    expect(groups[0].children[0].label).toBe("Schema Resolved");
    expect(groups[1].step?.label).toBe("Analyzing");
  });

  it("domain nodes between steps belong to the preceding step", () => {
    const nodes = buildAnalyticsNodes([
      ev("step_start", { label: "Running" }),
      ev("query_generated", { sql: "SELECT 1" }),
      ev("query_executed", {
        query: "SELECT 1",
        success: true,
        columns: [],
        rows: [],
        duration_ms: 10,
        row_count: 0
      }),
      ev("step_end", { label: "Running", outcome: "advanced" }),
      ev("step_start", { label: "Answering" })
    ]);
    const groups = groupNodesByStep(nodes);
    expect(groups).toHaveLength(2);
    expect(groups[0].step?.label).toBe("Running");
    expect(groups[0].children).toHaveLength(2);
    expect(groups[0].children[0].label).toBe("SQL Generated");
    expect(groups[0].children[1].label).toBe("Query Executed");
    expect(groups[1].step?.label).toBe("Answering");
    expect(groups[1].children).toHaveLength(0);
  });

  it("multiple steps each collect their own domain children", () => {
    const nodes = buildAnalyticsNodes([
      ev("step_start", { label: "Analyzing" }),
      ev("schema_resolved", { tables: [], duration_ms: 12 }),
      ev("triage_completed", {
        summary: "s",
        question_type: "q",
        confidence: 1,
        relevant_tables: [],
        ambiguities: []
      }),
      ev("step_end", { label: "Analyzing", outcome: "advanced" }),
      ev("step_start", { label: "Planning" }),
      ev("spec_resolved", {
        resolved_metrics: [],
        resolved_tables: [],
        join_path: [],
        result_shape: "T",
        assumptions: [],
        solution_source: "Llm"
      }),
      ev("step_end", { label: "Planning", outcome: "advanced" })
    ]);
    const groups = groupNodesByStep(nodes);
    expect(groups).toHaveLength(2);
    expect(groups[0].children).toHaveLength(2);
    expect(groups[1].children).toHaveLength(1);
  });

  // ── Fan-out / multi-spec ──────────────────────────────────────────────────

  it("fan-out: outer step has sub-groups for each sub-spec", () => {
    const nodes = buildAnalyticsNodes([
      ev("step_start", { label: "Running 3 queries" }), // depth 0
      ev("step_start", { label: "Query 1 of 3" }), // depth 1
      ev("spec_resolved", {
        resolved_metrics: [],
        resolved_tables: [],
        join_path: [],
        result_shape: "T",
        assumptions: [],
        solution_source: "Llm"
      }),
      ev("query_executed", {
        query: "SELECT 1",
        success: true,
        columns: [],
        rows: [],
        duration_ms: 10,
        row_count: 1
      }),
      ev("step_end", { label: "Query 1 of 3", outcome: "advanced" }),
      ev("step_start", { label: "Query 2 of 3" }), // depth 1
      ev("spec_resolved", {
        resolved_metrics: [],
        resolved_tables: [],
        join_path: [],
        result_shape: "T",
        assumptions: [],
        solution_source: "Llm"
      }),
      ev("query_executed", {
        query: "SELECT 2",
        success: true,
        columns: [],
        rows: [],
        duration_ms: 15,
        row_count: 2
      }),
      ev("step_end", { label: "Query 2 of 3", outcome: "advanced" }),
      ev("step_end", { label: "Running 3 queries", outcome: "advanced" })
    ]);
    const groups = groupNodesByStep(nodes);
    expect(groups).toHaveLength(1);
    expect(groups[0].step?.label).toBe("Running 3 queries");
    expect(groups[0].step?.status).toBe("done");
    expect(groups[0].subGroups).toHaveLength(2);
    expect(groups[0].subGroups[0].step?.label).toBe("Query 1 of 3");
    expect(groups[0].subGroups[0].children).toHaveLength(2);
    expect(groups[0].subGroups[1].step?.label).toBe("Query 2 of 3");
    expect(groups[0].subGroups[1].children).toHaveLength(2);
  });

  it("fan-out: outer step status is independent of sub-spec statuses", () => {
    const nodes = buildAnalyticsNodes([
      ev("step_start", { label: "Running 2 queries" }),
      ev("step_start", { label: "Query 1 of 2" }),
      ev("query_executed", {
        query: "SELECT 1",
        success: false,
        error: "bad",
        columns: [],
        rows: [],
        duration_ms: 5,
        row_count: 0
      }),
      ev("step_end", { label: "Query 1 of 2", outcome: "failed" }),
      ev("step_start", { label: "Query 2 of 2" }),
      ev("query_executed", {
        query: "SELECT 2",
        success: true,
        columns: [],
        rows: [],
        duration_ms: 5,
        row_count: 0
      }),
      ev("step_end", { label: "Query 2 of 2", outcome: "advanced" }),
      ev("step_end", { label: "Running 2 queries", outcome: "failed" })
    ]);
    const groups = groupNodesByStep(nodes);
    expect(groups[0].step?.status).toBe("failed");
    expect(groups[0].subGroups[0].step?.status).toBe("failed");
    expect(groups[0].subGroups[1].step?.status).toBe("done");
  });

  it("fan-out while running: outer step stays running while sub-specs execute", () => {
    const nodes = buildAnalyticsNodes([
      ev("step_start", { label: "Running 3 queries" }),
      ev("step_start", { label: "Query 1 of 3" }),
      ev("query_executed", {
        query: "SELECT 1",
        success: true,
        columns: [],
        rows: [],
        duration_ms: 5,
        row_count: 0
      }),
      ev("step_end", { label: "Query 1 of 3", outcome: "advanced" }),
      ev("step_start", { label: "Query 2 of 3" }) // still running
    ]);
    const groups = groupNodesByStep(nodes);
    expect(groups[0].step?.status).toBe("running"); // outer still open
    expect(groups[0].subGroups[0].step?.status).toBe("done");
    expect(groups[0].subGroups[1].step?.status).toBe("running");
  });

  it("non-fan-out steps have empty subGroups", () => {
    const nodes = buildAnalyticsNodes([
      ev("step_start", { label: "Analyzing" }),
      ev("schema_resolved", { tables: [], duration_ms: 12 }),
      ev("step_end", { label: "Analyzing", outcome: "advanced" })
    ]);
    const groups = groupNodesByStep(nodes);
    expect(groups[0].subGroups).toHaveLength(0);
    expect(groups[0].children).toHaveLength(1);
  });
});
