/**
 * One authenticated HTTP request to an Oxy deployment.
 *
 * This is the layer every other command goes through, so it is also the only
 * place that knows how a credential is attached and how a failure becomes an
 * exit code. A command that built its own `fetch` would be a second answer to
 * both questions, and the second answer is always the one that forgets the
 * `X-API-Key` surface.
 */

import { usableAsApiKey } from "../auth/token-kind.js";
import { CliError, ExitCode, exitCodeForStatus } from "../util/errors.js";
import { cacheKey, readCache, writeCache } from "./cache.js";
import { isExternalSurface } from "./paths.js";
import { userAgent } from "./user-agent.js";

export interface RequestOptions {
  target: string;
  /** Already normalised and placeholder-substituted. */
  path: string;
  method: string;
  body?: string;
  headers?: Record<string, string>;
  bearer?: string;
  apiKey?: string;
  /** Milliseconds a cached response stays usable. 0 disables the cache. */
  cacheMs?: number;
  timeoutMs?: number;
}

export interface ApiResponse {
  status: number;
  statusText: string;
  headers: Record<string, string>;
  body: string;
  url: string;
  /** True when this came from `--cache` rather than the network. */
  fromCache: boolean;
}

/** Requests wait two minutes, matching the Rust CLI's client timeout. */
const DEFAULT_TIMEOUT_MS = 120_000;

/**
 * Build the absolute URL. The target keeps its path, if it has one, because
 * `--target https://host/oxy` is a supported shape for a deployment served
 * under a prefix — concatenating rather than using `new URL(path, target)`,
 * which would discard that prefix.
 */
export function buildUrl(target: string, path: string): string {
  return `${target.replace(/\/+$/, "")}${path}`;
}

/**
 * Pick and attach the credential.
 *
 * The path decides, not the caller: `/external/api/*` is the API-key surface
 * and everything else is the bearer surface. An `Authorization` header the
 * caller passed by hand wins over both — that is the escape hatch for a
 * credential this tool does not model.
 *
 * ANY KIND OF TOKEN IS A BEARER on `/api/**` — a session JWT, `oxy_pat_`,
 * `oxy_sat_`, `oxy_ci_`, a legacy `oxy_<hex>` key, `oxypublish_`. On the
 * API-key surface, `OXY_API_KEY` is what goes out as `X-API-Key`; with none
 * set, a bearer that IS an API token stands in for it, so one `OXY_TOKEN`
 * covers both surfaces. A session JWT never does — it is not a key, and a
 * server that reads `X-API-Key` first would reject the request on it.
 */
export function authHeaders(opts: RequestOptions): Record<string, string> {
  const explicit = Object.keys(opts.headers ?? {}).map((h) => h.toLowerCase());
  const headers: Record<string, string> = {};
  if (isExternalSurface(opts.path)) {
    const key = opts.apiKey ?? (opts.bearer && usableAsApiKey(opts.bearer) ? opts.bearer : "");
    if (key && !explicit.includes("x-api-key")) headers["X-API-Key"] = key;
    // The bearer goes along too when there is one: some external routes accept
    // either, and sending both never makes a request that would have worked
    // fail — the server picks the one it recognises.
    if (opts.bearer && !explicit.includes("authorization")) {
      headers.Authorization = `Bearer ${opts.bearer}`;
    }
    return headers;
  }
  if (opts.bearer && !explicit.includes("authorization")) {
    headers.Authorization = `Bearer ${opts.bearer}`;
  }
  return headers;
}

/**
 * Make the request. Returns the response whatever its status — turning a 4xx
 * into an error is the *caller's* decision, because `--include` and
 * `--paginate` both need to see the failed response before deciding.
 */
