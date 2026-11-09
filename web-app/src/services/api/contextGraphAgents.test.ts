// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AgentService } from "./agents";
import { ContextGraphService } from "./contextGraph";
import { agentRefNames, matchesContextGlob } from "./contextGraphAgents";
import { DatabaseService } from "./database";
import { FileService } from "./files";

// The graph used to find agents by the `.agent.yml` file name, a retired kind,
// so a workspace of `.agentic.yml` agents drew none. Agents now come from the
// agent listing (the compiled agents), and their edges from `context:` globs.

type FileTree = Awaited<ReturnType<typeof FileService.getFileTree>>;

const tree = {
  primary: [
    {
      name: "semantics",
      is_dir: true,
      children: [{ name: "orders.view.yml", is_dir: false }]
    },
    { name: "sales.agentic.yml", is_dir: false },
    { name: "legacy.agent.yml", is_dir: false }
  ]
} as unknown as FileTree;

const FILES: Record<string, string> = {
  "semantics/orders.view.yml": "name: orders\ntable: orders\n",
  "sales.agentic.yml": "description: Sales\ncontext:\n  - ./semantics/**/*.view.yml\n",
  "legacy.agent.yml": "model: gpt\n"
};

beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(DatabaseService, "listDatabases").mockResolvedValue([]);
  vi.spyOn(FileService, "getFileTree").mockResolvedValue(tree);
  vi.spyOn(FileService, "getFile").mockImplementation(async (_ws, encoded) => {
    const path = atob(encoded);
    return FILES[path] ?? "";
  });
  vi.spyOn(AgentService, "listAgents").mockResolvedValue([
    { name: "sales", path: "sales.agentic.yml", public: true, model: "openai" }
  ]);
});

describe("Context Graph agents", () => {
  it("draws the listed agentic agents, not classic files, linked by context", async () => {
    const graph = await ContextGraphService.getContextGraph("ws", "main");

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
