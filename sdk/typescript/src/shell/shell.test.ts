import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { cx } from "./cx";
import { workspaceLogoUrl } from "./logoUrl";

describe("cx", () => {
  it("joins truthy classes and drops falsy ones", () => {
    expect(cx("a", false, "b", null, undefined, "c")).toBe("a b c");
    expect(cx()).toBe("");
  });
});

describe("workspaceLogoUrl", () => {
  it("builds the logo URL from an api base", () => {
    expect(workspaceLogoUrl("/api", "ws-1")).toBe("/api/ws-1/logo");
  });

  it("tolerates a trailing slash on the base", () => {
    expect(workspaceLogoUrl("http://localhost:3000/api/", "ws-1")).toBe(
      "http://localhost:3000/api/ws-1/logo"
    );
  });

  it("appends an encoded cache-busting version", () => {
    expect(workspaceLogoUrl("/api", "ws-1", "2026-07-06 10:00")).toBe(
      "/api/ws-1/logo?v=2026-07-06%2010%3A00"
    );
  });
});

describe("shell.css token scheme", () => {
  const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "shell.css"), "utf8");

  const varsIn = (block: string): Set<string> => {
    const out = new Set<string>();
    for (const m of block.matchAll(/(--oxy-shell-[a-z-]+):/g)) {
      out.add(m[1]);
    }
    return out;
  };

  // The scope blocks are the first two top-level rules; split on the light
  // scope's closing brace followed by the dark selector.
  const lightStart = css.indexOf(".oxy-shell-scope {");
  const darkStart = css.indexOf(".dark .oxy-shell-scope");
  const lightBlock = css.slice(lightStart, darkStart);
  const darkBlock = css.slice(darkStart, css.indexOf("}", darkStart + 1) + 1);

  it("finds both theme scopes", () => {
    expect(lightStart).toBeGreaterThanOrEqual(0);
    expect(darkStart).toBeGreaterThan(lightStart);
  });

  it("defines every color token in both light and dark scopes", () => {
    const light = varsIn(lightBlock);
    const dark = varsIn(darkBlock);
    // Radius + fonts are theme-invariant; every color token must flip.
    const themeInvariant = new Set([
      "--oxy-shell-radius",
      "--oxy-shell-font",
      "--oxy-shell-font-mono"
    ]);
    const lightColors = [...light].filter((v) => !themeInvariant.has(v));
    for (const v of lightColors) {
      expect(dark, `dark scope is missing ${v}`).toContain(v);
    }
    for (const v of dark) {
      expect(light, `light scope is missing ${v}`).toContain(v);
    }
  });

  it("draws the tooltip on the host's --muted in both scopes", () => {
    // The tooltip's text is --oxy-shell-foreground, i.e. the host's
    // --foreground. Its box must come from the token a host pairs with that
    // text, in both themes: a fixed light box under a dark-only host's
    // near-white --foreground measured 1.08:1.
    const tooltipBg = (block: string) =>
      block.match(/--oxy-shell-tooltip-bg:\s*([^;]+);/)?.[1].trim();
    expect(tooltipBg(lightBlock)).toMatch(/^var\(--muted,/);
    expect(tooltipBg(darkBlock)).toMatch(/^var\(--muted,/);
  });

  it("only uses namespaced selectors", () => {
    // Guard against ANY selector leaking styles into host apps: every rule
    // header (at any nesting level, e.g. inside @media) must be a comma
    // list of .oxy-* selectors, optionally under a .dark ancestor. Element,
    // id, attribute, and universal selectors are all leaks.
    const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "");
    // `[^{}]+\{` can't cross a rule body (the body's `}` breaks the run),
    // so each match is exactly one rule/at-rule header.
    const headers = [...stripped.matchAll(/([^{}]+)\{/g)].map((m) => m[1].trim());
    expect(headers.length).toBeGreaterThan(20); // the parser found real rules

    const KEYFRAME_STEP = /^(from|to|\d+(\.\d+)?%)(\s*,\s*(from|to|\d+(\.\d+)?%))*$/;
    for (const header of headers) {
      if (header.startsWith("@")) continue; // @media/@keyframes wrappers
      if (KEYFRAME_STEP.test(header)) continue; // steps inside @keyframes
      for (const sel of header.split(",").map((sub) => sub.trim())) {
        expect(
          /^(\.dark\s+)?\.oxy-/.test(sel),
          `selector "${sel}" is not namespaced under .oxy-*`
        ).toBe(true);
      }
    }
  });
});

