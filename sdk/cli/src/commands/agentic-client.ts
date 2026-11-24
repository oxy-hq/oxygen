/**
 * The wire half of `oxyc agentic`: the workspace's `/analytics` paths, the
 * refusals a caller can act on, and one JSON request helper — shared by
 * `run | events | answer | cancel` (`agentic-run.ts`) and
 * `list | inspect | status` (`agentic-runs.ts`).
 *
 * Every route sits under `/api/{workspace_id}/analytics`, so every verb needs
 * `--workspace <id>`. The Rust `oxy agentic` ran against the nil "local"
 * workspace and a database URL; there is no such workspace on a deployment.
 *
 * Two surfaces, two audiences. `runs`, `…/events`, `…/answer` and `…/cancel`
 * are the chat's routes: any member of the workspace. The `coordinator/*` reads
 * behind `list`, `inspect` and `status` are the Orchestrator Dashboard's, an
 * Oxygen Nexus surface: org owners and admins, staff, and members Oxy granted
 * the workspace (`403 factory_access_required` for anyone else).
 */

import { normalizePath, substitutePlaceholders } from "../api/paths.js";
import type { Context } from "../context/resolve.js";
import { call, type RefusalReason } from "./oltp-client.js";

/** `/api/{workspace}/analytics<suffix>`, with `{workspace}` from `--workspace`. */
export function analyticsPath(ctx: Context, suffix: string): string {
  return substitutePlaceholders(
    normalizePath(`{workspace}/analytics${suffix}`),
    ctx.placeholders()
  );
}

/** `…/runs/<id><rest>`, the run id escaped. */
export function runSuffix(runId: string, rest = ""): string {
  return `/runs/${encodeURIComponent(runId)}${rest}`;
}

/** Why an `/analytics` route refused, in the terms a caller acts on. */
export function agenticRefusal(status: number): string | undefined {
  switch (status) {
    case 400:
      return "the server rejected the request — the agent could not start (the message below says why)";
    case 401:
      return "not authenticated for this target";
    case 403:
      return (
        "not allowed here. A run needs membership of the workspace; list, inspect and status read " +
        "the Orchestrator Dashboard, which is for org owners and admins, Oxy staff, and members Oxy " +
        "granted the workspace (factory_access_required). Staff act inside a tenant through " +
        "`oxyc assume`"
      );
    case 404:
      return "no such workspace or run here — or one your credential cannot see";
    case 409:
      return "the run is not waiting for an answer (it is running, or already ended)";
    case 410:
      return "the run can no longer take an answer (no suspension is recorded for it)";
    case 503:
      return (
        "the deployment cannot start or resume this run yet (the workspace is not compiled, or " +
        "the run is driven by another pod right now) — retryable"
      );
    default:
      return undefined;
  }
}

const ANALYTICS = { refusal: agenticRefusal as RefusalReason, routesArea: "analytics" } as const;

/** A 2xx JSON body from an `/analytics` route, or the error its status deserves. */
export function agenticCall<T>(
  ctx: Context,
  method: string,
  suffix: string,
  body?: unknown
): Promise<T> {
  return call<T>(ctx, method, analyticsPath(ctx, suffix), { ...ANALYTICS, body });
}

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
