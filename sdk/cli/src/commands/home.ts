/**
 * `oxyc home show | get | apply | reset` — the org's home layout, from a terminal.
 *
 * The home is laid out by one document (sections, and widgets in order, each at a
 * named size). These verbs let an agent see what a person sees, read that
 * document, change it and write it back:
 *
 *   show                    GET /orgs/{id}/home + GET /home/widgets?org=  (as text, or --json)
 *   get                     the effective layout as JSON — the round-trip form
 *   apply -f layout.json    PUT /orgs/{id}/home/layout   {layout, revision?, dry_run?}
 *   reset                   DELETE /orgs/{id}/home/layout
 *
 * `apply` sends the revision it just read, so a layout someone else changed in
 * between is a 409 (exit 8), not an overwrite; `--force` sends none. A refused
 * layout is a 422 listing every problem at once, printed as `path: message (fix)`.
 */

import type { HomeDescription, HomeLayout } from "@oxy-hq/sdk/manifest";
import { parseJson } from "../api/request.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { resolveOrgId } from "./assume.js";
import { readDocument } from "./experiments-client.js";
import {
  call,
  homeError,
  homePath,
  problemLines,
  readJson,
  send,
  type WidgetsFeed,
  widgetsPath
} from "./home-client.js";
import { renderHome } from "./home-render.js";

function emit(value: unknown): void {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}

async function readHome(ctx: Context, orgId: string): Promise<HomeDescription> {
  return call<HomeDescription>(ctx, "GET", homePath(orgId));
}

async function readFeed(ctx: Context, orgSlug: string): Promise<WidgetsFeed> {
  return call<WidgetsFeed>(ctx, "GET", widgetsPath(orgSlug));
}

/** Draws the home as text, or `{home, widgets}` raw with `--json`. */
export async function runHomeShow(
  ctx: Context,
  org: string | undefined,
  opts: { json: boolean }
): Promise<void> {
  const orgId = await resolveOrgId(ctx, org);
  const home = await readHome(ctx, orgId);
  const feed = await readFeed(ctx, home.org.slug);
  if (opts.json) {
    emit({ home, widgets: feed });
    return;
  }
  process.stdout.write(renderHome(home, feed));
}

/** The effective layout, as the JSON `apply` reads back. */
/**
 * The effective layout and the revision it was read at, as
 * `{ "revision": N, "layout": … }`: `apply` sends that revision back, so an
 * edit made on a stale read is refused rather than written over someone else's.
 */
export async function runHomeGet(ctx: Context, org: string | undefined): Promise<void> {
  const orgId = await resolveOrgId(ctx, org);
  const home = await readHome(ctx, orgId);
  emit({ revision: home.revision, layout: home.effective_layout });
}

/** What `apply -f` read: the layout, and the revision when the file carries one. */
export interface LayoutFile {
  layout: HomeLayout;
  revision?: number;
}

/**
 * The file `apply` writes: what `home get` prints (`{ revision, layout }`), or a
 * bare layout document. YAML or JSON, from a path or `-`.
 */
export function readLayout(file: string | undefined): LayoutFile {
  if (!file) {
    throw usageError(
      "-f, --file <path|-> is required",
      "start from `oxyc home get > layout.json`, edit it, then `oxyc home apply -f layout.json --dry-run`"
    );
  }
  const doc = readDocument(file, file, { pathLike: true });
  if (!doc || typeof doc !== "object" || Array.isArray(doc)) {
    throw usageError(
      `${file} must hold one layout document (an object with version and sections)`,
      "`oxyc home get` prints one"
    );
  }
  const record = doc as Record<string, unknown>;
  if ("layout" in record) {
    return {
      layout: record.layout as HomeLayout,
      revision:
        record.revision === undefined ? undefined : asRevision(record.revision, `${file}: revision`)
    };
  }
  return { layout: doc as HomeLayout };
}

/**
 * A revision as given: a whole number from 0. Anything else is refused before a
 * request goes out — a `NaN` would serialize as `null`, which the server reads
 * as "no revision" and writes over whatever is saved.
 */
