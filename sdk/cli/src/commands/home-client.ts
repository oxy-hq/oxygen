/**
 * The wire half of `oxyc home`: the paths, one request helper that maps a
 * refusal onto the exit-code contract, and the shapes of the two answers the
 * commands combine.
 *
 *   GET    /api/orgs/{org_id}/home             the description (`HomeDescription`)
 *   PUT    /api/orgs/{org_id}/home/layout      {layout, revision?, dry_run?}
 *   DELETE /api/orgs/{org_id}/home/layout      back to the default
 *   GET    /api/home/widgets?org=<slug>        what the home draws, with each view
 */

import type { HomeLayoutProblem } from "@oxy-hq/sdk/manifest";
import { type ApiResponse, parseJson, request } from "../api/request.js";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode, exitCodeForStatus } from "../util/errors.js";

/** A button, row or view as the widgets route answers it. Only what the text draws is typed. */
export interface FeedButton {
  label?: string | null;
}
export interface FeedRow {
  title?: string | null;
  detail?: string | null;
  state?: string | null;
}
export interface FeedMetric {
  value?: string | null;
  label?: string | null;
  delta?: string | null;
  trend?: string | null;
}
export interface FeedView {
  headline?: string | null;
  detail?: string | null;
  level?: string | null;
  progress?: { done: number; total: number } | null;
  metric?: FeedMetric | null;
  rows?: FeedRow[] | null;
  buttons?: FeedButton[] | null;
  /** Seconds until the home reads the widget again (60–86400). */
  refresh_after?: number | null;
}
/** One widget copy the home draws. */
export interface FeedWidget {
  app: { id?: string; slug: string; name?: string; org_slug?: string; url?: string };
  /** The widget's id in its app's manifest. */
  id: string;
  /** This copy: `<app>/<widget>`, or `<app>/<widget>#<instance>`. */
  key: string;
  /** The layout item's `id`, or null for the one unnamed copy. */
  instance?: string | null;
  /** This copy's settings, defaults filled in. */
  params?: Record<string, string | number | boolean> | null;
  title: string;
  size: string;
  sizes?: string[];
  view: FeedView | null;
  error?: string | null;
}
/** A section of the home as the widgets route lists it; a widgets group names its copies by `key`. */
export type FeedSection =
  | { kind: "widgets"; title: string | null; items: string[] }
  | { kind: "apps" }
  | { kind: "insights" };
/** The `GET /api/home/widgets` answer. */
export interface WidgetsFeed {
  widgets: FeedWidget[];
  sections?: FeedSection[];
}

export const homePath = (orgId: string, rest = ""): string =>
  `/api/orgs/${encodeURIComponent(orgId)}/home${rest}`;
export const widgetsPath = (orgSlug: string): string =>
  `/api/home/widgets?org=${encodeURIComponent(orgSlug)}`;

/** The raw response, for callers that read a 409 or 422 as a value. */
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

function refusalHint(status: number): string | undefined {
  switch (status) {
    case 401:
    case 403:
      return "writing a layout needs an org owner or admin; staff need `oxyc assume start --org <slug>` first";
    case 404:
      return "no such org, or this deployment predates home layouts — `oxyc routes home` lists what it mounts";
    default:
      return undefined;
  }
}

/** The error a non-2xx deserves; 422 and 409 have their own, richer, ones in `home.ts`. */
export function homeError(method: string, path: string, response: ApiResponse): CliError {
  const words = serverWords(response.body);
  return new CliError(words ?? `${method} ${path} failed (${response.status})`, {
    code: exitCodeForStatus(response.status),
    detail: words ? `${method} ${path} → ${response.status}` : undefined,
    hint: refusalHint(response.status),
    remedy: response.status === 401 ? "oxyc login --env <env>" : undefined
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
  return readJson<T>(method, path, response);
}

export function readJson<T>(method: string, path: string, response: ApiResponse): T {
  if (response.status < 200 || response.status >= 300) throw homeError(method, path, response);
  const parsed = parseJson(response.body);
  if (parsed === undefined || parsed === null) {
    throw new CliError(`${method} ${path} did not return JSON`, {
      code: ExitCode.FAILURE,
      detail: response.body.trim().slice(0, 500) || undefined,
      hint: "the deployment may predate this route — `oxyc routes home` lists what it mounts"
    });
  }
  return parsed as T;
}

/** `path: message (fix)`, one line per problem a `422 layout_invalid` listed. */
export function problemLines(body: string): string[] {
  const parsed = parseJson(body) as { problems?: unknown } | undefined;
  if (!parsed || !Array.isArray(parsed.problems)) return [];
  return (parsed.problems as Partial<HomeLayoutProblem>[]).map((p) => {
    const where = p.path ? `${p.path}: ` : "";
    return `${where}${p.message ?? "invalid"}${p.fix ? ` (${p.fix})` : ""}`;
  });
}
