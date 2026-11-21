/**
 * The wire half of `oxyc airway`: the workspace's `/agentic-airway` paths, the
 * refusals an operator can act on, and one JSON request helper — shared by
 * `run` (`airway-run.ts`), `backfill | coverage` (`airway-backfill.ts`) and
 * `leases | release-lease` (`airway-leases.ts`).
 *
 * Every route here sits under `/api/{workspace_id}/agentic-airway`, so every
 * verb needs `--workspace <id>`: there is no "local workspace" any more. The
 * Rust `oxy airway` defaulted to the nil workspace, which belonged to no org and
 * made a cloud lease invisible to the command that existed to clear it.
 */

import { normalizePath, substitutePlaceholders } from "../api/paths.js";
import type { Context } from "../context/resolve.js";
import { call, type RefusalReason } from "./oltp-client.js";

/** `/api/{workspace}/agentic-airway<suffix>`, with `{workspace}` from `--workspace`. */
export function airwayPath(ctx: Context, suffix: string): string {
  return substitutePlaceholders(
    normalizePath(`{workspace}/agentic-airway${suffix}`),
    ctx.placeholders()
  );
}

/** Why an `/agentic-airway` route refused, in the terms an operator acts on. */
export function airwayRefusal(status: number): string | undefined {
  switch (status) {
    case 400:
      return "the server rejected the request — the message below says which field";
    case 401:
      return "not authenticated for this target";
    case 403:
      return (
        "you are not a member of this workspace with the standing this needs — the lease routes " +
        "need a workspace admin; staff act inside a tenant through `oxyc assume`"
      );
    case 404:
      return "no such workspace, run or range here — or one your credential cannot see";
    case 409:
      return "the pipeline is busy: another run of it holds its single-flight lease";
    case 503:
      return (
        "the deployment cannot serve this pipeline yet (its compiled revision does not have it, " +
        "or the compile boundary blipped) — retryable; a compile may already be queued"
      );
    default:
      return undefined;
  }
}

const AIRWAY = { refusal: airwayRefusal as RefusalReason, routesArea: "agentic-airway" } as const;

/** A 2xx JSON body from an `/agentic-airway` route, or the error its status deserves. */
export function airwayCall<T>(
  ctx: Context,
  method: string,
  suffix: string,
  body?: unknown
): Promise<T> {
  return call<T>(ctx, method, airwayPath(ctx, suffix), { ...AIRWAY, body });
}

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
