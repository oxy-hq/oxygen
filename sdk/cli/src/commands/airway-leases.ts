/**
 * `oxyc airway leases | release-lease` — airway's single-flight leases, the
 * answer to "why won't this pipeline start?" and its recovery.
 *
 *   GET    /api/{workspace_id}/agentic-airway/leases
 *   DELETE /api/{workspace_id}/agentic-airway/leases/{pipeline_name_b64}?run_id=<id>
 *   DELETE /api/{workspace_id}/agentic-airway/leases/{pipeline_name_b64}?force=true
 *
 * Both behind the server's `WorkspaceAdmin` guard. These were
 * `oxy airway leases | release-lease`, which needed `OXY_DATABASE_URL`.
 *
 * THE CONFIRMATION IS THE RUST COMMAND'S. It showed the holder and asked
 * `Release it? [y/N]`; with no terminal it refused; `--force` skipped the prompt
 * and released whatever held the lease. Here: the same prompt, the same refusal
 * (exit 8, the refused-destructive code), and `--force` the same. `--yes` is
 * new: it skips the prompt but keeps the delete scoped to the holder just
 * listed, the non-interactive form of answering `y`.
 */

import { request } from "../api/request.js";
import type { Context } from "../context/resolve.js";
import { table } from "../ui/render.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, exitCodeForStatus, usageError } from "../util/errors.js";
import { airwayCall, airwayPath, airwayRefusal } from "./airway-client.js";
import { confirm } from "./oltp-client.js";

/** One row of `GET …/leases` — `oxy_api_airway::leases::LeaseView`. */
export interface Lease {
  pipeline_name: string;
  run_id: string;
  acquired_at: string;
  expires_at: string;
  /** Past `expires_at` on the server's clock: it blocks nothing any more. */
  lapsed: boolean;
}

/** The pipeline name as the route's path segment: URL-safe base64, no padding. */
export function encodeName(name: string): string {
  return Buffer.from(name, "utf8").toString("base64url");
}

const when = (iso: string) => iso.replace("T", " ").replace(/(\.\d+)?(Z|\+00:00)$/, "");

export async function fetchLeases(ctx: Context): Promise<Lease[]> {
  const leases = await airwayCall<Lease[]>(ctx, "GET", "/leases");
  if (!Array.isArray(leases)) {
    throw new CliError("GET …/agentic-airway/leases did not return a list", {
      code: ExitCode.FAILURE
    });
  }
  return leases;
}

export async function runAirwayLeases(ctx: Context, json: boolean): Promise<void> {
  const leases = await fetchLeases(ctx);
  if (json) {
    process.stdout.write(`${JSON.stringify(leases, null, 2)}\n`);
    return;
  }
  if (leases.length === 0) {
    process.stdout.write(`${out.green("No airway leases held.")}\n`);
    return;
  }
  process.stdout.write(
    `${table(leases, [
      { header: "PIPELINE", value: (l) => l.pipeline_name },
      { header: "RUN_ID", value: (l) => l.run_id },
      { header: "ACQUIRED (UTC)", value: (l) => when(l.acquired_at) },
      // A lapsed row blocks nothing — the next acquire takes it over — so an
      // operator seeing one should not go reaching for release-lease.
      {
        header: "EXPIRES (UTC)",
        value: (l) => `${when(l.expires_at)}${l.lapsed ? " (LAPSED)" : ""}`
      }
    ])}\n`
  );
}

export interface ReleaseFlags {
  yes?: boolean;
  force?: boolean;
  json?: boolean;
}

/** What `DELETE …/leases/{name}` answers: `200 { outcome }` or `409 { error }`. */
interface ReleaseAnswer {
  outcome?: "released" | "already_free";
  removed?: number;
  error?: string;
  message?: string;
  holder?: Lease;
}

