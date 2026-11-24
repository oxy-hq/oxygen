/**
 * `oxyc run <file> --workspace <id>` — run a SQL file or an automation on the
 * deployment. It replaced the Rust `oxy run`, which ran both in-process against
 * a local checkout with the shell's credentials.
 *
 *   .sql                       GET  /api/{workspace_id}/databases           (which databases exist)
 *                              POST /api/{workspace_id}/sql/query            { sql, database }
 *   .automation.yml            GET  /api/{workspace_id}/agentic-automations/files/{path_b64}
 *   (.procedure.yml)           POST /api/{workspace_id}/agentic-automations/runs  { workflow_ref, variables }
 *                              GET  /api/{workspace_id}/agentic-automations/runs/{id}  until terminal
 *
 * Both are customer routes (any member of the workspace), served by any replica.
 *
 * THE BEHAVIOUR CHANGE. An automation runs the definition the deployment has
 * COMPILED for its served revision, on the worker fleet, never the file on this
 * machine: push it first, or try a branch with `oxyc preview run <branch>
 * procedure <ref>` (held writes). A SQL file's TEXT is read here and sent, but
 * it runs against the deployed workspace's database, with the workspace's
 * secrets and your identity. `--database` is required when the workspace has
 * more than one database: the client cannot see `config.yml`'s
 * `defaults.database`. The route caps a result at 10,000 rows.
 *
 * `-v KEY=VALUE` renders a SQL file's `{{ KEY }}` templates here, before
 * sending, and only when given (as in Rust). Only the plain `{{ name }}` form is
 * supported; anything else is refused (exit 2) rather than sent half-rendered.
 * For an automation `-v` becomes the run's `variables` (the Rust command
 * ignored it). `--retry`, `--retry-from` and `--dry-run` are gone: the first two
 * did nothing, and the query route has no dry run.
 */

import { existsSync, readFileSync, statSync } from "node:fs";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { parse as parseYaml } from "yaml";
import { normalizePath, substitutePlaceholders } from "../api/paths.js";
import { parseJson, request } from "../api/request.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, exitCodeForStatus, usageError } from "../util/errors.js";
import { printable } from "../util/printable.js";
import { sleep } from "./airway-client.js";

/** What kind of file `oxy run` took, by extension — `handle_run_command`'s match. */
export type RunKind = "sql" | "automation";

/** The route's row cap (`IDE_MAX_ROWS` in the server's SQL execute). */
export const SQL_ROW_CAP = 10_000;

/** Run-row statuses that end the wait: the runtime's terminal set. */
export const TERMINAL_STATUSES = ["done", "failed", "cancelled", "timed_out"] as const;

export interface RunFlags {
  database?: string;
  /** `-v KEY=VALUE`, repeatable. */
  variables: string[];
  json: boolean;
  /** Automations: print the run id and return without waiting. */
  detach: boolean;
  /** Automations: seconds to wait for a terminal status before exit 7. */
  timeoutSeconds: number;
  /** Automations: milliseconds between status reads (tests shorten it). */
  pollMs?: number;
}

/**
 * The file's kind, with `oxy run`'s messages for the two refusals. Compound
 * extensions: `x.automation.yml` / `x.procedure.yaml` are automations, any
 * other YAML is refused, `.sql` is SQL.
 */
export function classify(file: string): RunKind {
  const name = basename(file);
  if (/\.(automation|procedure)\.ya?ml$/.test(name)) return "automation";
  if (/\.ya?ml$/.test(name)) {
    throw usageError("Invalid YAML file. Must be either *.automation.yml or *.procedure.yml");
  }
  if (name.endsWith(".sql")) return "sql";
  throw usageError("Invalid file extension. Must be .automation.yml, .procedure.yml, or .sql");
}

/** `-v KEY=VALUE` pairs into a map, with `oxy run`'s message for a malformed one. */
export function parseVariablePairs(pairs: string[]): Record<string, string> {
  const vars: Record<string, string> = {};
  for (const pair of pairs) {
    const at = pair.indexOf("=");
    if (at <= 0) {
      throw usageError(
        `Invalid variable format: ${JSON.stringify(pair)}. Must be in the form of VAR=VALUE`
      );
    }
    vars[pair.slice(0, at)] = pair.slice(at + 1);
  }
  return vars;
}