function asRevision(value: unknown, what: string): number {
  const n = typeof value === "string" && value.trim() !== "" ? Number(value) : value;
  if (typeof n !== "number" || !Number.isInteger(n) || n < 0) {
    throw usageError(
      `${what} must be a whole number from 0, not ${JSON.stringify(value)}`,
      "use the revision `oxyc home get` printed, or --force to overwrite"
    );
  }
  return n;
}

/**
 * The revision to send: `--revision`, else the file's. Never one read now — that
 * would let an edit made on a stale read overwrite a newer layout unseen.
 * `--force` sends none.
 */
function revisionToSend(file: LayoutFile, opts: { revision?: string; force: boolean }) {
  if (opts.force) return undefined;
  const revision =
    opts.revision === undefined ? file.revision : asRevision(opts.revision, "--revision");
  if (revision === undefined) {
    throw usageError(
      "no revision to send: apply writes only over the layout you read",
      "use the file `oxyc home get` prints (it carries the revision), pass --revision N, or --force to overwrite"
    );
  }
  return revision;
}

/** A 409: someone saved a layout after the one this run read. */
function staleError(): CliError {
  return new CliError("the home layout changed since it was read", {
    code: ExitCode.REFUSED,
    detail: "nothing was saved: another writer got there first",
    remedy: "run `oxyc home get` again, redo your edit on it, or pass --force to overwrite"
  });
}

/** A 422: every problem, each as `path: message (fix)`. */
function invalidError(problems: string[]): CliError {
  const count = `${problems.length} problem${problems.length === 1 ? "" : "s"}`;
  return new CliError(`the layout was refused: ${count}`, {
    code: ExitCode.REQUEST,
    detail: problems.join("\n"),
    remedy: "fix them all, then re-run (`--dry-run` checks without saving)"
  });
}

/** The saved or previewed description, or the error a 409 / 422 / other status deserves. */
function readWrite(path: string, response: Awaited<ReturnType<typeof send>>): HomeDescription {
  if (response.status === 409) throw staleError();
  if (response.status === 422) {
    const problems = problemLines(response.body);
    if (problems.length > 0) throw invalidError(problems);
  }
  return readJson<HomeDescription>("PUT", path, response);
}

/** A dry run's live views come from the saved layout's feed; a failure there is a warning. */
async function previewFeed(ctx: Context, orgSlug: string): Promise<WidgetsFeed | undefined> {
  try {
    return await readFeed(ctx, orgSlug);
  } catch (cause) {
    log.warn(`live views are unavailable for the preview: ${(cause as Error).message}`);
    return undefined;
  }
}

/** Writes (or, with `--dry-run`, previews) a layout. */
export async function runHomeApply(
  ctx: Context,
  org: string | undefined,
  opts: { file?: string; revision?: string; dryRun: boolean; force: boolean; json: boolean }
): Promise<void> {
  const file = readLayout(opts.file);
  const revision = revisionToSend(file, opts);
  const orgId = await resolveOrgId(ctx, org);
  const path = homePath(orgId, "/layout");
  const body: Record<string, unknown> = { layout: file.layout };
  if (revision !== undefined) body.revision = revision;
  if (opts.dryRun) body.dry_run = true;
  const result = readWrite(path, await send(ctx, "PUT", path, body));

  if (opts.dryRun) {
    if (opts.json) return emit(result);
    const feed = await previewFeed(ctx, result.org.slug);
    process.stdout.write(renderHome(result, feed, { previewing: true }));
    log.info("dry run: nothing was saved");
    return;
  }
  if (opts.json) emit(result);
  log.info(`saved the home layout (revision ${result.revision})`);
}

/** Back to the default layout. */
export async function runHomeReset(ctx: Context, org: string | undefined): Promise<void> {
  const orgId = await resolveOrgId(ctx, org);
  const path = homePath(orgId, "/layout");
  const response = await send(ctx, "DELETE", path);
  if (response.status < 200 || response.status >= 300) throw homeError("DELETE", path, response);
  const revision = (parseJson(response.body) as { revision?: number } | undefined)?.revision;
  log.info(
    `the home is back to the default layout${revision === undefined ? "" : ` (revision ${revision})`}`
  );
}
