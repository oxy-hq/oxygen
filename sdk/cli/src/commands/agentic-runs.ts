/**
 * `oxyc agentic list | inspect | status` — read a workspace's agentic runs,
 * over the Orchestrator Dashboard's reads:
 *
 *   GET /api/{workspace}/analytics/coordinator/runs?limit=<n>      list, status
 *   GET /api/{workspace}/analytics/coordinator/active-runs          status --active
 *   GET /api/{workspace}/analytics/coordinator/runs/{id}/tree       inspect, status
 *
 * They are Oxygen Nexus routes: an org owner or admin, staff, or a member Oxy
 * granted the workspace; anyone else is `403 factory_access_required` (exit 4).
 *
 * `--json` keeps the Rust command's keys where the routes carry them. What the
 * routes do not carry, and so is gone: `inspect`'s `metadata`, `event_count`
 * and `extension.{thinking_mode, has_spec_hint}`, and `status`'s
 * `event_count`. Added: `agent_id`, `task_status`, `attempt`. Timestamps are
 * RFC 3339 (`2026-11-24T10:00:00+00:00`), where Rust printed chrono's display
 * form. `status`'s `current_stage` and `delegations` are worked out here, as
 * Rust did, from the run's structural events (`state_enter`,
 * `delegation_started` / `delegation_completed`), which the tree carries.
 */

import type { Context } from "../context/resolve.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { printable } from "../util/printable.js";
import { agenticCall, runSuffix, sleep } from "./agentic-client.js";

/** A run as `coordinator/runs` and `active-runs` list it. */
interface RunEntry {
  run_id: string;
  status: string;
  question: string;
  agent_id?: string;
  source_type?: string;
  created_at: string;
}

interface TreeEvent {
  seq: number;
  event_type: string;
  payload: Record<string, unknown>;
}

/** One node of `coordinator/runs/{id}/tree`. */
export interface TreeNode extends RunEntry {
  parent_run_id?: string | null;
  answer?: string | null;
  error_message?: string | null;
  attempt?: number;
  task_status?: string | null;
  trigger?: string | null;
  updated_at: string;
  event_log?: TreeEvent[];
  llm_usage?: unknown;
}

interface Tree {
  root_id: string;
  nodes: TreeNode[];
}

