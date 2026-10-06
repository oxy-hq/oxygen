/**
 * The `User-Agent` every request to a deployment carries:
 *
 *     oxyc/<version>[ agent/<label>][ mcp]
 *
 * The server records it on audit rows and in each token's usage, and it is the
 * only thing there that tells an AGENT driving `oxyc` from the engineer typing
 * the same commands with the same credential. So it says three things: which
 * `oxyc`, whether an agent is at the keyboard and which one, and whether the
 * call came through `oxyc mcp` rather than a command line.
 *
 * NOT A SECURITY BOUNDARY. A caller can set any user agent it likes; this is
 * for telling honest callers apart in a log, which is the common need.
 *
 * Pure, and reads the environment per call, so a test can vary it and so the
 * request layer can import it without pulling anything else in.
 */

import { VERSION } from "../generated/version.js";

/** Where the label comes from when the caller says who it is. */
export const AGENT_ENV = "OXY_AGENT";

const LABEL_MAX_CHARS = 32;

/**
 * Harnesses recognised by a variable they set for every process they start.
 * ONE LINE TO ADD ANOTHER. `OXY_AGENT` wins over all of them.
 *
 * `CLAUDECODE` is what Claude Code itself exports (`CLAUDECODE=1`), checked
 * against a live session — not `CLAUDE_CODE`, and not anything a user sets.
 */
const HARNESSES: ReadonlyArray<{ env: string; label: string }> = [
  { env: "CLAUDECODE", label: "claude-code" }
];

/**
 * A label as it may appear in the header: lowercase letters, digits, `.`, `_`
 * and `-`, at most 32 characters. Anything else becomes a single `-`, so
 * `Claude Code/2.1` reads `claude-code-2.1` rather than vanishing; a value with
 * nothing usable in it is no label at all.
 */
export function sanitizeAgentLabel(raw: string): string | undefined {
  const label = raw
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, LABEL_MAX_CHARS)
    .replace(/-+$/, "");
  return label || undefined;
}

/** Who is driving, when it is an agent: `OXY_AGENT`, else a recognised harness. */
export function agentLabel(env: NodeJS.ProcessEnv = process.env): string | undefined {
  const named = sanitizeAgentLabel(env[AGENT_ENV] ?? "");
  if (named) return named;
  return HARNESSES.find((harness) => env[harness.env]?.trim())?.label;
}

/** Set once by `oxyc mcp`: a process is an MCP server for its whole life, or never. */
let viaMcp = false;

/** Mark every later request as made through `oxyc mcp`. */
export function markMcpSession(on = true): void {
  viaMcp = on;
}

export function userAgent(env: NodeJS.ProcessEnv = process.env): string {
  const parts = [`oxyc/${VERSION}`];
  const label = agentLabel(env);
  if (label) parts.push(`agent/${label}`);
  if (viaMcp) parts.push("mcp");
  return parts.join(" ");
}

/**
 * `headers`, with the user agent added unless the caller set one. For the
 * requests that build their own `fetch` rather than going through
 * `api/request.ts`.
 */
export function withUserAgent(headers: Record<string, string> = {}): Record<string, string> {
  if (Object.keys(headers).some((name) => name.toLowerCase() === "user-agent")) return headers;
  return { ...headers, "user-agent": userAgent() };
}
