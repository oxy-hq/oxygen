/**
 * The wire half of `oxyc flags`: paths, one request helper that maps a refusal
 * onto the exit-code contract, and the `--file` reader.
 *
 * The staff routes mount under `/api/admin/feature-flags` (PlatformOperate);
 * the member-facing one under `/api/orgs/{org_id}/feature-flags`. Writes are
 * deployment-wide: there is no workspace or branch to send.
 */

import { parse as parseYaml } from "yaml";
import { type ApiResponse, parseJson, request } from "../api/request.js";
import type { Context } from "../context/resolve.js";
import { resolveDataInput } from "../util/data-input.js";
import { CliError, ExitCode, exitCodeForStatus, usageError } from "../util/errors.js";
import type { Targeting } from "./flags-rules.js";

const ADMIN = "/api/admin/feature-flags";

export const flagsPath = (key?: string): string =>
  key === undefined ? ADMIN : `${ADMIN}/${encodeURIComponent(key)}`;

export function evaluatePath(orgId?: string, userId?: string): string {
  const query = new URLSearchParams();
  if (orgId) query.set("org_id", orgId);
  if (userId) query.set("user_id", userId);
  const qs = query.toString();
  return `${ADMIN}/evaluate${qs ? `?${qs}` : ""}`;
}

export const orgFlagsPath = (orgId: string): string =>
  `/api/orgs/${encodeURIComponent(orgId)}/feature-flags`;

/** What the server said, in its own words: `{error}`, `{message}` or plain text. */
function serverWords(body: string): string | undefined {
  const trimmed = body.trim();
  if (!trimmed || trimmed === "{}") return undefined;
  const parsed = parseJson(trimmed) as { error?: unknown; message?: unknown } | undefined;
  if (parsed && typeof parsed === "object") {
    if (typeof parsed.error === "string" && parsed.error) return parsed.error;
    if (typeof parsed.message === "string" && parsed.message) return parsed.message;
  }
  return trimmed.slice(0, 1000);
}

/** Why a status refused, in the terms someone changing a flag acts on. */
function refusalHint(status: number): string | undefined {
  switch (status) {
    case 403:
      return "feature flags are staff-only (PlatformOperate); as staff, check your platform standing — an assume-role session does not grant it";
    case 409:
      return "the stored rules are unreadable by this build; replace them (`oxyc flags rules <key> --file rules.json`) or `--clear`";
    default:
      return undefined;
  }
}

export function flagsError(method: string, path: string, response: ApiResponse): CliError {
  const words = serverWords(response.body);
  const rejected = response.status === 422;
  return new CliError(
    rejected && words
      ? `the server refused these rules: ${words}`
      : (words ?? `${method} ${path} failed (${response.status})`),
    {
      code: exitCodeForStatus(response.status),
      detail: words ? `${method} ${path} → ${response.status}` : undefined,
      hint: refusalHint(response.status),
      remedy: response.status === 401 ? "oxyc login --env <env>" : undefined
    }
  );
}

/** A 2xx JSON body, or the error its status deserves. */
export async function call<T>(
  ctx: Context,
  method: string,
  path: string,
  body?: unknown
): Promise<T> {
  const response = await request({
    target: ctx.target(),
    path,
    method,
    bearer: await ctx.bearer(),
    body: body === undefined ? undefined : JSON.stringify(body)
  });
  if (response.status < 200 || response.status >= 300) {
    throw flagsError(method, path, response);
  }
  const parsed = parseJson(response.body);
  if (parsed === undefined || parsed === null) {
    throw new CliError(`${method} ${path} did not return JSON`, {
      code: ExitCode.FAILURE,
      detail: response.body.trim().slice(0, 500) || undefined,
      hint: "the deployment may predate this route — `oxyc routes feature-flags` lists what it mounts"
    });
  }
  return parsed as T;
}

/** `--file`: the rules as JSON or YAML, from a path or `-` for stdin. */
export function readRulesFile(file: string): Targeting {
  const source = file === "-" || file.startsWith("@") ? file : `@${file}`;
  let doc: unknown;
  try {
    doc = parseYaml(resolveDataInput(source));
  } catch (cause) {
    throw usageError(`${file} is not YAML or JSON: ${(cause as Error).message}`);
  }
  if (!doc || typeof doc !== "object" || Array.isArray(doc)) {
    throw usageError(
      `${file} must hold one rules object, as \`oxyc flags show <key> --json\` prints under "targeting"`
    );
  }
  return doc as Targeting;
}
