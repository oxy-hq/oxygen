/**
 * `oxyc sync` — write a workspace's `.databases/` files into the local checkout.
 *
 * It was `oxy sync`, which introspected the warehouses from the laptop with the
 * warehouse credentials in hand. Now the SERVER introspects them and renders the
 * files (`oxy::semantic::render`), so the file format stays Rust's and this
 * client never reimplements it. All it knows of the layout is what the paths in
 * the response say:
 *
 *   GET  /api/{workspace_id}/databases         the configured databases (names)
 *   POST /api/{workspace_id}/databases/sync    { database, datasets?, tables? } →
 *                                              [{ path, content }], paths under `.databases/`
 *
 * The route renders ONE database per request (each well inside the server's
 * 60s request limit), so with no database named this lists them and asks for
 * each: a database that fails is reported and the rest still sync, as `oxy sync`
 * did. A type sync cannot introspect (`airhouse_managed` and the like, 422) is
 * skipped, not failed.
 *
 * What it writes is decided here, against the checkout, because the server
 * cannot see it:
 *
 * - by default a file that exists is skipped and a new one is created;
 * - with `--overwrite` every file is written, and a `models/` directory the
 *   response names is pruned of the files the response does not carry (the
 *   database directory itself is never pruned);
 * - with `--dry-run` nothing is written; the plan is printed.
 *
 * Every path is checked before anything is written: relative, `/`-separated,
 * under `.databases/`, with no `.`/`..` segment, and resolving (symlinks
 * included) inside the checkout. One bad path refuses the whole write.
 */

import {
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  realpathSync,
  statSync,
  unlinkSync,
  writeFileSync
} from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";

import { normalizePath, substitutePlaceholders } from "../api/paths.js";
import { type ApiResponse, parseJson, request } from "../api/request.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import {
  CliError,
  ExitCode,
  type ExitCodeValue,
  exitCodeForStatus,
  usageError
} from "../util/errors.js";

/** One file a sync writes — `oxy::semantic::render::RenderedFile`. */
export interface RenderedFile {
  path: string;
  content: string;
}

export interface SyncOptions {
  database?: string;
  datasets: string[];
  tables: string[];
  overwrite: boolean;
  dryRun: boolean;
  /** The checkout root the paths are relative to (default: the working directory). */
  dir?: string;
  json: boolean;
}

/** What happened to one database. */
export interface DatabaseOutcome {
  database: string;
  status: "rendered" | "skipped" | "failed";
  /** Files the server rendered for it. */
  files?: number;
  /** Why it was skipped or failed. */
  error?: string;
  /** The exit code its failure maps to. */
  code?: ExitCodeValue;
}

export type FileAction = "create" | "overwrite" | "skip" | "delete";

export interface PlannedFile {
  path: string;
  action: FileAction;
}

/** The root every rendered path must sit under. */
export const DATABASES_DIR = ".databases";

export const syncPath = (ctx: Context) =>
  substitutePlaceholders(normalizePath("{workspace}/databases/sync"), ctx.placeholders());

const listPath = (ctx: Context) =>
  substitutePlaceholders(normalizePath("{workspace}/databases"), ctx.placeholders());

/**
 * Introspection is a warehouse round trip per dataset; give it the server's own
 * limit and a little more, so the server's 408 (not ours) is what a slow sync sees.
 */
const SYNC_TIMEOUT_MS = 90_000;

/** Why the sync route refused, in the terms a customer acts on. */
function refusalReason(status: number, database: string): string | undefined {
  switch (status) {
    case 400:
      return `'${database}' cannot connect as the deployed config.yml configures it — the server's message below says why`;
    case 401:
      return "not authenticated for this target";
    case 403:
      return "syncing needs a workspace admin; staff act inside a tenant through `oxyc assume`";
    case 404:
      return `no database '${database}' in the deployed config.yml — or no such workspace here. Push a config change before syncing it`;
    case 408:
      return "the introspection outlasted the server's request limit — narrow it with -d <dataset> or --tables";
    case 422:
      return `'${database}' is a type sync cannot introspect`;
    case 502:
      return "the warehouse failed the introspection — a rerun is safe";
    default:
      return undefined;
  }
}

/** A refusal's `message`, when the body is the route's `{ error, message }`. */
function serverMessage(response: ApiResponse): string | undefined {
  const parsed = parseJson(response.body) as { message?: unknown } | undefined;
  return typeof parsed?.message === "string" ? parsed.message : undefined;
}

/** 408 is a server-side timeout: retryable, not a malformed request. */
function exitFor(status: number): ExitCodeValue {
  return status === 408 ? ExitCode.UNAVAILABLE : exitCodeForStatus(status);
}

function syncError(database: string, response: ApiResponse): CliError {
  const message = serverMessage(response);
  const body = response.body.trim();
  return new CliError(`sync of '${database}' failed (${response.status})`, {
    code: exitFor(response.status),
    detail:
      [refusalReason(response.status, database), message ?? (body && body !== "{}" ? body : "")]
        .filter(Boolean)
        .join("\n") || undefined,
    remedy: response.status === 401 ? "oxyc login --env <env>" : undefined,
    serverMessage: message
  });
}

