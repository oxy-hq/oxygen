/**
 * `oxyc experiments` — an experiment's whole lifecycle from a terminal.
 *
 * Experiments are records, not files: a draft design edited in place, priced,
 * registered (which freezes it and makes it the run), watched, aborted or
 * reported. Each verb is a thin client over one route:
 *
 *   list                         GET  /experiments
 *   design <name>                GET  /experiments/{name}/design          (as YAML)
 *   create --file                POST /experiments                        {config}
 *   edit <name> --file           PUT  /experiments/{name}/design          {config, base_hash}
 *   duplicate <name> <new>       POST /experiments/{name}/duplicate       {name}
 *   price <name> --candidate…    POST /experiments/{name}/price           {candidates}
 *   proposals <name>             GET  /experiments/{name}/proposals
 *   use <name> <proposal>        POST /experiments/{name}/proposals/{id}/use
 *   register <name>              POST /experiments/{name}/register        {content_hash}
 *   retrospective <name>         POST /experiments/{name}/retrospective   {content_hash}
 *   status <name>                GET  /experiments/{name}/run
 *   abort <name> --reason        POST /experiments/{name}/abort           {reason, levers}
 *   result <name>                GET  /experiments/{name}/result
 *   interim <name> [--refresh]   GET (POST) /experiments/{name}/interim
 *   write-back <name>            POST /experiments/{name}/write-back/apply
 *
 * Register and retrospective send the hash of the draft as it was just read,
 * so a draft someone edited in between is a 409 rather than registered unseen.
 */

import { stringify as toYaml } from "yaml";
import type { Context } from "../context/resolve.js";
import { table } from "../ui/render.js";
import { usageError } from "../util/errors.js";
import {
  call,
  experimentError,
  experimentPath,
  readCandidates,
  readConfig,
  send
} from "./experiments-client.js";
import { printInterim, printResult, printRunStatus } from "./experiments-render.js";
import { confirm } from "./oltp-client.js";

interface Design {
  id: string;
  content_hash: string;
  frozen_at: string | null;
  config: Record<string, unknown>;
}

interface ListItem {
  name: string;
  status: string;
  owner?: string;
  proposals_count?: number;
  next_switch_on?: string | null;
  registration_refusal?: string | null;
}

export interface Proposal {
  id: string;
  request_id: string;
  status: string;
  request?: { shape?: string };
  mde_unit_day?: number | null;
  mde_relative?: number | null;
  alpha?: number;
  refusal?: string | null;
  warnings?: string[];
  stale?: boolean;
}

interface Registered {
  run_id: string;
  design_id: string;
  registered_at: string;
}

function emit(value: unknown): void {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}
function say(line: string): void {
  process.stdout.write(`${line}\n`);
}

export async function runExperimentsList(ctx: Context, opts: { json: boolean }): Promise<void> {
  const items = await call<ListItem[]>(ctx, "GET", experimentPath(ctx));
  if (opts.json) {
    emit(items);
    return;
  }
  if (items.length === 0) {
    process.stderr.write("no experiments in this workspace\n");
    return;
  }
  process.stdout.write(
    `${table(items, [
      { header: "name", value: (i) => i.name },
      { header: "status", value: (i) => i.status },
      { header: "owner", value: (i) => i.owner ?? "" },
      { header: "proposals", value: (i) => String(i.proposals_count ?? 0), align: "right" },
      { header: "next switch", value: (i) => i.next_switch_on ?? "" },
      { header: "note", value: (i) => i.registration_refusal ?? "" }
    ])}\n`
  );
}

async function readDesign(ctx: Context, name: string): Promise<Design> {
  return call<Design>(ctx, "GET", experimentPath(ctx, name, "design"));
}

function reportDesign(verb: string, name: string, design: Design, json: boolean): void {
  if (json) {
    emit(design);
    return;
  }
  say(`${verb} ${name} — design ${design.id}, hash ${design.content_hash}`);
}

