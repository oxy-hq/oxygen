/**
 * `oxyc evals run [test-file]` and `oxyc evals wait <run-id>` — LLM-judged
 * evals over `.test.yml`, run on the deployment: `oxy test`'s counterpart
 * (`internal-docs/oxy-cli-to-oxyc.md`, step 12), which still runs a checkout.
 *
 *   POST /api/{workspace_id}/evals/runs           { test_file?, case?, tag?, regrade? }
 *   GET  /api/{workspace_id}/evals/runs/{run_id}  status, accuracies, results_json
 *
 * THE BEHAVIOUR CHANGE FROM `oxy test`: the server queues the eval for its
 * worker fleet and runs the test files and agents the workspace has COMPILED
 * (its promoted revision), not a checkout on this machine. A test file is a
 * compiled kind, so a new or edited one runs once it is pushed.
 *
 * THE RESULTS FILE IS THE SERVER'S TEXT, written verbatim. `results_json` is
 * the exact document `oxy test --output-json` wrote; parsing and re-encoding it
 * here would turn a `1.0` score into `1`. `--json` prints the same text.
 *
 * THE GATE IS THE ENGINE'S: `accuracies` comes from the route, which reads the
 * metric fields `oxy test --min-accuracy` read, so the selection is not
 * restated here — only the average/all comparison is, with `oxy test`'s
 * messages. Under the threshold exits 9 (`a check ran and the answer was no`),
 * where `oxy test` exited 1.
 */

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { normalizePath, substitutePlaceholders } from "../api/paths.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { printable } from "../util/printable.js";
import { sleep } from "./airway-client.js";
import { call, type RefusalReason } from "./oltp-client.js";

/** Run-row statuses that end the wait. Every one but `done` exits 1. */
export const TERMINAL = ["done", "failed", "cancelled", "timed_out"] as const;

export type ThresholdMode = "average" | "all";

export interface EvalFlags {
  case?: string;
  tag?: string;
  /** A results file whose recorded outputs are graded again. */
  regrade?: string;
  minAccuracy?: number;
  thresholdMode: ThresholdMode;
  /** Give up waiting after this many seconds (exit 7); the run goes on. */
  waitSeconds?: number;
  /** Queue it, print the run id, and return. */
  detach?: boolean;
  json: boolean;
  /** `true` writes the default path; a string names the file. */
  outputJson?: boolean | string;
}

interface CreatedRun {
  run_id: string;
  test_files: string[];
}

export interface RunView {
  run_id: string;
  status: string;
  test_files: string[];
  error: string | null;
  accuracies: number[] | null;
  results: unknown;
  results_json: string | null;
}

/** Why an `/evals/runs` route refused, in the terms the caller acts on. */
export function evalsRefusal(status: number): string | undefined {
  switch (status) {
    case 400:
      return "the server refused the request — the message below says which field or test file";
    case 401:
      return "not authenticated for this target";
    case 403:
      return "you are not a member of this workspace";
    case 404:
      return (
        "no such workspace or run here — or the test file is not in the workspace's served " +
        "revision: test files are compiled on push, so push it first"
      );
    case 503:
      return (
        "the deployment could not read its compiled revision, or the revision predates compiled " +
        "test files (a compile is queued) — retryable"
      );
    default:
      return undefined;
  }
}

const EVALS = { refusal: evalsRefusal as RefusalReason, routesArea: "evals" } as const;

function evalsPath(ctx: Context, suffix: string): string {
  return substitutePlaceholders(normalizePath(`{workspace}/evals${suffix}`), ctx.placeholders());
}