describe("buildTraceSteps", async () => {
  const { buildTraceSteps } = await import("./trace");

  it("folds step lifecycle events into status rows", () => {
    const steps = buildTraceSteps([
      { type: "step_start", data: { label: "Analyzing", summary: "reading the question" } },
      { type: "step_end", data: { outcome: "advanced" } },
      { type: "step_start", data: { label: "Querying" } },
      { type: "step_end", data: { outcome: "failed" } }
    ]);
    expect(steps.map((s) => [s.label, s.status])).toEqual([
      ["Analyzing", "done"],
      ["Querying", "failed"]
    ]);
    expect(steps[0].summary).toBe("reading the question");
  });

  it("keeps the open step running and attaches tools + queries to it", () => {
    const steps = buildTraceSteps([
      { type: "step_start", data: { label: "Querying" } },
      { type: "tool_call", data: { name: "execute_sql" } },
      { type: "tool_result", data: { duration_ms: 1400 } },
      { type: "query_executed", data: { success: true, row_count: 22, duration_ms: 900 } }
    ]);
    expect(steps).toHaveLength(1);
    expect(steps[0].status).toBe("running");
    const [tool, sql] = steps[0].items;
    expect(tool).toMatchObject({
      kind: "tool",
      label: "Execute Sql",
      running: false,
      detail: "1.4s"
    });
    expect(sql).toMatchObject({ kind: "sql", label: "Query", detail: "22 rows · 900ms" });
  });

  it("marks failed queries and synthesizes a step when none is open", () => {
    const steps = buildTraceSteps([
      { type: "query_executed", data: { success: false, error: "relation missing" } }
    ]);
    expect(steps).toHaveLength(1);
    expect(steps[0].label).toBe("Working");
    expect(steps[0].items[0]).toMatchObject({ error: true, detail: "relation missing" });
  });

  it("closes dangling steps on terminal and recovery events", () => {
    const steps = buildTraceSteps([
      { type: "step_start", data: { label: "Analyzing" } },
      { type: "recovery_resumed", data: { message: "resuming" } },
      { type: "step_start", data: { label: "Querying" } },
      { type: "error", data: {} }
    ]);
    expect(steps.map((s) => [s.label, s.status])).toEqual([
      ["Analyzing", "done"],
      ["Resuming", "done"],
      ["Querying", "failed"]
    ]);
  });

  it("ignores unknown event types", () => {
    expect(buildTraceSteps([{ type: "text_delta", data: { token: "hi" } }])).toEqual([]);
  });

  it("settles a still-running item when the run ends without its result", () => {
    // A tool_call with no matching tool_result must not spin forever once a
    // terminal event closes the step.
    const steps = buildTraceSteps([
      { type: "step_start", data: { label: "Querying" } },
      { type: "tool_call", data: { name: "execute_sql" } },
      { type: "done", data: {} }
    ]);
    expect(steps[0].status).toBe("done");
    expect(steps[0].items[0].running).toBe(false);
  });

  it("closes the open step as suspended on awaiting_input", () => {
    const steps = buildTraceSteps([
      { type: "step_start", data: { label: "Clarifying" } },
      { type: "tool_call", data: { name: "search_catalog" } },
      { type: "awaiting_input", data: { questions: [{ prompt: "Which region?" }] } }
    ]);
    expect(steps[0].status).toBe("suspended");
    expect(steps[0].items[0].running).toBe(false);
  });
});

describe("extractClarification", async () => {
  const { extractClarification } = await import("./trace");

  it("reads the prompt from an awaiting_input questions array", () => {
    expect(
      extractClarification([
        { type: "awaiting_input", data: { questions: [{ prompt: "Which store?" }] } }
      ])
    ).toBe("Which store?");
  });

  it("falls back to a legacy single question field", () => {
    expect(extractClarification([{ type: "awaiting_input", data: { question: "When?" } }])).toBe(
      "When?"
    );
  });

  it("returns null when there is no suspension event", () => {
    expect(extractClarification([{ type: "text_delta", data: { token: "hi" } }])).toBeNull();
  });
});

