import { parse } from "yaml";
import { encodeBase64 } from "@/libs/encoding";
import type { ContextGraphEdge, ContextGraphNode } from "@/types/contextGraph";
import { AgentService } from "./agents";
import { FileService } from "./files";

/**
 * An agent as the Context Graph draws it. Which agents exist comes from the
 * workspace's agent listing (`GET /{workspaceId}/agents`), which is the
 * compiled `.agentic.yml` agents for the selected branch — not from a file
 * name pattern. The graph used to find agents by `.agent.yml`, a retired kind,
 * so it drew none.
 */
export type GraphAgent = {
  name: string;
  path: string;
  description?: string;
  model?: string;
  /** The agent's `context:` globs, workspace-root relative. */
  context: string[];
};

const AGENT_EXTENSIONS = [".agentic.yml", ".agentic.yaml"];
const CONTEXT_NODE_TYPES = new Set([
  "view",
  "topic",
  "sql_query",
  "automation",
  "procedure",
  "workflow"
]);

/** The workspace's agents, each with what its file says about its context. */
export async function loadGraphAgents(
  projectId: string,
  branchName: string
): Promise<GraphAgent[]> {
  let listed: Awaited<ReturnType<typeof AgentService.listAgents>>;
  try {
    listed = await AgentService.listAgents(projectId, branchName);
  } catch (error) {
    console.error("Failed to list agents for the context graph:", error);
    return [];
  }
  return Promise.all(
    listed.map(async (agent) => ({
      name: agent.name,
      path: agent.path,
      model: agent.model,
      ...(await readAgentBody(projectId, branchName, agent.path))
    }))
  );
}

/** `description:` and `context:` from the agent's file; nothing if unreadable. */
async function readAgentBody(
  projectId: string,
  branchName: string,
  path: string
): Promise<Pick<GraphAgent, "description" | "context">> {
  try {
    const content = await FileService.getFile(projectId, encodeBase64(path), branchName);
    const parsed = (parse(content) ?? {}) as Record<string, unknown>;
    return {
      description: typeof parsed.description === "string" ? parsed.description : undefined,
      context: Array.isArray(parsed.context)
        ? parsed.context.filter((c): c is string => typeof c === "string")
        : []
    };
  } catch (error) {
    console.error(`Failed to read agent ${path}:`, error);
    return { context: [] };
  }
}

/**
 * Whether a workspace path matches a `context:` glob: `**` crosses
 * directories, `*` and `?` do not. Both sides are root-relative.
 */
export function matchesContextGlob(path: string, pattern: string): boolean {
  const glob = pattern.replace(/^\.\//, "");
  let source = "";
  for (let i = 0; i < glob.length; i++) {
    const ch = glob[i];
    if (ch === "*" && glob[i + 1] === "*") {
      const slash = glob[i + 2] === "/";
      source += slash ? "(?:.*/)?" : ".*";
      i += slash ? 2 : 1;
    } else if (ch === "*") {
      source += "[^/]*";
    } else if (ch === "?") {
      source += "[^/]";
    } else {
      source += ch.replace(/[.+^${}()|[\]\\]/g, "\\$&");
    }
  }
  return new RegExp(`^${source}$`).test(path.replace(/^\.\//, ""));
}

/** The SQL files any agent's `context:` names, so their nodes are drawn. */
export function sqlPathsInAgentContext(
  agents: GraphAgent[],
  sqlQueries: Array<{ path: string }>
): string[] {
  const patterns = agents.flatMap((a) => a.context);
  return sqlQueries
    .filter((q) => patterns.some((p) => matchesContextGlob(q.path, p)))
    .map((q) => q.path);
}

/** Add one node per agent and a `uses` edge to each node its context names. */
export function addAgentNodes(
  agents: GraphAgent[],
  nodes: ContextGraphNode[],
  edges: ContextGraphEdge[]
): void {
  const targets = nodes.filter((n) => CONTEXT_NODE_TYPES.has(n.type) && n.data.path);
  for (const agent of agents) {
    const agentId = `agent:${agent.path}`;
    nodes.push({
      id: agentId,
      type: "agent",
      label: agent.name,
      data: {
        name: agent.name,
        path: agent.path,
        description: agent.description,
        metadata: { model: agent.model }
      }
    });
    for (const target of targets) {
      const path = target.data.path as string;
      if (agent.context.some((pattern) => matchesContextGlob(path, pattern))) {
        edges.push({
          id: `${agentId}->${target.id}`,
          source: agentId,
          target: target.id,
          label: "uses",
          type: "uses"
        });
      }
    }
  }
}

/**
 * Whether an automation step's `agent_ref` names `agent`: by its path, its
 * path without the extension, or its name — the spellings the runtime
 * resolves (`oxy_compile::agent_ref_spellings`).
 */
export function agentRefNames(agentRef: string, agent: GraphAgent): boolean {
  const ref = agentRef.trim().replace(/^\.\//, "");
  if (ref === agent.path || ref === agent.name) return true;
  return AGENT_EXTENSIONS.some((ext) => `${ref}${ext}` === agent.path);
}

/** Link each automation whose steps run an agent to that agent's node. */
export function linkAutomationAgentSteps(
  automations: Array<{ path: string; tasks: unknown[] }>,
  agents: GraphAgent[],
  nodes: ContextGraphNode[],
  edges: ContextGraphEdge[]
): void {
  for (const automation of automations) {
    const node = nodes.find(
      (n) => n.data.path === automation.path && n.type !== "agent" && n.type !== "sql_query"
    );
    if (!node) continue;
    for (const ref of agentRefsIn(automation.tasks)) {
      const agent = agents.find((a) => agentRefNames(ref, a));
      if (!agent) continue;
      const target = `agent:${agent.path}`;
      const id = `${node.id}->${target}`;
      if (!edges.some((e) => e.id === id)) {
        edges.push({ id, source: node.id, target, label: "uses", type: "uses" });
      }
    }
  }
}

function agentRefsIn(tasks: unknown): string[] {
  if (!Array.isArray(tasks)) return [];
  return tasks.flatMap((task) => {
    if (typeof task !== "object" || task === null) return [];
    const t = task as Record<string, unknown>;
    const own = t.type === "agent" && typeof t.agent_ref === "string" ? [t.agent_ref] : [];
    return [...own, ...agentRefsIn(t.tasks)];
  });
}