/** The config as YAML (or JSON) — the file `create` / `edit` read back. */
export async function runExperimentsDesign(
  ctx: Context,
  name: string,
  opts: { json: boolean; full: boolean }
): Promise<void> {
  const design = await readDesign(ctx, name);
  if (opts.full) {
    emit(design);
    return;
  }
  if (opts.json) {
    emit(design.config);
    return;
  }
  process.stdout.write(toYaml(design.config));
  process.stderr.write(
    `${design.frozen_at ? `frozen at ${design.frozen_at} — duplicate it to change it` : "draft"}, hash ${design.content_hash}\n`
  );
}

export async function runExperimentsCreate(
  ctx: Context,
  opts: { file?: string; json: boolean }
): Promise<void> {
  const config = readConfig(opts.file);
  const design = await call<Design>(ctx, "POST", experimentPath(ctx), { config });
  const name = typeof config.experiment === "string" ? config.experiment : "";
  reportDesign("created", name, design, opts.json);
}

export async function runExperimentsEdit(
  ctx: Context,
  name: string,
  opts: { file?: string; baseHash?: string; json: boolean }
): Promise<void> {
  const config = readConfig(opts.file);
  const baseHash = opts.baseHash ?? (await readDesign(ctx, name)).content_hash;
  const path = experimentPath(ctx, name, "design");
  const design = await call<Design>(ctx, "PUT", path, { config, base_hash: baseHash });
  reportDesign("updated", name, design, opts.json);
}

export async function runExperimentsDuplicate(
  ctx: Context,
  name: string,
  newName: string,
  opts: { json: boolean }
): Promise<void> {
  const path = experimentPath(ctx, name, "duplicate");
  const design = await call<Design>(ctx, "POST", path, { name: newName });
  reportDesign(`duplicated ${name} as`, newName, design, opts.json);
}

function pct(n: number | null | undefined): string {
  return typeof n === "number" ? `${(n * 100).toFixed(1)}%` : "";
}

function proposalNote(p: Proposal): string {
  if (p.refusal) return p.refusal;
  if (p.stale) return "stale: the draft changed since";
  return (p.warnings ?? []).join("; ");
}

export function printProposals(rows: Proposal[]): void {
  if (rows.length === 0) {
    process.stderr.write("no proposals yet — price the draft first\n");
    return;
  }
  process.stdout.write(
    `${table(rows, [
      { header: "id", value: (p) => p.id },
      { header: "status", value: (p) => p.status },
      { header: "shape", value: (p) => p.request?.shape ?? "" },
      { header: "mde/unit-day", value: (p) => p.mde_unit_day?.toFixed(2) ?? "", align: "right" },
      { header: "mde %", value: (p) => pct(p.mde_relative), align: "right" },
      {
        header: "alpha",
        value: (p) => (p.alpha === undefined ? "" : String(p.alpha)),
        align: "right"
      },
      { header: "note", value: proposalNote }
    ])}\n`
  );
}

export async function runExperimentsProposals(
  ctx: Context,
  name: string,
  opts: { json: boolean }
): Promise<void> {
  const rows = await call<Proposal[]>(ctx, "GET", experimentPath(ctx, name, "proposals"));
  if (opts.json) {
    emit(rows);
    return;
  }
  printProposals(rows);
}

const POLL_MS = () => Number(process.env.OXYC_POLL_MS) || 3000;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Poll until none of `ids` is `pending`; returns just those rows. */
async function settle(
  ctx: Context,
  name: string,
  ids: string[],
  timeoutMs: number
): Promise<Proposal[]> {
  const deadline = Date.now() + timeoutMs;
  const wanted = new Set(ids);
  for (;;) {
    const all = await call<Proposal[]>(ctx, "GET", experimentPath(ctx, name, "proposals"));
    const mine = all.filter((p) => wanted.has(p.id));
    if (mine.every((p) => p.status !== "pending")) return mine;
    if (Date.now() > deadline) {
      process.stderr.write("still pricing — read them later with `oxyc experiments proposals`\n");
      return mine;
    }
    await sleep(POLL_MS());
  }
}

