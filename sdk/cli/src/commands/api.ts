/**
 * `oxyc api` — the `gh api` of this tool.
 *
 * Everything the flags do is `gh api`'s, down to the letters, because the
 * whole value proposition is that muscle memory and half-remembered snippets
 * transfer. Where we differ it is for a reason written down at the site:
 * the `{org}` placeholder family, `--md`, and the pagination heuristic.
 */

import { readFileSync } from "node:fs";
import { parseDuration } from "../api/cache.js";
import { paramsToQuery, parseFields } from "../api/fields.js";
import { formatBody } from "../api/output.js";
import { paginate } from "../api/paginate.js";
import { isExternalSurface, normalizePath, substitutePlaceholders } from "../api/paths.js";
import { errorForResponse, request } from "../api/request.js";
import { refuseSandboxToken } from "../apps/sandbox-token.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";

export interface ApiFlags {
  method?: string;
  rawField: string[];
  field: string[];
  header: string[];
  input?: string;
  jq?: string;
  md?: boolean;
  paginate?: boolean;
  paginateKey?: string;
  maxPages?: string;
  slurp?: boolean;
  cache?: string;
  include?: boolean;
  silent?: boolean;
  verbose?: boolean;
  timeout?: string;
}

/**
 * Read `--input`: `-` is stdin, `@file` or a bare path is a file.
 *
 * A bare path is accepted as well as `@path` because `--input` already means
 * "from a file" — requiring the `@` too is ceremony, and `gh` accepts the bare
 * form here as well.
 */
function readInput(value: string): string {
  if (value === "-") return readFileSync(0, "utf8");
  const path = value.startsWith("@") ? value.slice(1) : value;
  try {
    return readFileSync(path, "utf8");
  } catch (cause) {
    throw usageError(`could not read ${path}: ${(cause as Error).message}`);
  }
}

/** Parse `-H 'Name: value'` into a header map. */
function parseHeaders(raw: string[]): Record<string, string> {
  const headers: Record<string, string> = {};
  for (const entry of raw) {
    const idx = entry.indexOf(":");
    if (idx < 0) throw usageError(`invalid --header '${entry}', expected 'Name: value'`);
    headers[entry.slice(0, idx).trim()] = entry.slice(idx + 1).trim();
  }
  return headers;
}

/**
 * `gh` makes `--template`, `--jq`, `--silent` and `--verbose` mutually
 * exclusive, and the reason is worth keeping: each one *replaces* the output,
 * so combining two means one silently wins. We have no `--template`, and
 * `--md` composes with `--jq` by design (jq selects, md renders), so the
 * exclusion is over the three that genuinely conflict.
 */
function rejectConflictingOutput(flags: ApiFlags): void {
  const chosen = [
    flags.jq !== undefined && "--jq",
    flags.silent && "--silent",
    flags.verbose && "--verbose"
  ].filter(Boolean) as string[];
  if (chosen.length > 1) {
    throw usageError(`only one of ${chosen.join(", ")} may be used`);
  }
  if (flags.slurp && !flags.paginate) {
    throw usageError("--slurp requires --paginate", "it wraps each page as an array element");
  }
}

