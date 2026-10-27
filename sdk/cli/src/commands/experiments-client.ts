/**
 * The wire half of `oxyc experiments`: the paths, one request helper that maps
 * a refusal onto the exit-code contract, and the file/flag readers the design
 * and candidate inputs share.
 *
 * Every route is `/api/{workspace}/experiments…` (`server/router/workspace.rs`).
 * No branch is ever sent: these routes read persisted rows or enqueue jobs, so
 * any replica answers — the web client omits it for the same reason.
 */

import { parse as parseYaml } from "yaml";
import { substitutePlaceholders } from "../api/paths.js";
import { type ApiResponse, parseJson, request } from "../api/request.js";
import type { Context } from "../context/resolve.js";
import { resolveDataInput } from "../util/data-input.js";
import { CliError, ExitCode, exitCodeForStatus, usageError } from "../util/errors.js";

/** `/api/{workspace}/experiments[/<name>[/<rest>]]`, the workspace resolved now. */
export function experimentPath(ctx: Context, name?: string, rest?: string): string {
  const root = substitutePlaceholders("/api/{workspace}/experiments", ctx.placeholders());
  if (name === undefined) return root;
  return `${root}/${encodeURIComponent(name)}${rest ? `/${rest}` : ""}`;
}

/** What the server said, in its own words: plain text, `{error}` or `{message}`. */
function serverWords(body: string): string | undefined {
  const trimmed = body.trim();
  if (!trimmed || trimmed === "{}") return undefined;
  const parsed = parseJson(trimmed) as { error?: unknown; message?: unknown } | undefined;
  if (parsed && typeof parsed === "object") {
    if (typeof parsed.message === "string" && parsed.message) return parsed.message;
    if (typeof parsed.error === "string" && parsed.error) return parsed.error;
  }
  return trimmed.slice(0, 1000);
}

/** Why a status refused, in the terms someone running an experiment acts on. */
function refusalHint(status: number): string | undefined {
  switch (status) {
    case 403:
      return "your workspace role does not allow this — or, as staff, start `oxyc assume start --org <slug>` first";
    case 503:
      return "this route needs the workspace's working copy, which this deployment does not serve";
    default:
      return undefined;
  }
}

export function experimentError(method: string, path: string, response: ApiResponse): CliError {
  const words = serverWords(response.body);
  return new CliError(words ?? `${method} ${path} failed (${response.status})`, {
    code: exitCodeForStatus(response.status),
    detail: words ? `${method} ${path} → ${response.status}` : undefined,
    hint: refusalHint(response.status),
    remedy: response.status === 401 ? "oxyc login --env <env>" : undefined
  });
}

/** The raw response, for callers that read a non-2xx as a value (`result`'s 409s). */
export async function send(
  ctx: Context,
  method: string,
  path: string,
  body?: unknown
): Promise<ApiResponse> {
  return request({
    target: ctx.target(),
    path,
    method,
    bearer: await ctx.bearer(),
    body: body === undefined ? undefined : JSON.stringify(body)
  });
}

/** A 2xx JSON body, or the error its status deserves. */
export async function call<T>(
  ctx: Context,
  method: string,
  path: string,
  body?: unknown
): Promise<T> {
  const response = await send(ctx, method, path, body);
  if (response.status < 200 || response.status >= 300) {
    throw experimentError(method, path, response);
  }
  const parsed = parseJson(response.body);
  if (parsed === undefined || parsed === null) {
    throw new CliError(`${method} ${path} did not return JSON`, {
      code: ExitCode.FAILURE,
      detail: response.body.trim().slice(0, 500) || undefined,
      hint: "the deployment may predate this route — `oxyc routes experiments` lists what it mounts"
    });
  }
  return parsed as T;
}

/**
 * YAML or JSON (JSON is YAML), from `-`, `@file`, a path, or an inline literal.
 * A bare path is accepted for `--file`; `--candidate` takes `@file` or a literal.
 */
export function readDocument(
  value: string,
  what: string,
  opts: { pathLike?: boolean } = {}
): unknown {
  const source = opts.pathLike && value !== "-" && !value.startsWith("@") ? `@${value}` : value;
  const text = resolveDataInput(source);
  try {
    return parseYaml(text);
  } catch (cause) {
    throw usageError(`${what} is not YAML or JSON: ${(cause as Error).message}`);
  }
}

/** The design file: the bare `ExperimentConfig`, as `design <name>` prints it. */
export function readConfig(file: string | undefined): Record<string, unknown> {
  if (!file) {
    throw usageError(
      "--file <path|-> is required",
      "start from `oxyc experiments design <name> > x.yml`"
    );
  }
  const doc = readDocument(file, file, { pathLike: true });
  if (!doc || typeof doc !== "object" || Array.isArray(doc)) {
    throw usageError(`${file} must hold one experiment design (a mapping), not ${describe(doc)}`);
  }
  return doc as Record<string, unknown>;
}

/** `--candidate` values, each one design; a document holding a list contributes each item. */
export function readCandidates(values: string[]): unknown[] {
  const out: unknown[] = [];
  for (const value of values) {
    const doc = readDocument(value, "--candidate");
    const items = Array.isArray(doc)
      ? doc
      : doc &&
          typeof doc === "object" &&
          Array.isArray((doc as { candidates?: unknown }).candidates)
        ? (doc as { candidates: unknown[] }).candidates
        : [doc];
    for (const item of items) {
      if (!item || typeof item !== "object" || !("shape" in item)) {
        throw usageError(
          "every --candidate needs a shape: waves or switchback",
          '--candidate "{shape: waves, wave_sizes: [4, 2], first_switch_on: 2026-11-02, spacing_days: 14}"'
        );
      }
      out.push(item);
    }
  }
  if (out.length === 0) {
    throw usageError(
      "price needs at least one --candidate",
      '--candidate "{shape: switchback, first_period_on: 2026-11-02, period_days: 7, pairs: 6}"'
    );
  }
  return out;
}

function describe(value: unknown): string {
  if (Array.isArray(value)) return "a list";
  if (value === null || value === undefined) return "nothing";
  return `a ${typeof value}`;
}
