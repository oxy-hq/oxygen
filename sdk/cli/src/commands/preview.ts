/**
 * `oxyc preview` — staff open a branch of a workspace on the real product,
 * against real data, without the branch being live
 * (`internal-docs/workspace-previews.md`). `create`/`list`/`show`/`delete`/
 * `checks` talk to `/api/{workspace}/previews[...]`
 * (`crates/app/src/server/api/workspace_previews.rs`), staff-only
 * (`WorkspacePreviewer`) — a plain user bearer, like every other `/api/
 * {workspace_id}/...` route; there is no publish-token or API-key path onto
 * this surface. Run verbs live in `preview-runs.ts`.
 *
 * There is no GET-by-branch route: `show` fetches the list and filters it
 * client-side, so its NOT_FOUND is synthesized here, not a literal 404 from
 * the server.
 */

import { parseJson, request } from "../api/request.js";
import { ensureOk } from "../apps/resolve.js";
import type { Context } from "../context/resolve.js";
import { previewsPath } from "../previews/resolve.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { confirm } from "./oltp-client.js";

export interface PreviewCreator {
  id: string;
  name: string;
}

export interface CheckSummary {
  /** `pending` | `done` | `failed`. */
  status: string;
  needs_reset: number;
  warnings: number;
  transforms: number;
}

/** `GET /previews` item, and what `POST /previews` / `POST /previews/refresh` wrap in `{item}`. */
export interface PreviewItem {
  branch: string;
  revision_id: string | null;
  sha: string | null;
  /** `compiling` | `ready` | `failed` | `stale`. */
  status: string;
  error: string | null;
  created_by: PreviewCreator | null;
  updated_at: string;
  compiled_at: string | null;
  checks: CheckSummary | null;
}

export interface ChecksResponse {
  branch: string;
  revision_id: string | null;
  /** `pending` | `done` | `failed`. */
  status: string;
  error: string | null;
  pipelines: unknown[];
  transforms: unknown[];
}

/** A preview's compile will never move past these on its own. */
const TERMINAL_PREVIEW_STATUSES = new Set(["ready", "failed"]);

/**
 * Whether a (terminal) `PreviewItem` is a failure rather than a success — the
 * ONE place that decision is made, so `runPreviewCreate` (exit code) and
 * `oxy_preview_create` (`mcp.ts`'s `isError`) can never disagree on the same
 * data the way they used to (the CLI verb exited 0 on a failed compile; the
 * MCP tool already set `isError` on it).
 */
