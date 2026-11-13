// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ContextGraphService } from "./contextGraph";
import { agentRefNames, matchesContextGlob } from "./contextGraphAgents";
import { DatabaseService } from "./database";

// The graph used to find agents by the `.agent.yml` file name, a retired kind,
// so a workspace of `.agentic.yml` agents drew none. Agents now come from the
// served revision's compiled agents, and their edges from `context:` globs.

beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(DatabaseService, "listDatabases").mockResolvedValue([]);
  vi.spyOn(ContextGraphService, "fetchSources").mockResolvedValue({
    views: [
      {
        name: "orders",
        path: "semantics/orders.view.yml",
        definition: { name: "orders", table: "orders" }
      }
    ],
    topics: [],
    automations: [],
    agents: [
      {
        name: "sales",
        path: "sales.agentic.yml",
        definition: {
          description: "Sales",
          llm: { ref: "openai" },
          context: ["./semantics/**/*.view.yml"]
        }
      }
    ],
    sql_queries: []
  });
});

describe("Context Graph agents", () => {
  it("draws the compiled agentic agents, linked by context", async () => {
    const graph = await ContextGraphService.getContextGraph("ws");

    const agents = graph.nodes.filter((n) => n.type === "agent");
    expect(agents.map((n) => n.data.path)).toEqual(["sales.agentic.yml"]);
    expect(agents[0].data.description).toBe("Sales");
    expect(graph.edges.map((e) => e.id)).toContain(
      "agent:sales.agentic.yml->view:semantics/orders.view.yml"
    );
  });

  it("matches context globs the way the runtime resolves them", () => {
    expect(matchesContextGlob("semantics/a/b.view.yml", "./semantics/**/*.view.yml")).toBe(true);
    expect(matchesContextGlob("semantics/b.view.yml", "semantics/**/*.view.yml")).toBe(true);
    expect(matchesContextGlob("semantics/a/b.view.yml", "semantics/*.view.yml")).toBe(false);
    expect(matchesContextGlob("queries/x.sql", "queries/x.sql")).toBe(true);
  });

  it("resolves an agent_ref by path, bare path or name", () => {
    const agent = { name: "revenue", path: "agents/rev.agentic.yml", context: [] };
    expect(agentRefNames("agents/rev.agentic.yml", agent)).toBe(true);
    expect(agentRefNames("./agents/rev", agent)).toBe(true);
    expect(agentRefNames("revenue", agent)).toBe(true);
    expect(agentRefNames("agents/rev.agent.yml", agent)).toBe(false);
  });
});