/** The response must be the route's array of files; anything else is a fault. */
export function parseFiles(body: string): RenderedFile[] | undefined {
  const parsed = parseJson(body);
  if (!Array.isArray(parsed)) return undefined;
  const ok = parsed.every(
    (f) =>
      typeof f === "object" &&
      f !== null &&
      typeof (f as RenderedFile).path === "string" &&
      typeof (f as RenderedFile).content === "string"
  );
  return ok ? (parsed as RenderedFile[]) : undefined;
}

/**
 * Where `rel` lands under `root`, or why it may not be written. Lexical checks
 * first, then the real path of the deepest ancestor that exists, so a symlinked
 * `.databases` pointing out of the checkout is refused too.
 */
export function resolveInside(root: string, rel: string): { abs: string } | { reason: string } {
  if (rel.length === 0) return { reason: "empty path" };
  if (rel.includes("\0") || rel.includes("\\")) return { reason: "NUL or backslash in path" };
  if (rel.startsWith("/") || isAbsolute(rel) || /^[A-Za-z]:/.test(rel)) {
    return { reason: "absolute path" };
  }
  const segments = rel.split("/");
  if (segments.some((s) => s === "" || s === "." || s === "..")) {
    return { reason: "empty, '.' or '..' segment" };
  }
  if (segments[0] !== DATABASES_DIR || segments.length < 3) {
    return { reason: `not a file under ${DATABASES_DIR}/<database>/` };
  }
  const abs = resolve(root, ...segments);
  if (!isWithin(root, abs)) return { reason: "resolves outside the checkout" };

  const realRoot = realpathSync(root);
  let probe = abs;
  while (!existsSync(probe) && probe !== root) probe = dirname(probe);
  if (existsSync(abs) && lstatSync(abs).isSymbolicLink()) {
    return { reason: "the file is a symbolic link" };
  }
  if (!isWithin(realRoot, realpathSync(probe))) {
    return { reason: "a directory on the way is a link out of the checkout" };
  }
  return { abs };
}

function isWithin(root: string, target: string): boolean {
  const rel = relative(root, target);
  return rel === "" || (!rel.startsWith(`..${sep}`) && rel !== ".." && !isAbsolute(rel));
}

/**
 * What a write of `files` into `root` does: the action per file, and with
 * `overwrite`, the stale files of each `models/` directory the response names.
 */
export function plan(
  root: string,
  files: RenderedFile[],
  overwrite: boolean,
  /**
   * The render was narrowed to tables (`--tables`), so it holds only part of
   * each dataset: a `models/` directory is not the response's to prune, and a
   * dataset's `.sql` carries only those tables' DDL, so an existing one is kept.
   */
  partial = false
): PlannedFile[] {
  const planned: PlannedFile[] = files.map((f) => {
    const exists = existsSync(join(root, f.path));
    const replace = overwrite && !(partial && f.path.endsWith(".sql"));
    return {
      path: f.path,
      action: exists ? (replace ? "overwrite" : "skip") : "create"
    };
  });
  if (!overwrite || partial) return planned;

  const returned = new Set(files.map((f) => f.path));
  const owned = new Set(
    files
      .map((f) => f.path.split("/"))
      // `.databases/<db>/<dataset>/models/<table>.schema.yml`: the `models/`
      // directory is the sync's wholly, as `oxy sync --overwrite` treated it.
      .filter((s) => s.length === 5 && s[3] === "models")
      .map((s) => s.slice(0, 4).join("/"))
  );
  for (const dir of [...owned].sort()) {
    const abs = join(root, dir);
    if (!existsSync(abs) || !statSync(abs).isDirectory()) continue;
    for (const entry of readdirSync(abs, { withFileTypes: true })) {
      const path = `${dir}/${entry.name}`;
      if (entry.isFile() && !returned.has(path)) planned.push({ path, action: "delete" });
    }
  }
  return planned;
}

function apply(root: string, files: RenderedFile[], planned: PlannedFile[]): void {
  const content = new Map(files.map((f) => [f.path, f.content]));
  for (const p of planned) {
    const abs = join(root, p.path);
    if (p.action === "delete") {
      unlinkSync(abs);
    } else if (p.action === "create" || p.action === "overwrite") {
      mkdirSync(dirname(abs), { recursive: true });
      writeFileSync(abs, content.get(p.path) ?? "");
    }
  }
}

async function listDatabases(ctx: Context): Promise<string[]> {
  const path = listPath(ctx);
  const response = await request({
    target: ctx.target(),
    path,
    method: "GET",
    bearer: await ctx.bearer()
  });
  if (response.status < 200 || response.status >= 300) {
    throw new CliError(`GET ${path} failed (${response.status})`, {
      code: exitCodeForStatus(response.status),
      detail: response.body.trim().slice(0, 500) || undefined,
      remedy: response.status === 401 ? "oxyc login --env <env>" : undefined
    });
  }
  const parsed = parseJson(response.body);
  if (!Array.isArray(parsed)) {
    throw new CliError(`GET ${path} did not return the database list`, { code: ExitCode.FAILURE });
  }
  return parsed
    .map((d) => (d as { name?: unknown }).name)
    .filter((n): n is string => typeof n === "string");
}