/** `""` → `null`: the routes send an empty `source_type` where Rust printed `null`. */
const orNull = (s: string | undefined | null) => (s ? s : null);
const short = (s: string, n: number) => (s.length > n ? s.slice(0, n) : s);
const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, n - 3)}...` : s);

function colorStatus(status: string): string {
  switch (status) {
    case "done":
      return out.green(status);
    case "failed":
      return out.red(status);
    case "running":
      return out.cyan(status);
    case "suspended":
      return out.yellow(status);
    default:
      return status;
  }
}

/** Pad to `width` by the text's own length, so colour codes do not skew the columns. */
function pad(text: string, plain: string, width: number): string {
  return text + " ".repeat(Math.max(1, width - plain.length));
}

// ── list ────────────────────────────────────────────────────────────────────

export async function runAgenticList(
  ctx: Context,
  flags: { limit: number; json: boolean }
): Promise<void> {
  if (!Number.isInteger(flags.limit) || flags.limit < 1 || flags.limit > 200) {
    throw usageError("--limit must be a whole number from 1 to 200");
  }
  const page = await agenticCall<{ runs: RunEntry[] }>(
    ctx,
    "GET",
    `/coordinator/runs?limit=${flags.limit}`
  );
  const runs = page.runs ?? [];
  if (flags.json) {
    const items = runs.map((r) => ({
      run_id: r.run_id,
      status: r.status,
      source_type: orNull(r.source_type),
      question: r.question,
      created_at: r.created_at
    }));
    process.stdout.write(`${JSON.stringify(items, null, 2)}\n`);
    return;
  }
  if (runs.length === 0) {
    process.stdout.write("No runs found.\n");
    return;
  }
  process.stdout.write(
    `${"RUN ID".padEnd(12)} ${"STATUS".padEnd(10)} ${"DOMAIN".padEnd(10)} QUESTION\n`
  );
  process.stdout.write(`${"─".repeat(70)}\n`);
  for (const r of runs) {
    const source = printable(r.source_type || "?");
    const line = `${short(r.run_id, 10).padEnd(12)} ${pad(colorStatus(r.status), r.status, 11)}${source.padEnd(10)} ${clip(printable(r.question), 40)}`;
    process.stdout.write(`${line}\n`);
  }
}

// ── inspect ─────────────────────────────────────────────────────────────────

export async function fetchTree(ctx: Context, runId: string): Promise<TreeNode> {
  const tree = await agenticCall<Tree>(ctx, "GET", `/coordinator${runSuffix(runId, "/tree")}`);
  const root = tree.nodes?.find((n) => n.run_id === tree.root_id) ?? tree.nodes?.[0];
  if (!root) {
    throw new CliError(`the deployment returned an empty tree for run ${runId}`, {
      code: ExitCode.FAILURE
    });
  }
  return root;
}

/** `inspect --json`: the Rust keys the tree carries, plus what it adds. */
export function inspectJson(root: TreeNode): Record<string, unknown> {
  return {
    run_id: root.run_id,
    status: root.status,
    source_type: orNull(root.source_type),
    question: root.question,
    answer: root.answer ?? null,
    error_message: root.error_message ?? null,
    created_at: root.created_at,
    updated_at: root.updated_at,
    agent_id: root.agent_id ?? null,
    task_status: root.task_status ?? null,
    attempt: root.attempt ?? 0,
    trigger: root.trigger ?? null,
    ...(root.llm_usage !== undefined ? { llm_usage: root.llm_usage } : {})
  };
}

export async function runAgenticInspect(
  ctx: Context,
  runId: string,
  flags: { json: boolean }
): Promise<void> {
  if (!runId.trim()) throw usageError("name the run: oxyc agentic inspect <run_id>");
  const root = await fetchTree(ctx, runId);
  if (flags.json) {
    process.stdout.write(`${JSON.stringify(inspectJson(root), null, 2)}\n`);
    return;
  }
  const rows: Array<[string, string | null | undefined]> = [
    ["Run ID", root.run_id],
    ["Status", colorStatus(root.status)],
    ["Domain", root.source_type || "?"],
    ["Agent", root.agent_id],
    ["Question", printable(root.question)],
    ["Answer", root.answer ? clip(printable(root.answer), 80) : undefined],
    ["Error", root.error_message ? out.red(printable(root.error_message)) : undefined],
    ["Attempt", root.attempt !== undefined ? String(root.attempt) : undefined],
    ["Created", root.created_at],
    ["Updated", root.updated_at]
  ];
  process.stdout.write(`Run Details\n${"─".repeat(50)}\n`);
  for (const [label, value] of rows) {
    if (value) process.stdout.write(`  ${`${label}:`.padEnd(13)}${value}\n`);
  }
}

// ── status ──────────────────────────────────────────────────────────────────

/** The run's last `state_enter`, as Rust's `get_last_stage` found it. */
export function currentStage(events: TreeEvent[]): string {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (e?.event_type === "state_enter" && typeof e.payload?.state === "string") {
      return e.payload.state;
    }
  }
  return "—";
}

/** Rust's `get_delegation_info`, over the same two events. */
export function delegationInfo(events: TreeEvent[]): string {
  let started = 0;
  let completed = 0;
  let active: string | undefined;
  for (const e of events) {
    if (e.event_type === "delegation_started") {
      started += 1;
      active = typeof e.payload?.target === "string" ? e.payload.target : undefined;
    } else if (e.event_type === "delegation_completed") {
      completed += 1;
      active = undefined;
    }
  }
  if (started === 0) return "none";
  const counts = `${started} started, ${completed} done`;
  return active ? `${counts}, active: ${active}` : counts;
}

export interface StatusRow {
  run_id: string;
  status: string;
  task_status: string | null;
  source_type: string | null;
  question: string;
  current_stage: string;
  delegations: string;
  created_at: string;
}

/**
 * What `status` costs the deployment. Each run's stage and delegations come
 * from its tree, and a tree read is not cheap (it loads the run's events), so
 * at most {@link STATUS_EXPANDED} runs are expanded, {@link TREE_READS_IN_FLIGHT}
 * at a time, and `--watch` refreshes no faster than {@link MIN_WATCH_SECONDS}.
 * `--active` is unbounded on the server, so its tail keeps the list's row.
 */
export const STATUS_EXPANDED = 20;
export const TREE_READS_IN_FLIGHT = 4;
export const MIN_WATCH_SECONDS = 2;

/** `items` mapped through `fn`, at most `width` calls pending at once; order kept. */
export async function mapPool<T, R>(
  items: T[],
  width: number,
  fn: (item: T) => Promise<R>
): Promise<R[]> {
  const results: R[] = new Array(items.length);
  let next = 0;
  const worker = async () => {
    while (next < items.length) {
      const i = next++;
      results[i] = await fn(items[i] as T);
    }
  };
  await Promise.all(Array.from({ length: Math.min(width, items.length) }, worker));
  return results;
}

/** One row: the list's entry, expanded with its tree when there is one. */
function statusRow(r: RunEntry, root: TreeNode | undefined): StatusRow {
  const events = root?.event_log ?? [];
  return {
    run_id: r.run_id,
    status: root?.status ?? r.status,
    task_status: root?.task_status ?? null,
    source_type: orNull(r.source_type),
    question: r.question,
    current_stage: root ? currentStage(events) : "—",
    delegations: root ? delegationInfo(events) : "—",
    created_at: r.created_at
  };
}

async function statusRows(ctx: Context, active: boolean): Promise<StatusRow[]> {
  const suffix = active ? "/coordinator/active-runs" : `/coordinator/runs?limit=${STATUS_EXPANDED}`;
  const page = await agenticCall<{ runs: RunEntry[] }>(ctx, "GET", suffix);
  const runs = page.runs ?? [];
  const expanded = runs.slice(0, STATUS_EXPANDED);
  // A run listed a moment ago can be gone by now (retention), or its tree
  // unreadable: it keeps its row, with what the list said.
  const roots = await mapPool(expanded, TREE_READS_IN_FLIGHT, (r) =>
    fetchTree(ctx, r.run_id).catch(() => undefined)
  );
  if (runs.length > expanded.length) {
    process.stderr.write(
      `${runs.length - expanded.length} more active run(s) shown without their stage and delegations\n`
    );
  }
  return runs.map((r, i) => statusRow(r, roots[i]));
}

function printStatusTable(rows: StatusRow[]): void {
  if (rows.length === 0) {
    process.stdout.write(`${out.dim("No runs found.")}\n`);
    return;
  }
  const head = `${"RUN ID".padEnd(38)} ${"STATUS".padEnd(12)} ${"TASK STATUS".padEnd(16)} ${"DOMAIN".padEnd(12)} ${"CURRENT STAGE".padEnd(20)} DELEGATIONS`;
  process.stdout.write(`${head}\n${"─".repeat(120)}\n`);
  for (const r of rows) {
    const task = r.task_status ?? "-";
    const line = `${r.run_id.padEnd(38)} ${pad(colorStatus(r.status), r.status, 13)}${task.padEnd(16)} ${(r.source_type ?? "?").padEnd(12)} ${printable(r.current_stage).padEnd(20)} ${printable(r.delegations)}`;
    process.stdout.write(`${line}\n`);
  }
}

export async function runAgenticStatus(
  ctx: Context,
  flags: { watch: number; active: boolean; json: boolean }
): Promise<void> {
  const once = flags.watch === 0;
  if (!Number.isInteger(flags.watch) || (!once && flags.watch < MIN_WATCH_SECONDS)) {
    throw usageError(
      `--watch must be 0 (once) or a whole number of seconds from ${MIN_WATCH_SECONDS}`,
      "each refresh reads every listed run's tree on the deployment"
    );
  }
  for (;;) {
    const rows = await statusRows(ctx, flags.active);
    if (flags.json) process.stdout.write(`${JSON.stringify(rows, null, 2)}\n`);
    else printStatusTable(rows);
    if (flags.watch === 0) return;
    await sleep(flags.watch * 1000);
    if (!flags.json) process.stdout.write("\x1B[2J\x1B[1;1H");
  }
}
