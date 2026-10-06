/**
 * What `oxyc mcp` serves when its credential is a sandbox agent token
 * (`oxy_sbx_…`): the sandbox loop, `oxy_whoami`, and four tools that exist
 * only here.
 *
 * A SMALLER LIST, NOT A FILTERED ONE. The generic tools — `oxy_request`,
 * `oxy_routes`, `oxy_schema` — and the workspace-preview tools are dropped:
 * the server answers this token `404` on everything they reach, and a tool
 * that can only fail still costs its schema on every turn. What replaces
 * `oxy_request` is named: a sandbox's secret is set with `oxy_env_secret_set`,
 * which can carry the one rule a generic request cannot — a `dev-<handle>`
 * environment, and nothing else.
 *
 * Tool DEFINITIONS and the startup decision live here. The calls themselves
 * are cases in `mcp.ts`'s `callTool`, beside the verbs they share code with.
 */

import { describeSandboxToken } from "../apps/sandbox-token.js";
import { isSandboxAgentToken } from "../auth/token-kind.js";
import { type Context, envOnly, notAuthenticated } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { CliError, ExitCode } from "../util/errors.js";

export interface ToolDef {
  name: string;
  description: string;
  inputSchema: {
    type: string;
    properties: Record<string, unknown>;
    required?: string[];
  };
}

/** Which list this session serves. Decided once, at startup, from the credential. */
export type ToolSet = "default" | "sandbox_agent";

const APP = { type: "string", description: '"<org-slug>/<app-slug>" or the app UUID.' };
const OWN_SANDBOX = {
  type: "string",
  description: "dev-<handle> — a sandbox this token created. Production and staging are refused."
};

/** The sandbox tools whose `appEnv` is optional for staff and required here. */
const NEEDS_SANDBOX = new Set([
  "oxy_fn_call",
  "oxy_checks_run",
  "oxy_invocations_list",
  "oxy_logs"
]);

const WHOAMI: ToolDef = {
  name: "oxy_whoami",
  description:
    "What this sandbox agent token is: its kind, when it expires, the apps it reaches and who " +
    "minted it. Call it FIRST — confirm the app you were asked to work on is among `apps` and " +
    "that `expires_at` is later than the task will take; otherwise stop and ask your operator " +
    "for a new token. Call it again when a tool fails with [exit 4 AUTH].",
  inputSchema: { type: "object", properties: {} }
};

/** The four tools only a sandbox agent token is served. */
export const SANDBOX_AGENT_TOOLS: ToolDef[] = [
  {
    name: "oxy_env_secret_list",
    description:
      "List a sandbox's secret KEYS and flags — is_set, required, inherits_staging — never a " +
      "value. `missing_required` above zero means the build declares a secret nothing provides. " +
      "A sandbox reads its own value first, then staging's.",
    inputSchema: {
      type: "object",
      properties: { app: APP, appEnv: OWN_SANDBOX },
      required: ["app", "appEnv"]
    }
  },
  {
    name: "oxy_env_secret_set",
    description:
      "Set one secret in a sandbox — a third party's SANDBOX key, say, so a function stops " +
      "reading staging's value. Accepts only a dev-<handle> appEnv. Write-only: no tool returns " +
      "a value. A value shaped like an Oxy credential is refused by the server.",
    inputSchema: {
      type: "object",
      properties: {
        app: APP,
        appEnv: OWN_SANDBOX,
        key: { type: "string", description: "The secret's name, e.g. STRIPE_SECRET_KEY." },
        value: { type: "string", description: "The value to store." }
      },
      required: ["app", "appEnv", "key", "value"]
    }
  },
  {
    name: "oxy_env_secret_delete",
    description:
      "Delete a sandbox's own value of a secret. Reads of it then fall back to staging's. " +
      "Accepts only a dev-<handle> appEnv.",
    inputSchema: {
      type: "object",
      properties: {
        app: APP,
        appEnv: OWN_SANDBOX,
        key: { type: "string", description: "The secret's name." }
      },
      required: ["app", "appEnv", "key"]
    }
  },
  {
    name: "oxy_token_revoke",
    description:
      "END THIS TOKEN — the last step of a task, after oxy_env_delete. It cannot be undone: " +
      "every later tool call fails with [exit 4 AUTH]. Requires confirm=true.",
    inputSchema: {
      type: "object",
      properties: {
        confirm: { type: "boolean", description: "Must be true, or the call is refused." }
      },
      required: ["confirm"]
    }
  }
];

/** A sandbox tool as this credential must call it: the environment is not optional. */
function forSandboxAgent(tool: ToolDef): ToolDef {
  if (tool.name === "oxy_env_show") {
    return {
      ...tool,
      inputSchema: {
        ...tool.inputSchema,
        properties: { ...tool.inputSchema.properties, name: OWN_SANDBOX }
      }
    };
  }
  if (!NEEDS_SANDBOX.has(tool.name)) return tool;
  const required = tool.inputSchema.required ?? [];
  return {
    ...tool,
    inputSchema: {
      ...tool.inputSchema,
      properties: { ...tool.inputSchema.properties, appEnv: OWN_SANDBOX },
      required: required.includes("appEnv") ? required : [...required, "appEnv"]
    }
  };
}

/** The list for a sandbox agent token, built from the loop's own definitions. */
export function sandboxAgentTools(sandboxLoop: ToolDef[]): ToolDef[] {
  return [WHOAMI, ...sandboxLoop.map(forSandboxAgent), ...SANDBOX_AGENT_TOOLS];
}

/**
 * Decide the tool list, and FAIL CLOSED before serving one.
 *
 * `oxyc mcp` is what an agent runtime starts, with the agent's own token in
 * its environment. Started without `--login`, an unset variable is exit `4`
 * right here: falling back to the login cache would hand the agent its
 * operator's whole reach, silently, for the life of the session. An
 * `OXY_API_KEY` the caller also set is a credential it named, and is let by.
 *
 * A sandbox agent token is then asked what it is (`GET /api/auth/token`).
 * Refused there, it is dead and the session never starts. Unreachable, the
 * prefix still picks the smaller list — the safe direction to be wrong in.
 */
export async function startupToolSet(ctx: Context): Promise<ToolSet> {
  const bearer = ctx.storedBearer();
  if (envOnly(ctx.flags) && !bearer && !ctx.apiKey()) {
    throw notAuthenticated(ctx.target(), ctx.flags);
  }
  if (!isSandboxAgentToken(bearer)) return "default";

  try {
    await describeSandboxToken(ctx.target(), bearer);
  } catch (cause) {
    if (!(cause instanceof CliError) || cause.code === ExitCode.AUTH) throw cause;
    log.warn(`could not confirm the sandbox agent token with ${ctx.target()}: ${cause.message}`);
    log.hint("serving the sandbox tools anyway — each call will report what it meets");
  }
  return "sandbox_agent";
}

/** A tool the session does not serve, called anyway. */
export function notServed(name: string, set: ToolSet): string {
  return set === "sandbox_agent"
    ? `${name} is not served to a sandbox agent token. It does the sandbox loop with the tools listed, and nothing else.`
    : `unknown tool: ${name}`;
}