export async function runExperimentsPrice(
  ctx: Context,
  name: string,
  opts: { candidate: string[]; wait: boolean; timeoutSec: number; json: boolean }
): Promise<void> {
  const candidates = readCandidates(opts.candidate);
  const started = await call<{ request_id: string; proposal_ids: string[] }>(
    ctx,
    "POST",
    experimentPath(ctx, name, "price"),
    { candidates }
  );
  if (!opts.wait) {
    if (opts.json) {
      emit(started);
      return;
    }
    say(`pricing ${started.proposal_ids.length} candidate(s), request ${started.request_id}`);
    for (const id of started.proposal_ids) say(`  ${id}`);
    process.stderr.write(`results: oxyc experiments proposals ${name}  (or re-run with --wait)\n`);
    return;
  }
  const rows = await settle(ctx, name, started.proposal_ids, opts.timeoutSec * 1000);
  if (opts.json) {
    emit(rows);
    return;
  }
  printProposals(rows);
  if (rows.some((p) => p.status === "done")) {
    process.stderr.write(`adopt one: oxyc experiments use ${name} <id>\n`);
  }
}

export async function runExperimentsUse(
  ctx: Context,
  name: string,
  proposalId: string,
  opts: { json: boolean }
): Promise<void> {
  const path = experimentPath(ctx, name, `proposals/${encodeURIComponent(proposalId)}/use`);
  const design = await call<Design>(ctx, "POST", path);
  reportDesign(`adopted ${proposalId} onto`, name, design, opts.json);
}

/** Register or analyse retrospectively: both freeze the draft as read, and take the one run. */
export async function runExperimentsRegister(
  ctx: Context,
  name: string,
  opts: { retrospective: boolean; yes: boolean; json: boolean }
): Promise<void> {
  const verb = opts.retrospective ? "retrospective" : "register";
  // Read first, so the confirmation can name the exact draft that will freeze.
  const draft = await readDesign(ctx, name);
  if (!opts.yes) {
    await confirm(
      `${opts.retrospective ? "Analyse past data for" : "Register"} ${name} (draft ${draft.content_hash})? The design freezes and cannot be edited after.`,
      {
        verb,
        why: opts.retrospective
          ? "a retrospective run freezes the design and is the experiment's only run"
          : "registering freezes the design and schedules its lever flips",
        declined: `${verb} declined — nothing was sent`
      }
    );
  }
  const run = await call<Registered>(ctx, "POST", experimentPath(ctx, name, verb), {
    content_hash: draft.content_hash
  });
  if (opts.json) {
    emit(run);
    return;
  }
  say(
    `${opts.retrospective ? "retrospective run" : "registered"} ${name}: run ${run.run_id} at ${run.registered_at}`
  );
  process.stderr.write(`watch it: oxyc experiments status ${name}\n`);
}

export async function runExperimentsStatus(
  ctx: Context,
  name: string,
  opts: { json: boolean }
): Promise<void> {
  const status = await call<Record<string, unknown>>(ctx, "GET", experimentPath(ctx, name, "run"));
  if (opts.json) {
    emit(status);
    return;
  }
  printRunStatus(status);
}