async function renderOne(
  ctx: Context,
  database: string,
  opts: SyncOptions
): Promise<{ outcome: DatabaseOutcome; files: RenderedFile[]; error?: CliError }> {
  const path = syncPath(ctx);
  const response = await request({
    target: ctx.target(),
    path,
    method: "POST",
    bearer: await ctx.bearer(),
    body: JSON.stringify({ database, datasets: opts.datasets, tables: opts.tables }),
    timeoutMs: SYNC_TIMEOUT_MS
  });
  if (response.status >= 200 && response.status < 300) {
    const files = parseFiles(response.body);
    if (!files) {
      const error = new CliError(`POST ${path} did not return a list of files`, {
        code: ExitCode.FAILURE,
        hint: "the deployment may predate this route — `oxyc routes databases` lists what it mounts"
      });
      return {
        outcome: { database, status: "failed", error: error.message, code: error.code },
        files: [],
        error
      };
    }
    return { outcome: { database, status: "rendered", files: files.length }, files };
  }
  const error = syncError(database, response);
  const status = response.status === 422 ? "skipped" : "failed";
  return {
    outcome: {
      database,
      status,
      error: error.serverMessage ?? refusalReason(response.status, database) ?? error.message,
      code: error.code
    },
    files: [],
    error
  };
}

export async function runSync(ctx: Context, opts: SyncOptions): Promise<void> {
  if (opts.database === undefined && (opts.datasets.length > 0 || opts.tables.length > 0)) {
    throw usageError(
      "-d/--datasets and --tables narrow ONE database",
      "name it: oxyc sync <database> -d <dataset> --workspace <id>"
    );
  }
  const root = resolve(opts.dir ?? process.cwd());
  if (!existsSync(root) || !statSync(root).isDirectory()) {
    throw usageError(`${root} is not a directory`, "--dir takes the root of your checkout");
  }

  const named = opts.database !== undefined;
  const databases = named ? [opts.database as string] : await listDatabases(ctx);
  if (databases.length === 0) {
    log.info("the deployed config.yml configures no databases — nothing to sync");
    if (opts.json)
      process.stdout.write(`${JSON.stringify({ databases: [], files: [] }, null, 2)}\n`);
    return;
  }

  const outcomes: DatabaseOutcome[] = [];
  const files: RenderedFile[] = [];
  for (const database of databases) {
    if (!opts.json) log.info(`syncing ${database}…`);
    const result = await renderOne(ctx, database, opts);
    // One database is the whole ask: its refusal is the command's.
    if (named && result.error) throw result.error;
    // A credential that fails one fails them all.
    if (result.error?.code === ExitCode.AUTH) throw result.error;
    outcomes.push(result.outcome);
    files.push(...result.files);
  }

  for (const f of files) {
    const where = resolveInside(root, f.path);
    if ("reason" in where) {
      throw new CliError(`refusing to write ${JSON.stringify(f.path)}: ${where.reason}`, {
        code: ExitCode.REFUSED,
        detail:
          "nothing was written — every path the server returns must sit under .databases/ in the checkout"
      });
    }
  }

  const planned = plan(root, files, opts.overwrite, opts.tables.length > 0);
  if (!opts.dryRun) apply(root, files, planned);
  report(outcomes, planned, opts);

  const failed = outcomes.find((o) => o.status === "failed");
  if (failed) {
    throw new CliError(
      `${outcomes.filter((o) => o.status === "failed").length} of ${outcomes.length} databases failed to sync`,
      { code: failed.code ?? ExitCode.FAILURE }
    );
  }
}

function report(outcomes: DatabaseOutcome[], planned: PlannedFile[], opts: SyncOptions): void {
  if (opts.json) {
    process.stdout.write(
      `${JSON.stringify({ dry_run: opts.dryRun, databases: outcomes, files: planned }, null, 2)}\n`
    );
    return;
  }
  const verb: Record<FileAction, string> = opts.dryRun
    ? {
        create: "would create",
        overwrite: "would overwrite",
        skip: "would skip (exists)",
        delete: "would delete"
      }
    : { create: "created", overwrite: "overwrote", skip: "skipped (exists)", delete: "deleted" };
  for (const p of planned) log.say(`${verb[p.action]}  ${p.path}`);
  for (const o of outcomes) {
    if (o.status === "skipped") log.warn(`skipped ${o.database}: ${o.error}`);
    if (o.status === "failed") log.error(`${o.database}: ${o.error}`);
  }
  const skipped = planned.filter((p) => p.action === "skip").length;
  if (skipped > 0 && !opts.overwrite) {
    log.hint(`${skipped} existing file(s) left as they are — pass --overwrite to replace them`);
  }
}