export async function request(opts: RequestOptions): Promise<ApiResponse> {
  const url = buildUrl(opts.target, opts.path);
  const method = opts.method.toUpperCase();

  const key = cacheKey(method, url, opts.body, opts.bearer);
  if (opts.cacheMs && opts.cacheMs > 0) {
    const hit = readCache(key, opts.cacheMs);
    if (hit) {
      return {
        status: hit.status,
        statusText: "OK (cached)",
        headers: hit.headers,
        body: hit.body,
        url,
        fromCache: true
      };
    }
  }

  const headers: Record<string, string> = {
    Accept: "application/json",
    // `oxyc/<version>[ agent/<label>][ mcp]` — see `user-agent.ts`.
    "User-Agent": userAgent(),
    ...authHeaders(opts),
    ...(opts.headers ?? {})
  };
  // Default to JSON when sending a body; an explicit `-H content-type:` above
  // has already overwritten this, since the caller's headers are spread last.
  if (
    opts.body !== undefined &&
    !Object.keys(headers).some((h) => h.toLowerCase() === "content-type")
  ) {
    headers["Content-Type"] = "application/json";
  }

  let response: Response;
  try {
    response = await fetch(url, {
      method,
      headers,
      body: opts.body,
      signal: AbortSignal.timeout(opts.timeoutMs ?? DEFAULT_TIMEOUT_MS)
    });
  } catch (cause) {
    const message = (cause as Error).message;
    // A timeout and a refused connection are both "the deployment did not
    // answer", which is retryable — distinct from a request the server
    // rejected, and the distinction is what an agent branches on.
    throw new CliError(`request to ${url} failed: ${message}`, {
      code: ExitCode.UNAVAILABLE,
      hint: message.includes("timed out")
        ? "the deployment may be starting up — retry, or raise --timeout"
        : `check the target is reachable: ${opts.target}`
    });
  }

  const body = await response.text();
  const responseHeaders: Record<string, string> = {};
  response.headers.forEach((value, name) => {
    responseHeaders[name] = value;
  });

  if (opts.cacheMs && opts.cacheMs > 0) {
    writeCache(key, method, response.status, responseHeaders, body);
  }

  return {
    status: response.status,
    statusText: response.statusText,
    headers: responseHeaders,
    body,
    url,
    fromCache: false
  };
}

/**
 * The previews and sandbox surfaces answer a refusal as `{"code":…,
 * "message":…}`. Picked out here, once, so every caller of `errorForResponse`
 * gets it on the `CliError` rather than having to re-parse `detail` itself.
 */
function parseServerError(body: string): { code?: string; message?: string } {
  try {
    const parsed = JSON.parse(body) as { code?: unknown; message?: unknown };
    return {
      code: typeof parsed.code === "string" ? parsed.code : undefined,
      message: typeof parsed.message === "string" ? parsed.message : undefined
    };
  } catch {
    return {};
  }
}

/**
 * Turn a non-2xx response into the error it deserves.
 *
 * The body goes into `detail` rather than the message because it is the part
 * that actually says what was wrong, and it is often several lines of JSON —
 * folded into a one-line message it would be unreadable, and truncated it
 * would lose the field name that identifies the problem.
 */
export function errorForResponse(response: ApiResponse): CliError {
  const code = exitCodeForStatus(response.status);
  const hint =
    code === ExitCode.AUTH
      ? "your token may be expired or lack the role — try `oxyc login` again, or `oxyc assume <org> --reason …` for a tenant surface"
      : code === ExitCode.NOT_FOUND
        ? "check the path with `oxyc routes <filter>` — and note an admin 404 can be a scope boundary, not a missing row"
        : undefined;
  const server = parseServerError(response.body);
  return new CliError(`${response.status} ${response.statusText} — ${response.url}`, {
    code,
    hint,
    detail: response.body.trim() || undefined,
    serverCode: server.code,
    serverMessage: server.message
  });
}

/** JSON-parse a response body, or `undefined` when it is not JSON. */
export function parseJson(body: string): unknown {
  try {
    return JSON.parse(body);
  } catch {
    return undefined;
  }
}