export function previewFailed(item: PreviewItem): boolean {
  return item.status === "failed";
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function validateWaitSeconds(waitSeconds: number | undefined): void {
  if (waitSeconds === undefined) return;
  if (!Number.isFinite(waitSeconds) || waitSeconds <= 0) {
    throw usageError(
      "--wait must be a positive number of seconds",
      "e.g. --wait 60 (the default is 120)"
    );
  }
}

async function postCreate(ctx: Context, branch: string): Promise<PreviewItem> {
  const response = await request({
    target: ctx.target(),
    path: previewsPath(ctx),
    method: "POST",
    body: JSON.stringify({ branch }),
    bearer: ctx.bearer()
  });
  ensureOk(response);
  const payload = parseJson(response.body) as { item?: PreviewItem } | undefined;
  if (!payload?.item) {
    throw new CliError("POST .../previews did not return an item", { code: ExitCode.UNAVAILABLE });
  }
  return payload.item;
}

export async function previewList(ctx: Context): Promise<PreviewItem[]> {
  const response = await request({
    target: ctx.target(),
    path: previewsPath(ctx),
    method: "GET",
    bearer: ctx.bearer()
  });
  ensureOk(response);
  const payload = parseJson(response.body) as { items?: PreviewItem[] } | undefined;
  return payload?.items ?? [];
}

export async function previewShow(ctx: Context, branch: string): Promise<PreviewItem> {
  const found = (await previewList(ctx)).find((i) => i.branch === branch);
  if (!found) {
    throw new CliError(`there is no preview of branch ${branch}`, {
      code: ExitCode.NOT_FOUND,
      hint: "oxyc preview create <branch> to start one, or oxyc preview list to see what exists"
    });
  }
  return found;
}

/** Poll `previewShow` until the branch's compile reaches a terminal status. */
async function pollPreviewToTerminal(
  ctx: Context,
  branch: string,
  waitSeconds: number,
  pollMs: number
): Promise<PreviewItem> {
  const deadline = Date.now() + waitSeconds * 1000;
  for (;;) {
    const item = await previewShow(ctx, branch);
    if (TERMINAL_PREVIEW_STATUSES.has(item.status)) return item;
    if (Date.now() >= deadline) {
      throw new CliError(`preview of ${branch} did not finish compiling within ${waitSeconds}s`, {
        code: ExitCode.UNAVAILABLE,
        hint: "the compile runs on the IDE instance — retry `oxyc preview create … --wait`, or check `oxyc preview show` later"
      });
    }
    await sleep(pollMs);
  }
}

/**
 * Preview a branch: compile its head (or reuse a ready revision of that
 * commit) and start serving it. Idempotent. With `waitSeconds`, blocks until
 * the compile reaches `ready` or `failed`, rather than returning the
 * `compiling` item the POST itself answers with.
 */
export async function previewCreate(
  ctx: Context,
  branch: string,
  opts: { waitSeconds?: number; pollMs?: number } = {}
): Promise<PreviewItem> {
  validateWaitSeconds(opts.waitSeconds);
  const item = await postCreate(ctx, branch);
  if (opts.waitSeconds === undefined || TERMINAL_PREVIEW_STATUSES.has(item.status)) return item;
  return pollPreviewToTerminal(ctx, branch, opts.waitSeconds, opts.pollMs ?? 2000);
}

export async function previewDelete(
  ctx: Context,
  branch: string,
  opts: { yes?: boolean } = {}
): Promise<{ branch: string; deleted: true }> {
  if (!opts.yes) {
    await confirm(`Stop previewing ${branch}?`, {
      verb: "a preview delete",
      why: "it cancels the preview's queued runs and releases its staging revision",
      declined: "not deleted — the confirmation was declined"
    });
  }
  const response = await request({
    target: ctx.target(),
    path: `${previewsPath(ctx)}?branch=${encodeURIComponent(branch)}`,
    method: "DELETE",
    bearer: ctx.bearer()
  });
  ensureOk(response);
  return { branch, deleted: true };
}

export async function previewChecks(ctx: Context, branch: string): Promise<ChecksResponse> {
  const response = await request({
    target: ctx.target(),
    path: `${previewsPath(ctx, "/checks")}?branch=${encodeURIComponent(branch)}`,
    method: "GET",
    bearer: ctx.bearer()
  });
  ensureOk(response);
  return parseJson(response.body) as ChecksResponse;
}

function printPreviewItem(item: PreviewItem, asJson: boolean): void {
  if (asJson) {
    process.stdout.write(`${JSON.stringify(item)}\n`);
    return;
  }
  process.stdout.write(`${item.branch} — ${item.status}${item.error ? `: ${item.error}` : ""}\n`);
  if (item.checks) {
    const c = item.checks;
    process.stdout.write(
      `  checks: ${c.status} (needs_reset=${c.needs_reset}, warnings=${c.warnings}, transforms=${c.transforms})\n`
    );
  }
}

export async function runPreviewCreate(
  ctx: Context,
  branch: string,
  opts: { waitSeconds?: number; json: boolean; pollMs?: number }
): Promise<void> {
  const item = await previewCreate(ctx, branch, opts);
  printPreviewItem(item, opts.json);
  // Only meaningful on the AWAITED path: without --wait, the item is just
  // whatever snapshot the POST itself returned (almost always "compiling"),
  // not a result the caller asked to be told the outcome of.
  if (opts.waitSeconds !== undefined && previewFailed(item)) {
    throw new CliError(
      `preview of ${branch} failed to compile${item.error ? `: ${item.error}` : ""}`,
      { code: ExitCode.FAILURE }
    );
  }
}

export async function runPreviewList(ctx: Context, opts: { json: boolean }): Promise<void> {
  const items = await previewList(ctx);
  if (opts.json) {
    process.stdout.write(`${JSON.stringify({ items })}\n`);
    return;
  }
  if (items.length === 0) {
    process.stderr.write("no previews\n");
    return;
  }
  for (const item of items) {
    process.stdout.write(`${item.branch.padEnd(30)} ${item.status.padEnd(10)} ${item.sha ?? ""}\n`);
  }
}

export async function runPreviewShow(
  ctx: Context,
  branch: string,
  opts: { json: boolean }
): Promise<void> {
  printPreviewItem(await previewShow(ctx, branch), opts.json);
}

export async function runPreviewDelete(
  ctx: Context,
  branch: string,
  opts: { yes?: boolean; json: boolean }
): Promise<void> {
  const result = await previewDelete(ctx, branch, opts);
  if (opts.json) process.stdout.write(`${JSON.stringify(result)}\n`);
  else process.stdout.write(`${out.green(branch)} is no longer previewed\n`);
}

export async function runPreviewChecks(
  ctx: Context,
  branch: string,
  opts: { json: boolean }
): Promise<void> {
  const checks = await previewChecks(ctx, branch);
  if (opts.json) {
    process.stdout.write(`${JSON.stringify(checks)}\n`);
    return;
  }
  const line = `${checks.branch} — ${checks.status}${checks.error ? `: ${checks.error}` : ""}\n`;
  process.stdout.write(line);
  process.stdout.write(
    `  pipelines: ${checks.pipelines.length}, transforms: ${checks.transforms.length}\n`
  );
}
