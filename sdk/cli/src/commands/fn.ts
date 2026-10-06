/**
 * `oxyc fn call` — call a custom app's Oxy Function directly, the way the
 * SDK's `useFunction(name).invoke(body)` does from the browser.
 *
 * `POST {target}/customer-apps/{org}/{app}/fn/{name}` — no `/api` prefix,
 * because this is the public custom-apps surface
 * (`crates/app/src/server/api/custom_apps_functions/mod.rs`), reachable by
 * a bearer OR an API key, and in a sandbox or staging by a publish token too
 * (unlike `env`/`invocations`/`logs`, which refuse one outright). The
 * response is NOT JSON: it is an SSE stream of `log` / `data` / `done` /
 * `error` frames, framed by `sse_event` server-side.
 */

import { parseJson, request } from "../api/request.js";
import { withUserAgent } from "../api/user-agent.js";
import { APP_ENV_HEADER, isProduction, parseAppEnv } from "../apps/environment.js";
import { ensureOk, PUBLISH_TOKEN_PREFIX, UUID_RE } from "../apps/resolve.js";
import {
  readRefusal,
  requireOwnSandbox,
  resolveSandboxApp,
  sandboxRefusalHint,
  withReason
} from "../apps/sandbox-token.js";
import { isSandboxAgentToken } from "../auth/token-kind.js";
import { type Context, notAuthenticated } from "../context/resolve.js";
import { err } from "../ui/tty.js";
import { resolveDataInput } from "../util/data-input.js";
import { CliError, ExitCode, exitCodeForStatus, usageError } from "../util/errors.js";
import { printable, printableLines } from "../util/printable.js";

export interface FnCallResult {
  function: string;
  environment: string;
  invocationId?: string;
  ok: boolean;
  status?: number;
  body?: unknown;
  logs: { level: string; message: string }[];
  error?: string;
}

interface SseFrame {
  event: string;
  data: unknown;
}

/** `event: <name>\ndata: <json>\n\n` blocks, as `sse_event` writes them server-side. */
function parseSseFrames(text: string): SseFrame[] {
  const frames: SseFrame[] = [];
  for (const block of text.split("\n\n")) {
    const trimmed = block.trim();
    if (!trimmed) continue;
    let event = "message";
    let dataLine: string | undefined;
    for (const line of trimmed.split("\n")) {
      if (line.startsWith("event:")) event = line.slice(6).trim();
      else if (line.startsWith("data:")) dataLine = line.slice(5).trim();
    }
    if (dataLine === undefined) continue;
    frames.push({ event, data: parseJson(dataLine) });
  }
  return frames;
}

/**
 * The stream is EITHER zero-or-more `log` frames then one `error` frame, OR
 * zero-or-more `log` frames then a `data` + `done` pair — never both
 * (`custom_apps_functions::mod.rs`'s `success_sse_body` / the `error_msg`
 * branch beside it). `done`'s `status` is the function's OWN `Response`
 * status, which can be any value — a function answering 403 is a function
 * failure, not a transport one, so it is scored here, not by the HTTP status
 * of the POST (which was 200: the stream started).
 */
export function parseFunctionStream(
  text: string
): Pick<FnCallResult, "ok" | "status" | "body" | "logs" | "error"> {
  const frames = parseSseFrames(text);
  const logs = frames
    .filter((f) => f.event === "log")
    .map((f) => f.data as { level: string; message: string });

  const errorFrame = frames.find((f) => f.event === "error");
  if (errorFrame) {
    const data = errorFrame.data as { error?: string; message?: string } | undefined;
    return {
      ok: false,
      logs,
      error: data?.message ?? data?.error ?? "the function reported an error"
    };
  }

  const doneFrame = frames.find((f) => f.event === "done");
  if (doneFrame) {
    const status = (doneFrame.data as { status?: number } | undefined)?.status;
    const ok = typeof status === "number" && status >= 200 && status < 300;
    const dataFrame = frames.find((f) => f.event === "data");
    return {
      ok,
      status,
      body: dataFrame?.data,
      logs,
      error: ok ? undefined : `the function returned ${status}`
    };
  }

  return { ok: false, logs, error: "the stream ended with no `done` or `error` event" };
}

interface FnCreds {
  target: string;
  bearer?: string;
  apiKey?: string;
}

function fnCredential(ctx: Context): FnCreds {
  // The stored bearer, then the API key — no OIDC exchange, as before it existed.
  const bearer = ctx.storedBearer();
  if (bearer) return { target: ctx.target(), bearer };
  const apiKey = ctx.apiKey();
  if (apiKey) return { target: ctx.target(), apiKey };
  throw notAuthenticated(ctx.target(), ctx.flags);
}

/**
 * `<app>` already as `<org>/<app>` needs no lookup — the route takes slugs
 * directly, and a publish token (which may call a function in production)
 * can never reach `/api/admin/apps` anyway. A UUID needs one lookup, which
 * does need a non-machine credential — same constraint `checks.ts` has in
 * the opposite direction.
 */
