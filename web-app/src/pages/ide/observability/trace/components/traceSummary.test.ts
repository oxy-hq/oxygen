import { describe, expect, it } from "vitest";
import type { SpanEvent, TimelineSpan } from "@/services/api/traces";
import { summarizeTrace } from "./traceSummary";

let nextId = 0;

function span(spanName: string, overrides: Partial<TimelineSpan> = {}): TimelineSpan {
  nextId += 1;
  return {
    spanId: `span-${nextId}`,
    parentSpanId: "",
    spanName,
    timestamp: "2026-10-01T00:00:00Z",
    durationMs: 10,
    offsetMs: 0,
    depth: 0,
    statusCode: "Ok",
    spanKind: "INTERNAL",
    attributes: {},
    events: [],
    children: [],
    ...overrides
  };
}

const event = (name: string, attributes: Record<string, string> = {}): SpanEvent => ({
  timestamp: "2026-10-01T00:00:00Z",
  name,
  attributes
});

const usage = (total: string) => event("llm.usage", { total_tokens: total });

describe("summarizeTrace", () => {
  it("is all zeroes for a trace with no spans", () => {
    expect(summarizeTrace([])).toEqual({
      spanCount: 0,
      errorCount: 0,
      llmCallCount: 0,
      toolCallCount: 0,
      totalTokens: 0
    });
  });

  it("counts spans by the category the waterfall colours them", () => {
    const toolCall = { attributes: { "oxy.span_type": "tool_call" } };
    const summary = summarizeTrace([
      span("analytics.run", { attributes: { "oxy.span_type": "analytics" } }),
      span("llm.call"),
      span("llm.call"),
      span("analytics.tool_call", toolCall),
      // The explicit type wins over a name that says nothing.
      span("step", toolCall),
      // Coloured `sql`, so not a tool row — even though a tool call ran it.
      span("sql.execute")
    ]);
    expect(summary.spanCount).toBe(6);
    expect(summary.llmCallCount).toBe(2);
    expect(summary.toolCallCount).toBe(2);
  });

  it("does not count a tool span the waterfall hides", () => {
    // An eventless `tool.execute` is left out of the waterfall. Counting it
    // would put a number in the strip that names a row the reader cannot find.
    const summary = summarizeTrace([
      span("tool.execute"),
      span("tool.execute", { events: [event("tool.result")] })
    ]);
    expect(summary.toolCallCount).toBe(1);
    // Still a span: the header's count is of everything recorded.
    expect(summary.spanCount).toBe(2);
  });

  it("counts errors whatever case the status arrives in", () => {
    const summary = summarizeTrace([
      span("llm.call", { statusCode: "Error" }),
      span("tool.sql", { statusCode: "ERROR", events: [event("tool.sql.result")] }),
      span("agent.run_agent", { statusCode: "Ok" })
    ]);
    expect(summary.errorCount).toBe(2);
  });

  it("sums tokens off `llm.usage` events, on whichever span carries them", () => {
    // The same rule the trace list's total uses, so a card and its detail page
    // show one number.
    const summary = summarizeTrace([
      span("llm.call", { events: [usage("120")] }),
      span("agent.run_agent", { events: [usage("30"), usage("50")] })
    ]);
    expect(summary.totalTokens).toBe(200);
  });

  it("ignores token figures that are not `llm.usage` events", () => {
    const summary = summarizeTrace([
      // Another event that happens to carry the attribute.
      span("llm.call", { events: [event("llm.request", { total_tokens: "999" })] }),
      // The span-level attribute the inspector reads: a second copy of a call's
      // usage, which would double the total if it were added in.
      span("llm.call", {
        attributes: { "gen_ai.usage.input_tokens": "70", "gen_ai.usage.output_tokens": "30" },
        events: [usage("100")]
      }),
      // An unparseable value contributes nothing rather than `NaN`.
      span("llm.call", { events: [usage("n/a"), event("llm.usage")] })
    ]);
    expect(summary.totalTokens).toBe(100);
  });
});
