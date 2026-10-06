/**
 * What `oxyc` does differently when the credential is a sandbox agent token
 * (`oxy_sbx_…`): where an app is resolved, which mount read-back uses, what is
 * refused before any request, and what a refusal from the server means.
 *
 * The token does the sandbox loop on the apps it was minted for, in sandboxes
 * it created itself, and nothing else (`internal-docs/custom-app-sandboxes.md`
 * §1.2). THE SERVER ENFORCES ALL OF IT. The checks here save a round trip and
 * say what to do instead, which a bare `404` cannot.
 */

import { type ApiResponse, errorForResponse, parseJson, request } from "../api/request.js";
import { normalizeToken, type SandboxTokenApp, type Token } from "../auth/token-api.js";
import { isSandboxAgentToken } from "../auth/token-kind.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { printable, printableLines } from "../util/printable.js";

/**
 * Where a sandbox agent token reads invocations and held writes. The admin
 * mount (`/api/admin/apps`) answers this credential `404` on every path.
 */
export const SANDBOX_READ_BACK_SURFACE = "/api/customer-apps";

/** An app id, as opposed to an `<org-slug>/<app-slug>` pair. */
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** What every dead-token and refused-token error tells an agent to do. */
export const STOP_AND_REPORT =
  "stop and report this to your operator — do not look for another credential, a cached `oxyc login` included";

const DEAD_TOKEN_DETAIL =
  "a sandbox agent token stops working when it expires, is revoked, or the person who minted it loses access to the app.";

export interface SandboxTokenInfo {
  /** `GET /api/auth/token`'s body, as the server sent it. */
  body: string;
  token: Token;
  apps: SandboxTokenApp[];
}

export interface SandboxResolvedApp {
  appId: string;
  label: string;
  orgSlug: string;
  appSlug: string;
}

/**
 * One description per token per process. A token's apps are fixed at mint
 * (the server answers `409 sandbox_token_fixed` to an edit), so `oxyc mcp`
 * asks once rather than before every tool call. Failures are never kept.
 */
const described = new Map<string, SandboxTokenInfo>();

/** Drop what is remembered: after a revoke, and between tests. */
export function forgetSandboxTokens(): void {
  described.clear();
}

/** The error for a sandbox agent token the deployment no longer accepts. */
export function deadSandboxToken(target: string, response?: ApiResponse): CliError {
  return new CliError(`the sandbox agent token is no longer accepted by ${target}`, {
    code: ExitCode.AUTH,
    detail: [DEAD_TOKEN_DETAIL, printableLines(response?.body ?? "").trim()]
      .filter(Boolean)
      .join("\n"),
    hint: STOP_AND_REPORT
  });
}

/**
 * What the token is and which apps it reaches: `GET /api/auth/token`.
 *
 * THROWS, unlike `introspectToken`: every caller here cannot go on without
 * the answer, and "the token is dead" must exit `4`, not read as "no apps".
 */
export async function describeSandboxToken(
  target: string,
  bearer: string,
  // `fresh` for a caller asking "does it still work" — `whoami` — where a
  // remembered answer is the one wrong answer.
  opts: { fresh?: boolean } = {}
): Promise<SandboxTokenInfo> {
  const key = `${target}\n${bearer}`;
  const known = opts.fresh ? undefined : described.get(key);
  if (known) return known;

  const response = await request({
    target,
    path: "/api/auth/token",
    method: "GET",
    bearer,
    timeoutMs: 30_000
  });
  if (response.status === 401 || response.status === 403) {
    throw deadSandboxToken(target, response);
  }
  if (response.status < 200 || response.status >= 300) throw sandboxTokenError(response);

  const raw = parseJson(response.body) as Partial<Token> | undefined;
  if (!raw || typeof raw.id !== "string" || raw.kind !== "sandbox_agent") {
    throw new CliError("GET /api/auth/token did not describe a sandbox agent token", {
      code: ExitCode.UNAVAILABLE,
      detail: response.body.trim().slice(0, 2000) || undefined,
      hint: "the deployment may predate sandbox agent tokens"
    });
  }
  const token = normalizeToken(raw);
  const info: SandboxTokenInfo = { body: response.body, token, apps: token.apps ?? [] };
  described.set(key, info);
  return info;
}

