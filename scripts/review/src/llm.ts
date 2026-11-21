// One model pass through the Claude Agent SDK: the Claude Code harness, read-only tools,
// a hard dollar cap, and a JSON-schema answer instead of prose.

import type {
  HookCallback,
  McpServerConfig,
  SDKMessage,
  SDKResultMessage
} from "@anthropic-ai/claude-agent-sdk";
import { ALLOWED_TOOLS } from "../config.ts";
import { guardHook } from "./guard.ts";
import { agentEnv } from "./redact.ts";
import type { FirstCallUsage, PassResult, RunPass, TokenUsage } from "./types.ts";

const SYSTEM_APPEND = [
  "You are an automated pull-request reviewer for this repository, running in CI.",
  "You cannot edit files, post comments or ask questions. You read code and answer through the structured output tool.",
  "The repository's CLAUDE.md files are the house rules: a change that breaks one is a finding."
].join("\n");

const GRAPH_SERVER: Record<string, McpServerConfig> = {
  "code-review-graph": { type: "stdio", command: "code-review-graph", args: ["serve"] }
};

/** The guard, in the shape the SDK calls: it only ever looks at tool calls. */
function preToolUse(cwd: string): HookCallback {
  const guard = guardHook(cwd);
  return async (input) =>
    input.hook_event_name === "PreToolUse"
      ? guard({ tool_name: input.tool_name, tool_input: input.tool_input })
      : {};
}

/** Tokens across every model the pass called, from the SDK's per-model totals. */
export function usageOf(
  result: Pick<SDKResultMessage, "modelUsage"> | null
): TokenUsage | undefined {
  const models = Object.values(result?.modelUsage ?? {});
  if (!models.length) return undefined;
  const sum = (pick: (m: (typeof models)[number]) => number) =>
    models.reduce<number>((n, m) => n + (pick(m) || 0), 0);
  return {
    input: sum((m) => m.inputTokens),
    cacheWrite: sum((m) => m.cacheCreationInputTokens),
    cacheRead: sum((m) => m.cacheReadInputTokens),
    output: sum((m) => m.outputTokens)
  };
}

interface Consumed {
  result: SDKResultMessage | null;
  tools: Record<string, number>;
  first?: FirstCallUsage;
}

/**
 * Read a pass's message stream: tool counts, the first call's input usage, the result.
 * `onFirst` fires on the first sign of a response, a stream event when partial messages
 * are on, else the first assistant message.
 */
export async function consume(
  stream: AsyncIterable<SDKMessage>,
  onFirst: (() => void) | undefined
): Promise<Consumed> {
  const out: Consumed = { result: null, tools: {} };
  let fired = false;
  const fire = () => {
    if (fired) return;
    fired = true;
    onFirst?.();
  };
  for await (const m of stream) {
    if (m.type === "stream_event") fire();
    else if (m.type === "assistant") {
      fire();
      if (!out.first && m.parent_tool_use_id === null) {
        const u = m.message.usage;
        out.first = {
          input: u.input_tokens ?? 0,
          cacheWrite: u.cache_creation_input_tokens ?? 0,
          cacheRead: u.cache_read_input_tokens ?? 0
        };
      }
      for (const b of m.message.content)
        if (b.type === "tool_use") out.tools[b.name] = (out.tools[b.name] ?? 0) + 1;
    } else if (m.type === "result") {
      out.result = m;
    }
  }
  return out;
}

function errorsOf(result: SDKResultMessage | null, crash: string | null): string {
  if (crash) return crash;
  return result && result.subtype !== "success" ? result.errors.join("; ").slice(0, 500) : "";
}

/**
 * Run one pass. Never throws for a model-side failure: the result says what happened,
 * so the caller can review with what finished and name what did not.
 */
export const runPass: RunPass = async <T>({
  name,
  prompt,
  schema,
  model,
  effort,
  budgetUsd,
  taskTokens,
  cwd,
  graph,
  onFirstResponse
}: Parameters<RunPass>[0]): Promise<PassResult<T>> => {
  const { query } = await import("@anthropic-ai/claude-agent-sdk");
  const started = Date.now();
  let consumed: Consumed = { result: null, tools: {} };
  let crash: string | null = null;
  try {
    const stream = query({
      prompt,
      options: {
        model,
        effort,
        cwd,
        maxBudgetUsd: budgetUsd,
        ...(taskTokens ? { taskBudget: { total: taskTokens } } : {}),
        // Only the pass others wait on streams partial messages: it is how they learn,
        // within a second or two, that its prefix is cached.
        ...(onFirstResponse ? { includePartialMessages: true } : {}),
        outputFormat: { type: "json_schema", schema },
        systemPrompt: { type: "preset", preset: "claude_code", append: SYSTEM_APPEND },
        // `project` loads the CLAUDE.md files (root, and nested ones as files are read).
        settingSources: ["project"],
        tools: ["Read", "Grep", "Glob", "Bash"],
        allowedTools: graph
          ? [...ALLOWED_TOOLS]
          : ALLOWED_TOOLS.filter((t) => !t.startsWith("mcp__")),
        permissionMode: "dontAsk",
        strictMcpConfig: true,
        mcpServers: graph ? GRAPH_SERVER : {},
        persistSession: false,
        // No GitHub token or other secret (redact.ts), and no path outside the checkout
        // (guard.ts).
        env: { ...agentEnv(process.env), CLAUDE_CODE_DISABLE_BACKGROUND_TASKS: "1" },
        hooks: { PreToolUse: [{ hooks: [preToolUse(cwd)] }] }
      }
    });
    consumed = await consume(stream, onFirstResponse);
  } catch (err) {
    crash = (err instanceof Error ? err.message : String(err)).slice(0, 500);
  }
  const { result, tools, first } = consumed;
  // The SDK validated it against `schema`; that is the contract the caller's T names.
  const output =
    result?.subtype === "success" ? ((result.structured_output as T | undefined) ?? null) : null;
  const usage = usageOf(result);
  return {
    name,
    ok: output !== null,
    // The CLI can exit non-zero after it has reported (a budget stop does): the result wins.
    subtype: result?.subtype ?? (crash ? "crashed" : "no_result"),
    output,
    cost: result?.total_cost_usd ?? 0,
    ms: Date.now() - started,
    turns: result?.num_turns ?? 0,
    tools,
    error: errorsOf(result, crash),
    ...(usage ? { usage } : {}),
    ...(first ? { first } : {})
  };
};