describe("trace helpers", async () => {
  const { prettyToolName, aggregateLlmStats, buildTraceSteps } = await import("./trace");

  it("prettifies tool names like the web-app pills", () => {
    expect(prettyToolName("search_catalog")).toBe("Search Catalog");
    expect(prettyToolName("render-chart")).toBe("Render Chart");
  });

  it("aggregates llm_usage events into header meta", () => {
    expect(
      aggregateLlmStats([
        { type: "llm_usage", data: { duration_ms: 1200 } },
        { type: "llm_usage", data: { duration_ms: 800 } },
        { type: "text_delta", data: { token: "x" } }
      ])
    ).toEqual({ calls: 2, totalMs: 2000 });
  });

  it("captures full input/output payloads for expandable rows", () => {
    const steps = buildTraceSteps([
      { type: "tool_call", data: { name: "search_catalog", input: '{"queries":["labor hours"]}' } },
      { type: "tool_result", data: { output: { matches: 3 }, duration_ms: 12 } },
      { type: "query_executed", data: { success: true, query: "SELECT 1", row_count: 5 } }
    ]);
    const [tool, sql] = steps[0].items;
    expect(tool.input).toContain('"queries"');
    expect(tool.input).toContain("labor hours");
    expect(tool.output).toContain('"matches": 3');
    expect(sql.input).toBe("SELECT 1");
  });

  it("unwraps pre-serialized JSON tool inputs in previews", () => {
    const steps = buildTraceSteps([
      {
        type: "tool_call",
        data: { name: "search_automations", input: '{"query":"top stores by revenue"}' }
      }
    ]);
    expect(steps[0].items[0].preview).toBe("top stores by revenue");
  });

  it("captures tool input previews and streamed thinking text", () => {
    const steps = buildTraceSteps([
      { type: "step_start", data: { label: "Clarifying" } },
      {
        type: "tool_call",
        data: { name: "search_catalog", input: { query: "net sales by store" } }
      },
      { type: "thinking_start", data: {} },
      { type: "thinking_token", data: { token: "Rank the " } },
      { type: "thinking_token", data: { token: "top stores" } },
      { type: "thinking_end", data: {} }
    ]);
    const [tool, think] = steps[0].items;
    expect(tool).toMatchObject({ label: "Search Catalog", preview: "net sales by store" });
    expect(think).toMatchObject({ kind: "thinking", text: "Rank the top stores", running: false });
  });
});

describe("humanizeCol", async () => {
  const { humanizeCol } = await import("./AnswerChart");
  it("turns a semantic column id into a friendly label", () => {
    expect(humanizeCol("sales_daily__total_net_sales")).toBe("Total Net Sales");
    expect(humanizeCol("orders")).toBe("Orders");
    expect(humanizeCol(undefined)).toBe("");
  });
});

describe("chart pivoting", async () => {
  const { pivotChart, niceCeil, axisDomain } = await import("./AnswerChart");

  it("pivots single-series rows on the x column", () => {
    const data = pivotChart({
      config: { chart_type: "bar_chart", x: "store", y: "sales" },
      columns: ["store", "sales"],
      rows: [
        ["Almaden", "46764"],
        ["Palo Alto", 41679]
      ]
    });
    expect(data.categories).toEqual(["Almaden", "Palo Alto"]);
    expect(data.series).toEqual([{ name: "sales", values: [46764, 41679] }]);
  });

  it("splits rows into series when a series column is set", () => {
    const data = pivotChart({
      config: { chart_type: "line_chart", x: "month", y: "sales", series: "region" },
      columns: ["month", "region", "sales"],
      rows: [
        ["Jan", "North", 10],
        ["Jan", "South", 20],
        ["Feb", "North", 30]
      ]
    });
    expect(data.categories).toEqual(["Jan", "Feb"]);
    expect(data.series).toEqual([
      { name: "North", values: [10, 30] },
      { name: "South", values: [20, 0] }
    ]);
  });

  it("rounds axis ceilings to nice numbers", () => {
    expect(niceCeil(46764)).toBe(50000);
    expect(niceCeil(82)).toBe(100);
    expect(niceCeil(1.4)).toBe(2);
    expect(niceCeil(0)).toBe(1);
  });

  it("keeps the axis pinned at zero for all-positive data", () => {
    expect(axisDomain([10, 50, 30])).toEqual({ lo: 0, hi: 50 });
  });

  it("extends the axis below zero so negative values stay visible", () => {
    // A -100 minimum must produce a negative floor, not clamp to 0.
    expect(axisDomain([-100, 50])).toEqual({ lo: -100, hi: 50 });
    // All-negative data must not collapse to a 0..1 axis.
    expect(axisDomain([-40, -100]).lo).toBe(-100);
    expect(axisDomain([-40, -100]).hi).toBe(0);
  });

  it("gives all-zero data a unit span so ticks still render", () => {
    expect(axisDomain([0, 0])).toEqual({ lo: 0, hi: 1 });
  });
});
