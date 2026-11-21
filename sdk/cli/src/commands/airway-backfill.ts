/**
 * `oxyc airway backfill | coverage` — the chunked, resumable backfill and what
 * it has loaded.
 *
 *   POST /api/{workspace_id}/agentic-airway/chunked-backfill   { pipeline_ref, from, to, granularity, resources }
 *   POST /api/{workspace_id}/agentic-airway/resume-backfill    { range_id }
 *   GET  /api/{workspace_id}/agentic-airway/backfill-ranges?pipeline_ref=
 *   GET  /api/{workspace_id}/agentic-airway/coverage?range_id=
 *
 * Three changes from `oxy airway backfill | coverage`, each the server's
 * semantics rather than a choice made here:
 *
 * - The drive is queued for the worker fleet, so the command returns once the
 *   range is recorded. `--wait` polls the range until it settles.
 * - Re-running the same window no longer resumes it. Each POST is a new range,
 *   as the web app's Backfill dialog makes; resuming is `--resume <range_id>`,
 *   and an unfinished range for the same window is named before a new one is
 *   made.
 * - Coverage is per range. `coverage <ref>` lists the pipeline's ranges with
 *   their tallies; `--range <id>` shows one range's missing periods, which is
 *   what `oxy airway coverage` printed for the whole pipeline.
 */

import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { table } from "../ui/render.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { airwayCall, sleep } from "./airway-client.js";

/** `agentic_pipeline::backfill::BackfillRangeInfo`. */
export interface BackfillRange {
  id: string;
  pipeline_ref: string;
  requested_from: string;
  requested_to: string;
  granularity: string;
  concurrency: number;
  created_by: string | null;
  /** `pending` | `running` | `done` | `failed` | `degraded`. */
  status: string;
  created_at: string;
  updated_at: string;
  total: number;
  done: number;
  missing: number;
}

/** `agentic_pipeline::backfill::CoverageChunk`. */
interface CoverageChunk {
  period_start: string;
  period_end: string;
  status: string;
  run_id: string | null;
  row_count: number | null;
  attempts: number;
  error: string | null;
}

/** `agentic_pipeline::backfill::CoverageReport`. */
export interface CoverageReport {
  pipeline_ref: string;
  range_id: string | null;
  chunks: CoverageChunk[];
  summary: {
    total: number;
    done: number;
    loaded_from: string | null;
    loaded_to: string | null;
    missing: number;
  };
}

/** `ChunkedBackfillResponse`. */
interface Queued {
  range_id: string;
  chunk_count: number;
}

const GRANULARITIES = ["month", "week", "day"];
/** Range statuses nothing will change without a new drive. */
export const SETTLED = ["done", "failed", "degraded"];

const day = (iso: string | null) => (iso ? iso.slice(0, 10) : "?");

function instant(raw: string | undefined, flag: string): string {
  if (!raw) throw usageError(`--${flag} is required (RFC3339, e.g. 2026-01-01T00:00:00Z)`);
  const ms = Date.parse(raw);
  if (Number.isNaN(ms) || !/^\d{4}-\d{2}-\d{2}T/.test(raw)) {
    throw usageError(`invalid --${flag} \`${raw}\``, "RFC3339, e.g. 2026-01-01T00:00:00Z");
  }
  return new Date(ms).toISOString();
}

export interface BackfillFlags {
  from?: string;
  to?: string;
  granularity?: string;
  resources?: string[];
  resume?: string;
  waitSeconds?: number;
  pollMs?: number;
  json: boolean;
}