export async function runAirwayReleaseLease(
  ctx: Context,
  pipelineName: string,
  flags: ReleaseFlags
): Promise<void> {
  const name = pipelineName.trim();
  if (!name) throw usageError("name the pipeline: oxyc airway release-lease <pipeline-name>");

  if (flags.force) return finish(ctx, name, await release(ctx, name, "force=true"), flags);

  const held = (await fetchLeases(ctx)).find((l) => l.pipeline_name === name);
  if (!held) {
    // Not an error: "already free" is the state the caller wanted.
    if (flags.json) {
      process.stdout.write(`${JSON.stringify({ outcome: "already_free", pipeline_name: name })}\n`);
      return;
    }
    process.stdout.write(`${out.green(`No lease held for \`${name}\` — nothing to release.`)}\n`);
    return;
  }

  if (!flags.yes) {
    printHolder(held);
    await confirm("Release it?", {
      verb: "release-lease",
      why: `run ${held.run_id} holds the lease; if it is still executing, releasing lets a second run start alongside it`,
      declined: "aborted; lease left in place"
    });
  }
  const query = `run_id=${encodeURIComponent(held.run_id)}`;
  return finish(ctx, name, await release(ctx, name, query), flags, held.run_id);
}

/**
 * The holder and the risk, on stderr — stdout is the result — and not through
 * `log.info`, which `--quiet` silences: this is what the prompt asks about.
 */
function printHolder(held: Lease): void {
  process.stderr.write(
    `Lease on \`${held.pipeline_name}\` is held by run ${held.run_id} ` +
      `(acquired ${when(held.acquired_at)} UTC, expires ${when(held.expires_at)} UTC).\n` +
      "If that run is still executing, releasing lets a second run start alongside it — which " +
      "can produce duplicate rows. Only release a holder you know is dead (dead-lettered, or a " +
      "run whose worker died).\n"
  );
}

async function release(
  ctx: Context,
  name: string,
  query: string
): Promise<{ status: number; answer: ReleaseAnswer; body: string }> {
  const path = `${airwayPath(ctx, `/leases/${encodeName(name)}`)}?${query}`;
  const response = await request({
    target: ctx.target(),
    path,
    method: "DELETE",
    bearer: await ctx.bearer()
  });
  let answer: ReleaseAnswer = {};
  try {
    answer = JSON.parse(response.body) as ReleaseAnswer;
  } catch {
    // A non-JSON body is reported as the refusal below.
  }
  if ((response.status < 200 || response.status >= 300) && response.status !== 409) {
    const reason = airwayRefusal(response.status);
    const body = response.body.trim();
    throw new CliError(`DELETE ${path} failed (${response.status})`, {
      code: exitCodeForStatus(response.status),
      detail: [reason, body ? `server: ${body}` : ""].filter(Boolean).join("\n") || undefined,
      remedy: response.status === 401 ? "oxyc login --env <env>" : undefined
    });
  }
  return { status: response.status, answer, body: response.body };
}

function finish(
  ctx: Context,
  name: string,
  result: { status: number; answer: ReleaseAnswer; body: string },
  flags: ReleaseFlags,
  reviewed?: string
): void {
  const { status, answer } = result;
  if (flags.json && status !== 409) {
    process.stdout.write(`${result.body.trim()}\n`);
    return;
  }
  if (status !== 409) {
    if (answer.outcome === "already_free") {
      process.stdout.write(
        `${out.green(
          reviewed
            ? `Lease on \`${name}\` was already released by run ${reviewed}; the pipeline can run again.`
            : `No lease held for \`${name}\` — nothing to release.`
        )}\n`
      );
      return;
    }
    process.stdout.write(
      `${out.green(`Released ${answer.removed ?? 1} lease(s) for \`${name}\`; the pipeline can run again.`)}\n`
    );
    return;
  }
  throw stillHeld(ctx, name, answer, flags, reviewed);
}

/**
 * Still blocked. Both forms say leave it alone or review it again — never
 * "re-run with --force", which would point a widened delete at a run the
 * operator has never been shown. A plain re-run re-lists, shows the new holder
 * and prompts against it.
 */
function stillHeld(
  ctx: Context,
  name: string,
  answer: ReleaseAnswer,
  flags: ReleaseFlags,
  reviewed?: string
): CliError {
  const ws = ctx.placeholders().workspace;
  const recheck = `oxyc airway leases${ws ? ` --workspace ${ws}` : ""}`;
  if (flags.json) process.stdout.write(`${JSON.stringify(answer)}\n`);
  if (answer.error === "lease_reheld_by_same_run") {
    return new CliError(`lease on \`${name}\` was not released`, {
      code: ExitCode.REQUEST,
      detail: `run ${reviewed ?? "?"} released it and was retried, so it holds the lease again and is live`,
      hint: `leave it alone, or re-check with \`${recheck}\``
    });
  }
  const next = answer.holder?.run_id ?? "?";
  return new CliError(`lease on \`${name}\` was not released`, {
    code: ExitCode.REQUEST,
    detail: flags.force
      ? `run ${next} acquired it since the release was asked for. That run is live — leave it alone.`
      : `run ${reviewed ?? "?"} no longer holds it; run ${next} does now`,
    hint: flags.force
      ? `re-check with \`${recheck}\``
      : "re-run the same command to review and confirm against that run"
  });
}