function label(app: SandboxTokenApp): string {
  return `${app.org_slug}/${app.slug}`;
}

/**
 * Resolve `<org-slug>/<app-slug>` or an app UUID against the token's OWN app
 * list. Never `GET /api/admin/apps`, which this credential cannot reach.
 */
export async function resolveSandboxApp(
  target: string,
  bearer: string,
  app: string
): Promise<SandboxResolvedApp> {
  const isId = UUID_RE.test(app);
  if (!isId) {
    const [orgSlug, ...rest] = app.split("/");
    if (!orgSlug || !rest.join("/")) {
      throw usageError(
        `"${app}" is not a valid app`,
        '<app> is "<org-slug>/<app-slug>" or an app UUID'
      );
    }
  }

  const { apps } = await describeSandboxToken(target, bearer);
  const wanted = app.toLowerCase();
  const match = apps.find((a) =>
    isId ? a.id.toLowerCase() === wanted : label(a).toLowerCase() === wanted
  );
  if (!match) {
    const reach = apps.length > 0 ? apps.map(label).join(", ") : "no app";
    throw new CliError(`this sandbox agent token was not minted for "${app}"`, {
      code: ExitCode.NOT_FOUND,
      detail: `it reaches: ${reach}`,
      hint: `a token's apps are fixed when it is minted — ${STOP_AND_REPORT}`
    });
  }
  return { appId: match.id, label: label(match), orgSlug: match.org_slug, appSlug: match.slug };
}

/**
 * Refuse, before any request, a command that names no sandbox or names an
 * environment this token can never reach. Returns the sandbox name.
 *
 * `appEnv` is already through `parseAppEnv`, so it is `production`, `staging`
 * or a well-formed `dev-<handle>`.
 */
export function requireOwnSandbox(command: string, appEnv: string | undefined): string {
  if (appEnv === undefined) {
    throw usageError(
      `${command} needs --app-env dev-<handle> with a sandbox agent token`,
      "the token reaches only the sandboxes it created — with no --app-env this is production, which it is refused"
    );
  }
  if (!appEnv.startsWith("dev-")) {
    throw usageError(
      `a sandbox agent token cannot reach ${appEnv}`,
      "pass --app-env dev-<handle>, naming a sandbox this token created"
    );
  }
  return appEnv;
}

/** Refuse a whole command to a sandbox agent token, before any request. */
export function refuseSandboxToken(bearer: string | undefined, what: string, hint: string): void {
  if (!isSandboxAgentToken(bearer)) return;
  throw usageError(`a sandbox agent token cannot ${what}`, hint);
}

/** What a refused response says, in whichever shape its route answers. */
export interface Refusal {
  /** The server's own code, e.g. `token_sandbox_limit`. */
  code?: string;
  /** Its sentence, on one line, safe to print. */
  reason?: string;
}

/** A server error code: snake_case, nothing a terminal or a tool result could misread. */
const CODE_RE = /^[a-z][a-z0-9_]{1,63}$/;
/** `credential_shaped_value: <sentence>` — a code leading a plain-text body. */
const CODE_PREFIX_RE = /^([a-z][a-z0-9]*(?:_[a-z0-9]+)+):\s+(\S[\s\S]*)$/;
/** The most of a server's sentence that goes on the error line. */
const REASON_MAX_CHARS = 300;

/** `text` on one line, without control characters, cut to a length a line can carry. */
function oneLine(text: string): string | undefined {
  const line = printable(text.replace(/\s+/g, " ")).trim();
  if (!line) return undefined;
  return line.length > REASON_MAX_CHARS ? `${line.slice(0, REASON_MAX_CHARS - 1)}…` : line;
}

function codeOf(value: unknown): string | undefined {
  return typeof value === "string" && CODE_RE.test(value) ? value : undefined;
}