export async function runAirwayBackfill(
  ctx: Context,
  pipelineRef: string | undefined,
  flags: BackfillFlags
): Promise<void> {
  // EVERY USAGE ERROR BEFORE A REQUEST.
  if (flags.waitSeconds !== undefined && !(flags.waitSeconds > 0)) {
    throw usageError("--wait must be a positive number of seconds");
  }
  let queued: Queued;
  let label: string;
  let baseline: Baseline | undefined;
  if (flags.resume) {
    if (flags.from || flags.to) {
      throw usageError("--resume takes the range's own window; drop --from / --to");
    }
    // The resume only queues: until a worker re-drives them, the chunks keep
    // their old `failed` status, which reads as settled. Remember each
    // not-done chunk's attempts so the wait can tell a re-drive from the past.
    if (flags.waitSeconds !== undefined)
      baseline = snapshot(await fetchCoverage(ctx, flags.resume));
    queued = await airwayCall<Queued>(ctx, "POST", "/resume-backfill", { range_id: flags.resume });
    label = `resuming range ${queued.range_id}: ${queued.chunk_count} chunk(s) not done`;
  } else {
    const ref = pipelineRef?.trim();
    if (!ref) throw usageError("name the pipeline: oxyc airway backfill <ref> --from … --to …");
    const granularity = flags.granularity ?? "month";
    if (!GRANULARITIES.includes(granularity)) {
      throw usageError(`invalid --granularity \`${granularity}\` (expected month|week|day)`);
    }
    const from = instant(flags.from, "from");
    const to = instant(flags.to, "to");
    if (Date.parse(from) >= Date.parse(to)) {
      throw usageError("empty window — --from must be before --to");
    }
    await warnUnfinished(ctx, ref, from, to, granularity);
    queued = await airwayCall<Queued>(ctx, "POST", "/chunked-backfill", {
      pipeline_ref: ref,
      from,
      to,
      granularity,
      resources: flags.resources ?? []
    });
    label = `backfill ${ref}: range ${queued.range_id}, ${queued.chunk_count} ${granularity} chunk(s) queued`;
  }

  if (flags.waitSeconds === undefined) {
    if (flags.json) {
      process.stdout.write(`${JSON.stringify(queued)}\n`);
      return;
    }
    process.stdout.write(`${label}\n`);
    log.hint(`progress: oxyc airway coverage --range ${queued.range_id}`);
    return;
  }
  if (!flags.json) process.stderr.write(`${label}\n`);
  const report = await waitForRange(ctx, queued.range_id, {
    waitSeconds: flags.waitSeconds,
    pollMs: flags.pollMs ?? 5000,
    // A fresh range's checkpoints are written as the drive reaches them, so
    // it is not settled until all of them exist.
    expectedTotal: baseline ? 0 : queued.chunk_count,
    baseline
  });
  printCoverage(report, flags.json);
  const failed = report.chunks.filter((c) => c.status !== "done").length;
  if (failed > 0) {
    throw new CliError(`backfill range ${queued.range_id}: ${failed} chunk(s) not done`, {
      code: ExitCode.FAILURE,
      hint: `re-drive the failed and partial chunks: oxyc airway backfill --resume ${queued.range_id}`
    });
  }
}

/**
 * Name an unfinished range for this exact window. Not a refusal: the old
 * command resumed it silently, and the server cannot match on the resource
 * scope (it is not in the listing), so the operator decides.
 */
async function warnUnfinished(
  ctx: Context,
  ref: string,
  from: string,
  to: string,
  granularity: string
): Promise<void> {
  const ranges = await listRanges(ctx, ref);
  const same = ranges.find(
    (r) =>
      r.granularity === granularity &&
      Date.parse(r.requested_from) === Date.parse(from) &&
      Date.parse(r.requested_to) === Date.parse(to) &&
      r.missing > 0
  );
  if (same) {
    log.warn(
      `range ${same.id} already covers this window with ${same.missing} chunk(s) not done — ` +
        `\`oxyc airway backfill --resume ${same.id}\` re-drives only those; starting a new range`
    );
  }
}

async function listRanges(ctx: Context, ref: string): Promise<BackfillRange[]> {
  const suffix = `/backfill-ranges?pipeline_ref=${encodeURIComponent(ref)}`;
  return airwayCall<BackfillRange[]>(ctx, "GET", suffix);
}

async function fetchCoverage(ctx: Context, rangeId: string): Promise<CoverageReport> {
  return airwayCall<CoverageReport>(
    ctx,
    "GET",
    `/coverage?range_id=${encodeURIComponent(rangeId)}`
  );
}

/** Each not-done chunk's attempts before a resume, keyed by its period start. */
export type Baseline = Map<string, number>;

export function snapshot(report: CoverageReport): Baseline {
  return new Map(
    report.chunks.filter((c) => c.status !== "done").map((c) => [c.period_start, c.attempts])
  );
}

/**
 * Settled: every expected chunk exists, none is pending or running, and — after
 * a resume — every chunk that was not done is done now or was attempted again.
 */