/** `oxy test`'s flag rules, checked before anything is sent. */
export function validateFlags(file: string | undefined, flags: EvalFlags): void {
  const min = flags.minAccuracy;
  if (min !== undefined && !(min >= 0 && min <= 1)) {
    throw usageError(`min-accuracy must be between 0.0 and 1.0, got: ${min}`);
  }
  if (file !== undefined && !file.endsWith(".test.yml")) {
    throw usageError(`Unsupported test file: ${file}. Evals run .test.yml files only.`);
  }
  if (flags.case !== undefined && file === undefined) {
    throw usageError("--case requires a specific file to be specified");
  }
  if (flags.regrade !== undefined) {
    if (file === undefined) {
      throw usageError("--regrade requires the test file the results came from");
    }
    if (flags.case !== undefined || flags.tag !== undefined) {
      throw usageError("--regrade cannot be combined with --case or --tag");
    }
  }
  if (flags.waitSeconds !== undefined && !(flags.waitSeconds > 0)) {
    throw usageError("--wait takes a positive number of seconds");
  }
}

/**
 * Where `--output-json` writes with no file named: `oxy test`'s rule, the test
 * file with `.yml` swapped for `.results.json`, or `test-results.json`.
 */
export function resultsPath(file: string | undefined): string {
  if (file === undefined) return "test-results.json";
  const stem = file.replace(/(\.yml)+$/, "").replace(/(\.yaml)+$/, "");
  return `${stem}.results.json`;
}

/**
 * The `--min-accuracy` gate over the route's `accuracies`, with `oxy test`'s
 * wording. `undefined` passes; a string is the failure. No accuracies at all
 * passes with a warning, as it did.
 */
export function gate(
  accuracies: number[],
  min: number,
  mode: ThresholdMode
): { failure?: string; warning?: string } {
  if (accuracies.length === 0) {
    return { warning: "--min-accuracy specified but no accuracy metrics found" };
  }
  if (mode === "average") {
    const avg = accuracies.reduce((a, b) => a + b, 0) / accuracies.length;
    return avg < min
      ? { failure: `Average accuracy ${avg.toFixed(4)} below threshold ${min.toFixed(4)}` }
      : {};
  }
  const failing = accuracies
    .map((acc, i) => ({ acc, i }))
    .filter(({ acc }) => acc < min)
    .map(({ acc, i }) => `Test ${i + 1}: ${acc.toFixed(4)}`);
  return failing.length === 0
    ? {}
    : {
        failure: `${failing.length} test(s) below threshold ${min.toFixed(4)}: ${failing.join(", ")}`
      };
}

/** `oxyc evals run`: queue the run, then (unless `--detach`) wait for it. */
export async function runEvals(
  ctx: Context,
  file: string | undefined,
  flags: EvalFlags
): Promise<void> {
  validateFlags(file, flags);
  const body: Record<string, string> = {};
  if (file !== undefined) body.test_file = file;
  if (flags.case !== undefined) body.case = flags.case;
  if (flags.tag !== undefined) body.tag = flags.tag;
  if (flags.regrade !== undefined) body.regrade = readRegrade(flags.regrade);
  const created = await call<CreatedRun>(ctx, "POST", evalsPath(ctx, "/runs"), {
    ...EVALS,
    body
  });
  if (flags.detach) {
    if (flags.json) process.stdout.write(`${JSON.stringify(created)}\n`);
    else {
      process.stdout.write(`${created.run_id}\n`);
      log.hint(`wait for it: oxyc evals wait ${created.run_id} --workspace <id>`);
    }
    return;
  }
  if (!flags.json) {
    process.stderr.write(
      `eval run ${created.run_id} queued: ${created.test_files.map(printable).join(", ")}\n`
    );
  }
  await waitAndReport(ctx, created.run_id, file, flags);
}

/** `oxyc evals wait <run-id>`: wait for a queued run and report it like `run`. */
export async function waitEvals(ctx: Context, runId: string, flags: EvalFlags): Promise<void> {
  validateFlags(undefined, { ...flags, case: undefined, regrade: undefined });
  await waitAndReport(ctx, runId, undefined, flags);
}

function readRegrade(path: string): string {
  try {
    return readFileSync(path, "utf8");
  } catch (e) {
    throw usageError(`cannot read the results file ${path}: ${(e as Error).message}`);
  }
}