async function resolveOrgAppSlug(
  creds: FnCreds,
  app: string
): Promise<{ orgSlug: string; appSlug: string }> {
  if (!UUID_RE.test(app)) {
    const [orgSlug, ...rest] = app.split("/");
    const appSlug = rest.join("/");
    if (!orgSlug || !appSlug) {
      throw usageError(
        `"${app}" is not a valid app`,
        '<app> is "<org-slug>/<app-slug>" or an app UUID'
      );
    }
    return { orgSlug, appSlug };
  }
  // A UUID needs `GET /api/admin/apps/{id}`, which a publish token may never
  // reach — the same invariant `checks.ts` enforces, in the opposite
  // direction (there, the admin APP LISTING a slug needs is refused; here,
  // it's the admin LOOKUP a UUID needs). Refuse client-side rather than
  // sending a request the server would 403 anyway.
  if (creds.bearer?.startsWith(PUBLISH_TOKEN_PREFIX)) {
    throw usageError(
      `a publish token cannot resolve ${app} — pass "<org-slug>/<app-slug>"`,
      "resolving a UUID means GET /api/admin/apps/{id}, which a publish token may not reach"
    );
  }
  // A sandbox agent token may not reach it either, and does not need to: the
  // token's own app list has the slugs.
  if (isSandboxAgentToken(creds.bearer)) {
    return resolveSandboxApp(creds.target, creds.bearer, app);
  }
  const response = await request({
    target: creds.target,
    path: `/api/admin/apps/${app}`,
    method: "GET",
    bearer: creds.bearer,
    headers: creds.apiKey ? { "X-API-Key": creds.apiKey } : undefined
  });
  ensureOk(response);
  const payload = parseJson(response.body) as { slug?: string; org_slug?: string } | undefined;
  if (!payload?.slug || !payload.org_slug) {
    throw new CliError(`GET /api/admin/apps/${app} did not return slug and org_slug`, {
      code: ExitCode.UNAVAILABLE
    });
  }
  return { orgSlug: payload.org_slug, appSlug: payload.slug };
}

function printFnCallResult(result: FnCallResult): void {
  if (process.env.OXYC_QUIET) return;
  for (const line of result.logs) process.stderr.write(`  ${line.level}: ${line.message}\n`);
  if (result.ok) {
    process.stderr.write(`${err.green("✓")} ${result.function} → ${result.status}\n`);
  } else {
    process.stderr.write(
      `${err.red("✗")} ${result.function} failed: ${result.error ?? result.status}\n`
    );
  }
}

/**
 * The error for a call the deployment refused before the function ran.
 *
 * The body may be JSON with a `message` (the environment gate, a function the
 * build does not have), a sentence in some other shape, or nothing at all: a
 * sandbox agent token outside its own sandbox is answered a bare `404`. The
 * message is one readable line in every case, and never anything but a string.
 */
function fnRefused(response: Response, text: string, url: string, creds: FnCreds): CliError {
  const refusal = readRefusal(text);
  const parsed = parseJson(text) as { message?: unknown } | undefined;
  const said =
    typeof parsed?.message === "string"
      ? printable(parsed.message.replace(/\s+/g, " ")).trim()
      : "";
  const status = `${response.status} ${response.statusText} — ${url}`;
  return new CliError(said || withReason(status, refusal), {
    code: exitCodeForStatus(response.status),
    detail: printableLines(text).trim() || undefined,
    hint: isSandboxAgentToken(creds.bearer) ? sandboxRefusalHint(response.status, text) : undefined,
    serverCode: refusal.code,
    serverMessage: refusal.reason
  });
}

/**
 * POST the function and parse its SSE stream into a result — no printing, and
 * no throw for a function-level failure (`result.ok === false`): that
 * decision belongs to the caller. `runFnCall` below prints and throws for the
 * CLI's exit-code contract; the `oxyc mcp` tool returns the result as-is and
 * lets the agent read `ok`.
 */
export async function fnCall(
  ctx: Context,
  app: string,
  fn: string,
  opts: { appEnv?: string; data?: string; timeoutSeconds: number }
): Promise<FnCallResult> {
  const appEnv = opts.appEnv !== undefined ? parseAppEnv(opts.appEnv) : undefined;
  const creds = fnCredential(ctx);
  if (creds.bearer?.startsWith(PUBLISH_TOKEN_PREFIX) && !isProduction(appEnv)) {
    throw usageError(
      `a publish token cannot call a function in ${appEnv}`,
      "sandboxes and staging need a staff credential — oxyc login, or OXY_TOKEN set to a user token"
    );
  }
  // Before any request: with no `--app-env` this is a production call.
  if (isSandboxAgentToken(creds.bearer)) requireOwnSandbox("oxyc fn call", appEnv);

  const { orgSlug, appSlug } = await resolveOrgAppSlug(creds, app);
  const url = `${creds.target.replace(/\/+$/, "")}/customer-apps/${orgSlug}/${appSlug}/fn/${encodeURIComponent(fn)}`;
  const headers = withUserAgent({ "content-type": "application/json" });
  if (creds.bearer) headers.authorization = `Bearer ${creds.bearer}`;
  if (creds.apiKey) headers["x-api-key"] = creds.apiKey;
  if (!isProduction(appEnv)) headers[APP_ENV_HEADER] = appEnv as string;

  let response: Response;
  try {
    response = await fetch(url, {
      method: "POST",
      headers,
      body: resolveDataInput(opts.data),
      signal: AbortSignal.timeout(opts.timeoutSeconds * 1000)
    });
  } catch (cause) {
    throw new CliError(`POST ${url} failed: ${(cause as Error).message}`, {
      code: ExitCode.UNAVAILABLE
    });
  }

  const invocationId = response.headers.get("x-oxy-invocation-id") ?? undefined;
  const environment = appEnv ?? "production";

  if (!response.ok) throw fnRefused(response, await response.text(), url, creds);

  const streamed = parseFunctionStream(await response.text());
  return { function: fn, environment, invocationId, ...streamed };
}

export async function runFnCall(
  ctx: Context,
  app: string,
  fn: string,
  opts: { appEnv?: string; data?: string; json: boolean; timeoutSeconds: number }
): Promise<void> {
  const result = await fnCall(ctx, app, fn, opts);

  if (opts.json) process.stdout.write(`${JSON.stringify(result)}\n`);
  else printFnCallResult(result);

  if (!result.ok) {
    throw new CliError(result.error ?? `${fn} failed`, { code: ExitCode.FAILURE });
  }
}