export function settled(report: CoverageReport, expectedTotal = 0, baseline?: Baseline): boolean {
  if (report.chunks.length === 0 || report.summary.total < expectedTotal) return false;
  if (report.chunks.some((c) => c.status === "pending" || c.status === "running")) return false;
  if (!baseline) return true;
  return report.chunks.every((c) => {
    const before = baseline.get(c.period_start);
    return before === undefined || c.status === "done" || c.attempts > before;
  });
}

async function waitForRange(
  ctx: Context,
  rangeId: string,
  opts: { waitSeconds: number; pollMs: number; expectedTotal: number; baseline?: Baseline }
): Promise<CoverageReport> {
  const { waitSeconds, pollMs } = opts;
  const deadline = Date.now() + waitSeconds * 1000;
  for (;;) {
    const report = await fetchCoverage(ctx, rangeId);
    if (settled(report, opts.expectedTotal, opts.baseline)) return report;
    if (Date.now() >= deadline) {
      throw new CliError(`backfill range ${rangeId} did not settle within ${waitSeconds}s`, {
        code: ExitCode.UNAVAILABLE,
        detail:
          `${report.summary.done}/${report.summary.total} chunks done. The drive carries on on the ` +
          "worker fleet. A chunk left pending with nothing running was deferred: another run held " +
          "the pipeline's lease (`oxyc airway leases`).",
        hint: `check it later: oxyc airway coverage --range ${rangeId}`
      });
    }
    await sleep(pollMs);
  }
}

export async function runAirwayCoverage(
  ctx: Context,
  pipelineRef: string | undefined,
  flags: { range?: string; json: boolean }
): Promise<void> {
  if (flags.range) {
    printCoverage(await fetchCoverage(ctx, flags.range), flags.json);
    return;
  }
  const ref = pipelineRef?.trim();
  if (!ref) {
    throw usageError("name the pipeline, or one range: oxyc airway coverage <ref> | --range <id>");
  }
  const ranges = await listRanges(ctx, ref);
  if (flags.json) {
    process.stdout.write(`${JSON.stringify(ranges, null, 2)}\n`);
    return;
  }
  if (ranges.length === 0) {
    process.stdout.write(`no backfill ranges for \`${ref}\`\n`);
    return;
  }
  process.stdout.write(
    `${table(ranges, [
      { header: "RANGE", value: (r) => r.id },
      { header: "WINDOW", value: (r) => `${day(r.requested_from)} → ${day(r.requested_to)}` },
      { header: "GRAIN", value: (r) => r.granularity },
      { header: "STATUS", value: (r) => r.status },
      { header: "DONE", value: (r) => `${r.done}/${r.total}` },
      { header: "MISSING", value: (r) => String(r.missing) }
    ])}\n`
  );
  log.hint("one range's missing periods: oxyc airway coverage --range <RANGE>");
}

/** One range's report, as `oxy airway coverage` printed a pipeline's. */
function printCoverage(report: CoverageReport, json: boolean): void {
  if (json) {
    process.stdout.write(`${JSON.stringify(report)}\n`);
    return;
  }
  if (report.chunks.length === 0) {
    process.stdout.write(`no backfill checkpoints for range ${report.range_id ?? "?"}\n`);
    return;
  }
  const { summary } = report;
  const lines = [`Coverage ${report.pipeline_ref} — ${summary.done}/${summary.total} chunks done`];
  if (summary.loaded_from && summary.loaded_to) {
    lines.push(`  loaded range: ${day(summary.loaded_from)} → ${day(summary.loaded_to)}`);
  }
  const missing = report.chunks.filter((c) => c.status !== "done");
  if (missing.length === 0) {
    lines.push(`  ${out.green("no missing periods.")}`);
  } else {
    lines.push(`  ${missing.length} period(s) not done:`);
    for (const c of missing) {
      const mark = ["failed", "cancelled", "timed_out"].includes(c.status)
        ? "✗"
        : c.status === "completed_with_errors"
          ? "⚠"
          : "•";
      const note = c.error ? `: ${c.error}` : "";
      lines.push(
        `    ${mark} ${day(c.period_start)} → ${day(c.period_end)} [${c.status}] (attempts ${c.attempts})${note}`
      );
    }
  }
  process.stdout.write(`${lines.join("\n")}\n`);
}