/**
 * Read a refused response's body. THE ROUTES DO NOT AGREE ON A SHAPE:
 *
 *   `{"code", "error": "<sentence>"}`             the token routes
 *   `{"code", "error": "<code>", "message"}`      a refused publish (403)
 *   `{"error": "<code>", "message"}`              sandboxes, checks, read-back, logs
 *   `{"error": "<sentence>"}`                     logs, for a caller it does not admit
 *   `<sentence>`                                  secrets, and every other publish refusal
 *   `<code>: <sentence>`                          a secret value shaped like a credential
 *   nothing                                       `/fn`, and any route outside the token's reach
 *
 * Never throws and never guesses: a body that is none of these says nothing,
 * and the caller falls back to the status line.
 */
export function readRefusal(body: string): Refusal {
  const text = body.trim();
  if (!text) return {};
  const parsed = parseJson(text);
  if (parsed !== undefined) {
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const { code, error, message } = parsed as Record<string, unknown>;
    // `error` holds the code on the sandbox routes and a sentence elsewhere,
    // so it is a code only when it is shaped like one.
    const named = codeOf(code) ?? codeOf(error);
    const said = [message, error].find(
      (value): value is string => typeof value === "string" && value !== named
    );
    return { code: named, reason: said === undefined ? undefined : oneLine(said) };
  }
  // A proxy's error page is not the deployment's answer.
  if (text.startsWith("<")) return {};
  const prefixed = CODE_PREFIX_RE.exec(text);
  if (prefixed) return { code: prefixed[1], reason: oneLine(prefixed[2] ?? "") };
  return { reason: oneLine(text) };
}

function hintFor(status: number, code: string | undefined): string | undefined {
  if (code === "token_sandbox_limit") {
    return "this token holds 3 sandboxes, counting any still being deleted — delete one of your own and wait for its teardown (`oxyc env delete <app> dev-<handle> --yes --wait`), then create once more. Do not retry in a loop";
  }
  if (code === "sandbox_token_refused") {
    return "a sandbox agent token never publishes to a channel and never promotes — publish with --app-env dev-<handle>";
  }
  if (code === "credential_shaped_value") {
    return "a sandbox's secret holds a third party's key, never an Oxy token, key or session — this token included. Do not retry with the same value";
  }
  if (status === 401) return `${DEAD_TOKEN_DETAIL} ${STOP_AND_REPORT}`;
  if (status === 403) return `the token is not allowed to do this. ${STOP_AND_REPORT}`;
  if (status === 404) {
    return "a sandbox agent token reaches only the apps it was minted for (`oxyc whoami`) and the dev-<handle> sandboxes it created (`oxyc env list <app>`) — anything else answers 404, production and staging included";
  }
  return undefined;
}

/** What a sandbox agent should do about a refused request, if anything. */
export function sandboxRefusalHint(status: number, body: string): string | undefined {
  return hintFor(status, readRefusal(body).code);
}

/**
 * `headline`, then the server's own sentence when its body carried one:
 * `404 Not Found — <url>: this app has no environment dev-a1`.
 *
 * ONE LINE, because one line is all some readers get: `oxyc mcp` hands a model
 * the message and the hint and never the body, so a reason left in `detail`
 * is a refusal with no reason.
 */
export function withReason(headline: string, refusal: Refusal): string {
  const line = printable(headline);
  return refusal.reason ? `${line}: ${refusal.reason}` : line;
}

/**
 * `errorForResponse`, with the server's reason on the error line and the hint
 * a sandbox agent can act on. The generic hints say "try `oxyc login` again"
 * and "check the path with `oxyc routes`": the first is the one thing this
 * caller must never do, and it cannot do the second.
 */
export function sandboxTokenError(response: ApiResponse): CliError {
  const base = errorForResponse(response);
  const refusal = readRefusal(response.body);
  return new CliError(withReason(base.message, refusal), {
    code: base.code,
    // The body is the deployment's: kept whole, minus what a terminal acts on.
    detail: base.detail === undefined ? undefined : printableLines(base.detail),
    hint: hintFor(response.status, refusal.code),
    serverCode: refusal.code,
    serverMessage: refusal.reason
  });
}