export async function runExperimentsAbort(
  ctx: Context,
  name: string,
  opts: { reason?: string; levers: string; yes: boolean; json: boolean }
): Promise<void> {
  const reason = opts.reason?.trim();
  if (!reason)
    throw usageError("abort needs --reason <text>", "it is what the experiment's owner is told");
  if (opts.levers !== "off" && opts.levers !== "keep") {
    throw usageError(
      `--levers ${opts.levers} is not off or keep`,
      "off turns treated units' levers off; keep leaves them"
    );
  }
  if (!opts.yes) {
    await confirm(
      `Abort ${name}${opts.levers === "off" ? " and turn its levers off" : ", keeping levers as they are"}?`,
      {
        verb: "abort",
        why: "an aborted run cannot be resumed; its report is labelled ABORTED",
        declined: "abort declined — nothing was sent"
      }
    );
  }
  const status = await call<Record<string, unknown>>(
    ctx,
    "POST",
    experimentPath(ctx, name, "abort"),
    {
      reason,
      levers: opts.levers
    }
  );
  if (opts.json) {
    emit(status);
    return;
  }
  say(`abort requested for ${name}; the next tick actuates it`);
  printRunStatus(status);
}

export async function runExperimentsResult(
  ctx: Context,
  name: string,
  opts: { json: boolean }
): Promise<void> {
  const path = experimentPath(ctx, name, "result");
  const response = await send(ctx, "GET", path);
  // The two 409s are states, not failures: a live run is sealed by design.
  if (response.status === 409) {
    const body = JSON.parse(response.body) as {
      sealed?: boolean;
      pending?: boolean;
      state?: string;
    };
    if (opts.json) {
      emit(body);
      return;
    }
    if (body.pending) say(`${name} is ${body.state}; its report is being prepared`);
    else {
      say(`${name} is sealed (run is ${body.state}): no effect number until it is reported`);
      if (body.state === "live" || body.state === "closing") {
        process.stdout.write(`a peek, not the result: oxyc experiments interim ${name}\n`);
      }
    }
    return;
  }
  if (response.status < 200 || response.status >= 300) throw experimentError("GET", path, response);
  const result = JSON.parse(response.body) as Record<string, unknown>;
  if (opts.json) {
    emit(result);
    return;
  }
  printResult(result);
}

export async function runExperimentsWriteBack(
  ctx: Context,
  name: string,
  opts: { json: boolean }
): Promise<void> {
  const path = experimentPath(ctx, name, "write-back/apply");
  const applied = await call<{ branch: string; file_path: string }>(ctx, "POST", path);
  if (opts.json) {
    emit(applied);
    return;
  }
  say(`write-back opened branch ${applied.branch}, editing ${applied.file_path}`);
}

interface Interim {
  state?: string;
  running?: boolean;
  in_progress?: boolean;
}

/**
 * The interim reading of a live or closing run. `--refresh` asks for a fresh one (one
 * at a time per run; it reads the warehouse on a worker), `--wait` polls until it lands.
 * A run not in progress answers 409 — a state, like `result`'s sealed one.
 */
export async function runExperimentsInterim(
  ctx: Context,
  name: string,
  opts: { refresh: boolean; wait: boolean; timeoutSec: number; json: boolean }
): Promise<void> {
  const path = experimentPath(ctx, name, "interim");
  let method = opts.refresh ? "POST" : "GET";
  let response = await send(ctx, method, path);
  const deadline = Date.now() + opts.timeoutSec * 1000;
  while (opts.refresh && opts.wait && response.status < 300) {
    const body = JSON.parse(response.body) as Interim;
    if (!body.running) break;
    if (Date.now() > deadline) {
      process.stderr.write(
        `still reading — check later with \`oxyc experiments interim ${name}\`\n`
      );
      break;
    }
    await sleep(POLL_MS());
    method = "GET";
    response = await send(ctx, method, path);
  }
  if (response.status === 409) {
    const body = JSON.parse(response.body) as Interim;
    if (opts.json) {
      emit(body);
      return;
    }
    say(`${name} is ${body.state}: an interim reads only a live or closing run`);
    if (body.state === "reported" || body.state === "aborted") {
      say(`its report is the reading: oxyc experiments result ${name}`);
    }
    return;
  }
  if (response.status < 200 || response.status >= 300) {
    throw experimentError(method, path, response);
  }
  const interim = JSON.parse(response.body) as Record<string, unknown>;
  if (opts.json) {
    emit(interim);
    return;
  }
  printInterim(interim);
}