export async function runApi(ctx: Context, rawPath: string, flags: ApiFlags): Promise<void> {
  // Against ANY path, and before anything is read or sent: a hand-built
  // request is the generic escape hatch, and this token is the opposite of
  // generic. The server answers it 404 outside the sandbox loop regardless.
  refuseSandboxToken(
    ctx.storedBearer(),
    "use `oxyc api`",
    "it does the sandbox loop through its own verbs — env, publish --app-env, fn call, checks run, invocations, logs, env secret"
  );
  rejectConflictingOutput(flags);

  const fields = parseFields(flags.rawField, flags.field);
  const body = flags.input !== undefined ? readInput(flags.input) : undefined;

  if (body !== undefined && fields.present) {
    throw usageError(
      "--input cannot be combined with -f/-F",
      "either send a raw body or build one from fields, not both"
    );
  }

  // Method defaults to GET, or POST when something is being sent. Same rule as
  // `gh api`, and the same rule the Rust `oxy api` used.
  const method = (
    flags.method ?? (body !== undefined || fields.present ? "POST" : "GET")
  ).toUpperCase();
  const carriesBody = !["GET", "HEAD", "DELETE"].includes(method);

  let path = substitutePlaceholders(normalizePath(rawPath), ctx.placeholders());

  // Fields on a body-less method become query parameters rather than being
  // refused — this API has plenty of parameterised GETs, and the alternative
  // is forcing callers back to hand-built URLs for the commonest case.
  let requestBody: string | undefined = body;
  if (fields.present) {
    if (carriesBody) {
      requestBody = JSON.stringify(fields.params);
    } else {
      const query = paramsToQuery(fields.params);
      if (query) path += (path.includes("?") ? "&" : "?") + query;
    }
  }

  const target = ctx.target();
  const external = isExternalSurface(path);
  // The API-key surface accepts a key OR a bearer, so a missing key is only
  // fatal when there is no bearer either. Demanding both would refuse a
  // request that would have worked.
  const apiKey = ctx.apiKey();
  const bearer = external && apiKey ? ctx.storedBearer() : await ctx.bearer();

  if (external && !apiKey && !bearer) {
    throw new CliError(`${path} is the API-key surface and no key is set`, {
      code: ExitCode.AUTH,
      hint: `set ${ctx.flags.apiKeyEnv ?? "OXY_API_KEY"}, or run \`oxyc login\` for a bearer`
    });
  }

  const common = {
    target,
    path,
    method,
    body: requestBody,
    headers: parseHeaders(flags.header),
    bearer,
    apiKey,
    cacheMs: flags.cache ? parseDuration(flags.cache) : 0,
    timeoutMs: flags.timeout ? parseDuration(flags.timeout) : undefined
  };

  if (flags.verbose) {
    log.info(`${method} ${target}${path}`);
    if (requestBody) log.info(requestBody);
  }

  if (flags.paginate) {
    const merged = await paginate({
      ...common,
      paginateKey: flags.paginateKey,
      slurp: flags.slurp,
      maxPages: flags.maxPages ? Number(flags.maxPages) : undefined
    }).catch((cause) => {
      throw onBundleMount(cause, path);
    });
    emit(formatBody(merged, { jq: flags.jq, md: flags.md, silent: flags.silent }));
    return;
  }

  const response = await request(common);

  if (flags.include || flags.verbose) {
    // Headers go to STDOUT, not stderr, because `-i` asks for them as part of
    // the response — `gh api -i | head` has to show them.
    process.stdout.write(`HTTP ${response.status} ${response.statusText}\n`);
    for (const [name, value] of Object.entries(response.headers)) {
      process.stdout.write(`${name}: ${value}\n`);
    }
    process.stdout.write("\n");
  }
  if (response.fromCache) log.info("(served from --cache)");

  if (response.status < 200 || response.status >= 300) {
    throw onBundleMount(errorForResponse(response), path);
  }

  noteNullBody(response.body, bearer);
  emit(formatBody(response.body, { jq: flags.jq, md: flags.md, silent: flags.silent }));
}

/**
 * A 404 under `/customer-apps` gets a hint naming `/api/customer-apps`.
 *
 * `customer-apps` is in `TOP_LEVEL_MOUNTS` on purpose: `/customer-apps/<org>/<app>/…`
 * is where the server serves an app's bundle, and `oxyc api customer-apps/acme/store/`
 * has to reach it without an `/api` prefix. The app registry is a different
 * mount, `/api/customer-apps/…`, so `oxyc api customer-apps` and
 * `oxyc api customer-apps/<id>/builds` are 404s whose generic hint ("check the
 * path with `oxyc routes`") sends the caller to a listing that shows the path
 * they believe they typed.
 */
function onBundleMount(cause: unknown, path: string): unknown {
  if (!(cause instanceof CliError) || cause.code !== ExitCode.NOT_FOUND) return cause;
  if (!/^\/customer-apps(?:[/?]|$)/.test(path)) return cause;
  return new CliError(cause.message, {
    code: cause.code,
    detail: cause.detail,
    hint: `/customer-apps/<org>/<app>/… serves an app's bundle; the app registry API is under /api — try \`oxyc api api${path}\``
  });
}

/**
 * A bare `null` with a 200 is ambiguous, and the ambiguity is expensive.
 *
 * An expired session on this API does not always answer 401 — `/api/user`
 * answers `200 null`, which an agent reads as "there is no such user" and goes
 * off to debug the wrong thing. Some endpoints do legitimately answer `null`
 * for a missing row, so this cannot be an error; it is a note on STDERR, which
 * leaves the piped body untouched and costs nothing when the null was real.
 */
function noteNullBody(body: string, bearer: string | undefined): void {
  if (!bearer || body.trim() !== "null") return;
  log.warn(
    "the response is `null` with a 200 — on this API that is also what an expired session looks like"
  );
  log.hint("oxyc whoami   — to tell the two apart");
}

/** Write a body, with exactly one trailing newline and none for empty output. */
function emit(text: string): void {
  if (!text) return;
  process.stdout.write(text.endsWith("\n") ? text : `${text}\n`);
}