async function waitAndReport(
  ctx: Context,
  runId: string,
  file: string | undefined,
  flags: EvalFlags
): Promise<void> {
  const view = await poll(ctx, runId, flags.waitSeconds);
  if (view.status !== "done") {
    throw new CliError(`eval run ${runId} ${view.status}`, {
      code: ExitCode.FAILURE,
      detail: view.error ? printable(view.error) : undefined
    });
  }
  const text = view.results_json ?? "[]\n";
  if (flags.json) process.stdout.write(text);
  else printSummary(view);
  if (flags.outputJson) {
    // A run of one file names the default after it; `wait` and a run of every
    // file use `test-results.json`, as `oxy test` with no file did.
    const named = typeof flags.outputJson === "string" ? flags.outputJson : undefined;
    const target =
      named ?? resultsPath(file ?? (view.test_files.length === 1 ? view.test_files[0] : undefined));
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, text);
    process.stderr.write(`Results written to ${target}\n`);
  }
  if (flags.minAccuracy !== undefined) {
    const verdict = gate(view.accuracies ?? [], flags.minAccuracy, flags.thresholdMode);
    if (verdict.warning) log.warn(verdict.warning);
    if (verdict.failure) {
      throw new CliError(verdict.failure, {
        code: ExitCode.CHECK_FAILED,
        hint: `the run's results: oxyc evals wait ${runId} --workspace <id> --json`
      });
    }
  }
}

const POLL_MS = () => Number(process.env.OXYC_POLL_MS) || 3000;

async function poll(ctx: Context, runId: string, waitSeconds?: number): Promise<RunView> {
  const deadline = waitSeconds === undefined ? undefined : Date.now() + waitSeconds * 1000;
  const path = evalsPath(ctx, `/runs/${encodeURIComponent(runId)}`);
  for (;;) {
    const view = await call<RunView>(ctx, "GET", path, EVALS);
    if ((TERMINAL as readonly string[]).includes(view.status)) return view;
    if (deadline !== undefined && Date.now() >= deadline) {
      throw new CliError(`eval run ${runId} did not finish within ${waitSeconds}s`, {
        code: ExitCode.UNAVAILABLE,
        hint: `it is still running: oxyc evals wait ${runId} --workspace <id>`
      });
    }
    await sleep(POLL_MS());
  }
}

interface ResultDoc {
  test_name?: string | null;
  errors?: string[];
  stats?: { total_attempted?: number; answered?: number };
  metrics?: Array<{
    type?: string;
    score?: number;
    answer_acc?: number;
    records?: Array<{ prompt?: string; score?: number; choice?: string }>;
  }>;
}

/**
 * A compact report: per test file its gated accuracy and answer rate, then
 * the prompts of the runs that did not pass. `--json` / `--output-json` carry
 * the full document (`oxy test`'s pretty report did not come across).
 */
export function summaryLines(results: unknown): string[] {
  const docs = Array.isArray(results) ? (results as ResultDoc[]) : [];
  const lines: string[] = [];
  for (const doc of docs) {
    const name = printable(doc.test_name ?? "(unnamed)");
    const answered = doc.stats?.answered ?? 0;
    const total = doc.stats?.total_attempted ?? 0;
    for (const metric of doc.metrics ?? []) {
      const acc = metric.type === "Execution" ? metric.answer_acc : metric.score;
      lines.push(
        `${name}  ${metric.type ?? "?"} ${(acc ?? 0).toFixed(4)}  answered ${answered}/${total}`
      );
      for (const record of metric.records ?? []) {
        if ((record.score ?? 0) < 1 && record.choice !== "PASS") {
          lines.push(`  ${record.choice ?? "FAIL"}  ${printable(record.prompt ?? "")}`);
        }
      }
    }
  }
  return lines;
}

function printSummary(view: RunView): void {
  for (const line of summaryLines(view.results)) {
    const failed = line.startsWith("  ");
    process.stdout.write(`${failed ? out.red(line) : line}\n`);
  }
}