const PLAIN = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*$/;

/**
 * Render a SQL template's `{{ name }}` placeholders from `vars`, as the Rust
 * command's minijinja did for that form: the value's text, unescaped. Any other
 * template syntax (`{% … %}`, `{# … #}`, filters, expressions) and any name with
 * no value is refused — the query is never sent half-rendered.
 */
export function renderSql(sql: string, vars: Record<string, string>): string {
  if (/\{%|\{#/.test(sql)) {
    throw usageError(
      "this SQL uses Jinja blocks or comments ({% … %} / {# … #}); oxyc renders only {{ name }}",
      "render it first, or run the query without -v"
    );
  }
  return sql.replace(/\{\{([\s\S]*?)\}\}/g, (_whole, inner: string) => {
    const match = PLAIN.exec(inner);
    if (!match) {
      throw usageError(
        `oxyc renders only {{ name }}, not {{${inner}}}`,
        "filters and expressions are not supported; pass the final value with -v"
      );
    }
    const name = match[1] as string;
    if (!Object.hasOwn(vars, name)) {
      throw usageError(`no value for {{ ${name} }}`, `pass it: -v ${name}=<value>`);
    }
    return vars[name] as string;
  });
}

/**
 * The workspace-relative ref the server knows a file by. A file that exists
 * here resolves against the nearest directory above it holding `config.yml`
 * (the checkout root, as `oxy run` found it); otherwise the argument is taken
 * as already relative to the workspace root.
 */
export function workspaceRef(file: string, cwd: string): string {
  const abs = resolve(cwd, file);
  if (existsSync(abs)) {
    let dir = dirname(abs);
    for (;;) {
      if (existsSync(join(dir, "config.yml"))) return relative(dir, abs).split(sep).join("/");
      const up = dirname(dir);
      if (up === dir) break;
      dir = up;
    }
  }
  if (isAbsolute(file)) {
    throw usageError(
      `${file} is not inside a checkout (no config.yml above it)`,
      "name the automation by its path in the workspace, e.g. procedures/x.automation.yml"
    );
  }
  return file.replace(/\\/g, "/").replace(/^(\.\/)+/, "");
}

function workspacePath(ctx: Context, suffix: string): string {
  return substitutePlaceholders(normalizePath(`{workspace}${suffix}`), ctx.placeholders());
}

/** One JSON request; a non-2xx becomes the error its status deserves, with the server's text. */
async function send<T>(ctx: Context, method: string, path: string, body?: unknown): Promise<T> {
  const response = await request({
    target: ctx.target(),
    path,
    method,
    bearer: await ctx.bearer(),
    body: body === undefined ? undefined : JSON.stringify(body)
  });
  if (response.status < 200 || response.status >= 300) {
    throw refusal(method, path, response.status, response.body);
  }
  return parseJson(response.body) as T;
}

/** The server's own words: `message` from a JSON body (the SQL route's), else the text. */
export function serverMessage(body: string): string {
  const text = body.trim();
  try {
    const parsed = JSON.parse(text) as { message?: unknown };
    if (parsed && typeof parsed.message === "string") return parsed.message;
  } catch {
    // plain text
  }
  return text;
}

function refusal(method: string, path: string, status: number, body: string): CliError {
  const said = serverMessage(body);
  const reason =
    status === 401 || status === 403
      ? "not signed in for this target, or not a member of this workspace"
      : status === 503
        ? "the deployment cannot serve this yet (not compiled, or the warehouse is down) — retryable"
        : undefined;
  return new CliError(said ? printable(said) : `${method} ${path} failed (${status})`, {
    code: exitCodeForStatus(status),
    detail:
      [reason, said ? `${method} ${path} answered ${status}` : undefined]
        .filter(Boolean)
        .join("\n") || undefined,
    remedy: status === 401 ? "oxyc login --env <env>" : undefined
  });
}

export async function runRun(ctx: Context, file: string, flags: RunFlags): Promise<void> {
  const kind = classify(file);
  const vars = parseVariablePairs(flags.variables);
  if (kind === "sql") return runSql(ctx, file, vars, flags);
  return runAutomation(ctx, file, vars, flags);
}

// ── SQL ────────────────────────────────────────────────────────────────────

interface DatabaseInfo {
  name: string;
}

/** The database to run against: `--database`, checked, or the workspace's only one. */
async function pickDatabase(ctx: Context, named: string | undefined): Promise<string> {
  const listed = await send<DatabaseInfo[]>(ctx, "GET", workspacePath(ctx, "/databases"));
  const names = (listed ?? []).map((d) => d.name);
  if (named !== undefined) {
    if (!names.includes(named)) {
      throw new CliError(`Database '${named}' not found in config`, {
        code: ExitCode.NOT_FOUND,
        hint: names.length ? `the workspace serves: ${names.join(", ")}` : undefined
      });
    }
    return named;
  }
  if (names.length === 1) return names[0] as string;
  throw usageError(
    "Database is required for running SQL file. Please provide the database using --database",
    names.length
      ? `the workspace serves: ${names.join(", ")} (config.yml's defaults.database is not visible to oxyc)`
      : "the workspace's served config names no database"
  );
}

async function runSql(
  ctx: Context,
  file: string,
  vars: Record<string, string>,
  flags: RunFlags
): Promise<void> {
  const path = resolve(ctx.cwd, file);
  if (!existsSync(path) || !statSync(path).isFile()) {
    throw new CliError(`File not found: ${path}`, { code: ExitCode.NOT_FOUND });
  }
  const text = readFileSync(path, "utf8");
  const sql = flags.variables.length ? renderSql(text, vars) : text;
  const database = await pickDatabase(ctx, flags.database);
  log.info(`${sql.trimEnd()}\n-- on ${database}`);
  const rows = await send<unknown>(ctx, "POST", workspacePath(ctx, "/sql/query"), {
    sql,
    database
  });
  if (flags.json) {
    process.stdout.write(`${JSON.stringify(rows)}\n`);
  } else {
    process.stdout.write(`${renderTable(Array.isArray(rows) ? (rows as unknown[][]) : [])}\n`);
  }
  const count = Array.isArray(rows) ? Math.max(rows.length - 1, 0) : 0;
  if (count >= SQL_ROW_CAP) {
    log.warn(`the result stopped at the route's ${SQL_ROW_CAP}-row cap; there may be more rows`);
  }
}

/** Rows (header first, every cell a string) as a plain box-less table. */
export function renderTable(rows: unknown[][]): string {
  if (rows.length === 0) return "(no rows)";
  const cell = (c: unknown) =>
    typeof c === "string"
      ? c
      : c == null
        ? ""
        : typeof c === "object"
          ? JSON.stringify(c)
          : `${c as number}`;
  const cells = rows.map((row) => row.map((c) => printable(cell(c))));
  const widths: number[] = [];
  for (const row of cells) {
    row.forEach((c, i) => {
      widths[i] = Math.max(widths[i] ?? 0, c.length);
    });
  }
  const line = (row: string[]) =>
    row
      .map((c, i) => c.padEnd(widths[i] ?? 0))
      .join("  ")
      .trimEnd();
  const [header, ...body] = cells;
  const rule = widths.map((w) => "-".repeat(w)).join("  ");
  return [line(header ?? []), rule, ...body.map(line)].join("\n");
}

// ── Automations ────────────────────────────────────────────────────────────

/** `agentic_pipeline::automation_run::AutomationRunSnapshot`, the fields read here. */
export interface AutomationSnapshot {
  run_id: string;
  workflow_ref: string | null;
  status: string;
  error_message: string | null;
  results: Record<string, unknown>;
}

function base64Url(text: string): string {
  return Buffer.from(text, "utf8").toString("base64url");
}

/** The task names in the definition's order, to print results the way `oxy run` did. */
export function taskOrder(content: string): string[] {
  try {
    const doc = parseYaml(content) as { tasks?: Array<{ name?: unknown }> } | null;
    return (doc?.tasks ?? []).map((t) => t?.name).filter((n): n is string => typeof n === "string");
  } catch {
    return [];
  }
}

async function runAutomation(
  ctx: Context,
  file: string,
  vars: Record<string, string>,
  flags: RunFlags
): Promise<void> {
  const ref = workspaceRef(file, ctx.cwd);
  const runs = (suffix: string) => workspacePath(ctx, `/agentic-automations${suffix}`);
  // The deployed definition, first: a ref the revision does not have is a 404
  // (exit 5) here, not a run that fails a moment later.
  const definition = await send<{ content?: string }>(ctx, "GET", runs(`/files/${base64Url(ref)}`));
  const started = await send<{ run_id?: string }>(ctx, "POST", runs("/runs"), {
    workflow_ref: ref,
    variables: flags.variables.length ? vars : undefined
  });
  const runId = started?.run_id;
  if (!runId) {
    throw new CliError("POST …/agentic-automations/runs returned no run_id", {
      code: ExitCode.FAILURE
    });
  }
  if (flags.detach) {
    process.stdout.write(flags.json ? `${JSON.stringify({ run_id: runId })}\n` : `${runId}\n`);
    return;
  }
  log.info(`automation run started: ${runId}`);
  const snapshot = await waitForRun(ctx, runs(`/runs/${encodeURIComponent(runId)}`), runId, flags);
  if (flags.json) {
    process.stdout.write(`${JSON.stringify(snapshot)}\n`);
  } else if (snapshot.status === "done") {
    printResults(basename(ref), snapshot.results ?? {}, taskOrder(definition?.content ?? ""));
  }
  if (snapshot.status !== "done") {
    throw new CliError(
      `automation run ${runId} ${snapshot.status}${snapshot.error_message ? `: ${printable(snapshot.error_message)}` : ""}`,
      { code: ExitCode.FAILURE }
    );
  }
}

/** One status read; `undefined` when it failed in a way a later read may not (5xx, network). */
async function readRun(ctx: Context, path: string): Promise<AutomationSnapshot | undefined> {
  try {
    return await send<AutomationSnapshot>(ctx, "GET", path);
  } catch (e) {
    if (e instanceof CliError && e.code === ExitCode.UNAVAILABLE) return undefined;
    throw e;
  }
}

/**
 * Poll the started run until it ends. Once a run exists, nothing here may say
 * "retry this command" (exit 7): running it again would start a SECOND run.
 * So a transient read failure is retried until the deadline, and a wait that
 * ends with the run unfinished exits 1, naming the run to read later.
 */
async function waitForRun(
  ctx: Context,
  path: string,
  runId: string,
  flags: RunFlags
): Promise<AutomationSnapshot> {
  const deadline = Date.now() + flags.timeoutSeconds * 1000;
  for (;;) {
    const snapshot = await readRun(ctx, path);
    if (snapshot && (TERMINAL_STATUSES as readonly string[]).includes(snapshot.status)) {
      return snapshot;
    }
    if (Date.now() >= deadline) {
      throw new CliError(`automation run ${runId} did not finish within ${flags.timeoutSeconds}s`, {
        code: ExitCode.FAILURE,
        detail:
          "the run carries on on the worker fleet without this command — do not re-run it, " +
          "that starts a second run",
        hint: `read it later: oxyc api '{workspace}/agentic-automations/runs/${runId}' --workspace <id>`
      });
    }
    await sleep(flags.pollMs ?? 1000);
  }
}

/** `✓ <file>`, then each task's result as indented JSON, in task order — `oxy run`'s output. */
function printResults(name: string, results: Record<string, unknown>, order: string[]): void {
  const names = [...order.filter((n) => Object.hasOwn(results, n))];
  for (const n of Object.keys(results)) if (!names.includes(n)) names.push(n);
  const lines = [out.green(`✓ ${name}`)];
  for (const task of names) {
    lines.push(`  ${task}`);
    for (const l of JSON.stringify(results[task], null, 2).split("\n")) lines.push(`    ${l}`);
  }
  process.stdout.write(`${lines.join("\n")}\n`);
}
