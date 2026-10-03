/**
 * `oxyc apps drift [<org>/<app>]` — how far the source of an app has moved past
 * the build that is live.
 *
 * For each app: the live build's recorded repository and commit, a local
 * checkout of that repository, the app's directory in it, and the commits on
 * the checkout's current branch that touch that directory after the published
 * commit.
 *
 * THREE ANSWERS, AND THE THIRD IS NOT THE FIRST. `in_sync` and `ahead` are
 * both the result of a comparison that was made. `unknown` is a comparison that
 * could NOT be made — no commit recorded, no checkout, a commit the checkout
 * does not have — and it always carries the reason. It is never folded into
 * `in_sync`: "nothing found" from a comparison that did not run is not evidence
 * of no drift.
 *
 * READ-ONLY, on both sides. The requests are GETs. The git commands are
 * `rev-parse`, `remote get-url`, `cat-file`, `merge-base`, `ls-files`, `log` and
 * `status --no-optional-locks` — nothing fetches, checks out, or refreshes the
 * index, so a commit that is only on the remote reports as `unknown` with
 * "needs a fetch" rather than being fetched.
 */

import { existsSync, readFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import type { Context } from "../context/resolve.js";
import { dossierPath, isCloned } from "../customer/dossier.js";
import { repoMap } from "../customer/repos.js";
import * as log from "../ui/log.js";
import { table } from "../ui/render.js";
import { out } from "../ui/tty.js";
import { CliError, usageError } from "../util/errors.js";
import {
  type CommitLine,
  commitsTouching,
  currentBranch,
  hasCommit,
  headSha,
  isAncestor,
  parseRemoteSlug,
  repoRoot,
  slugFromRemote,
  trackedFiles,
  uncommittedIn
} from "../util/git.js";
import { BUILDS_CONCURRENCY, printFields, printJson, provenance, shortSha } from "./apps.js";
import {
  type AppIdentity,
  type AppRow,
  appLabel,
  type BuildHistory,
  CUSTOMER_APPS,
  connect,
  fetchBuilds,
  listAllApps,
  liveBuild,
  mapBounded,
  openApp
} from "./apps-client.js";

export type DriftStatus = "in_sync" | "ahead" | "unknown";

/** Why a comparison could not be made. */
export type UnknownReason =
  | "not_published"
  | "source_unrecorded"
  | "repo_not_checked_out"
  | "commit_not_in_checkout"
  | "app_dir_not_found"
  | "app_dir_ambiguous"
  | "commit_not_on_branch"
  | "working_tree_dirty"
  | "git_failed"
  | "request_failed";

const REASON_TEXT: Record<UnknownReason, string> = {
  not_published: "nothing is live",
  source_unrecorded: "commit not recorded",
  repo_not_checked_out: "repository not checked out here",
  commit_not_in_checkout: "published commit not in the checkout — needs a fetch",
  app_dir_not_found: "app directory not found",
  app_dir_ambiguous: "more than one directory matches",
  commit_not_on_branch: "published commit is not on the checked-out branch",
  working_tree_dirty: "uncommitted changes in the app directory",
  git_failed: "a git command failed in the checkout",
  request_failed: "builds request failed"
};

/** One app's answer — the `--json` shape. */
export interface DriftReport {
  app: string;
  app_id: string;
  status: DriftStatus;
  /** Set exactly when `status` is `unknown`. */
  reason?: UnknownReason;
  /** The sentence that goes with `reason`. */
  detail?: string;
  live?: {
    build_id: string;
    source_repo: string | null;
    commit_sha: string | null;
    source_branch: string | null;
  };
  /** `<owner>/<name>` parsed from the live build's `source_repo`. */
  repo?: string;
  checkout?: { path: string; branch: string | null; head: string | null };
  /** The app's directory, relative to the checkout root. */
  app_dir?: string;
  /**
   * The commits on the checkout's branch that touch `app_dir` after the
   * published commit, newest first. Empty for `in_sync`; also filled for
   * `working_tree_dirty`, where they are known but are not the whole answer.
   */
  commits?: CommitLine[];
}

const COMMIT_RE = /^[0-9a-f]{7,64}$/i;
const SLUG_RE = /^[A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._-]*$/;

/** `<owner>/<name>` from what a build recorded: a remote URL, or already a slug. */
function repoSlugOf(sourceRepo: string): string | undefined {
  return parseRemoteSlug(sourceRepo) ?? (SLUG_RE.test(sourceRepo) ? sourceRepo : undefined);
}

interface ManifestEntry {
  /** Path of the `oxy-app.json`, relative to the checkout root. */
  file: string;
  /** The app directory it describes, relative to the checkout root. */
  dir: string;
  slug?: string;
  orgSlug?: string;
}

/**
 * Every tracked `oxy-app.json` in a checkout, with the identity it declares.
 *
 * BY MANIFEST, NOT BY FOLDER NAME. `apps/pokehouse/…` holds the org whose slug
 * is `poke-house`, so a path built from the slugs finds nothing. The manifest
 * sits at `<app>/oxy-app.json` or `<app>/public/oxy-app.json`; in the second
 * layout the app directory is the parent of `public`.
 *
 * Tracked files only (`git ls-files`), which also keeps `node_modules` and
 * build output out without a list of names to skip.
 */
function appManifests(root: string): ManifestEntry[] {
  return trackedFiles(root, ["oxy-app.json", "*/oxy-app.json"]).flatMap((file) => {
    let parsed: unknown;
    try {
      parsed = JSON.parse(readFileSync(join(root, file), "utf8"));
    } catch {
      // Deleted in the working tree, or not JSON. It cannot name an app.
      return [];
    }
    if (typeof parsed !== "object" || parsed === null) return [];
    const { slug, orgSlug } = parsed as { slug?: unknown; orgSlug?: unknown };
    const holder = dirname(file);
    return [
      {
        file,
        dir: basename(holder) === "public" ? dirname(holder) : holder,
        slug: typeof slug === "string" ? slug : undefined,
        orgSlug: typeof orgSlug === "string" ? orgSlug : undefined
      }
    ];
  });
}

/**
 * Compare one app's live build against a local checkout.
 *
 * `checkoutFor` answers where `<owner>/<name>` is checked out on this machine,
 * or `undefined`. Everything else is read from `history` and from git.
 */
export function evaluateDrift(
  app: AppIdentity,
  history: BuildHistory,
  checkoutFor: (repo: string) => string | undefined
): DriftReport {
  const base: DriftReport = { app: appLabel(app), app_id: app.id, status: "unknown" };
  const unknown = (
    reason: UnknownReason,
    detail: string,
    more: Partial<DriftReport> = {}
  ): DriftReport => ({ ...base, ...more, status: "unknown", reason, detail });

  const build = liveBuild(history);
  if (!build) return unknown("not_published", "no build of this app is being served");
  base.live = {
    build_id: build.build_id,
    source_repo: build.source_repo,
    commit_sha: build.commit_sha,
    source_branch: build.source_branch
  };

  const sha = build.commit_sha;
  if (!build.source_repo || !sha) {
    const missing = [!build.source_repo && "repository", !sha && "commit"].filter(Boolean);
    return unknown(
      "source_unrecorded",
      `the live build ${build.build_id} records no ${missing.join(" and no ")}`
    );
  }
  const repo = repoSlugOf(build.source_repo);
  if (!repo || !COMMIT_RE.test(sha)) {
    return unknown(
      "source_unrecorded",
      `the live build records ${build.source_repo} at "${sha}", which does not name a GitHub repository and a commit`
    );
  }
  base.repo = repo;

  const root = checkoutFor(repo);
  if (!root) {
    return unknown(
      "repo_not_checked_out",
      `${repo} is not checked out on this machine — gh repo clone ${repo}, or pass --dir <path>`
    );
  }
  const branch = currentBranch(root) ?? null;
  base.checkout = { path: root, branch, head: headSha(root) ?? null };
  const branchText = branch ? `branch ${branch}` : "the detached HEAD";

  if (!hasCommit(root, sha)) {
    return unknown(
      "commit_not_in_checkout",
      `${root} does not have commit ${shortSha(sha)} — run \`git fetch\` there`
    );
  }

  const manifests = appManifests(root);
  const matching = manifests.filter((m) => m.slug === app.slug && m.orgSlug === app.org_slug);
  const found = matching[0];
  if (!found) {
    const near = manifests
      .filter((m) => m.slug === app.slug)
      .map((m) => `${m.file} declares orgSlug ${m.orgSlug ? `"${m.orgSlug}"` : "nothing"}`);
    return unknown(
      "app_dir_not_found",
      `no oxy-app.json on ${branchText} of ${root} declares slug "${app.slug}" with orgSlug "${app.org_slug}"` +
        (near.length > 0 ? ` (${near.join("; ")})` : "")
    );
  }
  if (matching.length > 1) {
    return unknown(
      "app_dir_ambiguous",
      `${matching.map((m) => m.file).join(" and ")} both declare ${base.app}`
    );
  }
  base.app_dir = found.dir;

  if (!isAncestor(root, sha)) {
    const from = build.source_branch ? `; it was published from branch ${build.source_branch}` : "";
    return unknown(
      "commit_not_on_branch",
      `commit ${shortSha(sha)} is not an ancestor of ${branchText} in ${root}${from}`
    );
  }

  // Both of these throw when git fails. A log or a status that could not be
  // read says nothing about the app, so it is `unknown` here — for this app
  // only. Left to propagate, one broken checkout would end the whole fleet
  // report with no row printed for any app.
  let commits: CommitLine[];
  let dirty: string[];
  try {
    commits = commitsTouching(root, `${sha}..HEAD`, found.dir);
    dirty = uncommittedIn(root, found.dir);
  } catch (cause) {
    if (!(cause instanceof CliError)) throw cause;
    return unknown(
      "git_failed",
      cause.detail ? `${cause.message}: ${cause.detail}` : cause.message
    );
  }
  if (dirty.length > 0) {
    return unknown(
      "working_tree_dirty",
      `${dirty.length} uncommitted path(s) under ${found.dir}` +
        (commits.length > 0 ? `, on top of ${commits.length} commit(s) after the live one` : ""),
      { commits }
    );
  }
  return { ...base, status: commits.length === 0 ? "in_sync" : "ahead", commits };
}

/** `--dir`: the checkout root it is inside, and the repository its `origin` names. */
function checkoutOverride(cwd: string, dir: string): { root: string; repo?: string } {
  const path = resolve(cwd, dir);
  const root = existsSync(path) ? repoRoot(path) : undefined;
  if (!root) throw usageError(`--dir ${dir} is not inside a git working tree`);
  return { root, repo: slugFromRemote(root) };
}

/**
 * Where `<owner>/<name>` is checked out: the scan `oxyc repos` reports, then
 * the customer clone `oxyc path` reports.
 */
function discoveredCheckout(repo: string, refresh: boolean | undefined): string | undefined {
  const wanted = repo.toLowerCase();
  const scanned = Object.entries(repoMap({ refresh })).find(
    ([slug]) => slug.toLowerCase() === wanted
  );
  if (scanned) return scanned[1];
  // `dossierPath` refuses anything that is not a plain `<owner>/<name>`.
  return SLUG_RE.test(repo) && isCloned(repo) ? dossierPath(repo) : undefined;
}

export async function runAppsDrift(
  ctx: Context,
  app: string | undefined,
  flags: { dir?: string; org?: string; refresh?: boolean; json?: boolean }
): Promise<void> {
  // Every usage error before a request.
  const override = flags.dir ? checkoutOverride(ctx.cwd, flags.dir) : undefined;
  if (override && app === undefined && !override.repo) {
    throw usageError(
      `--dir ${flags.dir} has no origin remote`,
      "with no <app>, --dir is used for the apps whose source repository is that checkout's origin"
    );
  }
  const checkoutFor = (repo: string): string | undefined => {
    // Naming one app and one directory is an explicit pairing, so it holds
    // whatever the checkout's remote is called (a fork, a mirror). Across
    // every app, the override applies only to the repository it is a clone of.
    if (override && (app !== undefined || override.repo?.toLowerCase() === repo.toLowerCase())) {
      return override.root;
    }
    return discoveredCheckout(repo, flags.refresh);
  };

  if (app !== undefined) {
    const { conn, row } = await openApp(ctx, app);
    const report = evaluateDrift(row, await fetchBuilds(conn, row.id), checkoutFor);
    if (flags.json) printJson(report);
    else printOne(report);
    return;
  }

  const conn = connect(ctx);
  const { rows: all, complete } = await listAllApps<AppRow>(conn, CUSTOMER_APPS);
  if (!complete) {
    log.warn(`stopped at ${all.length} apps — the list is TRUNCATED, the deployment has more.`);
  }
  const inScope = all.filter((row) => !flags.org || row.org_slug === flags.org);
  const rows = inScope
    .filter((row) => row.published_at != null)
    .sort((a, b) => appLabel(a).localeCompare(appLabel(b)));

  const failed: CliError[] = [];
  const reports = await mapBounded(rows, BUILDS_CONCURRENCY, async (row): Promise<DriftReport> => {
    let history: BuildHistory;
    try {
      history = await fetchBuilds(conn, row.id);
    } catch (cause) {
      if (!(cause instanceof CliError)) throw cause;
      failed.push(cause);
      return {
        app: appLabel(row),
        app_id: row.id,
        status: "unknown",
        reason: "request_failed",
        detail: cause.message
      };
    }
    return evaluateDrift(row, history, checkoutFor);
  });

  if (flags.json) {
    printJson(reports);
  } else if (reports.length === 0) {
    log.info(
      inScope.length === 0
        ? `no custom apps are visible to you on ${conn.target}${flags.org ? ` in ${flags.org}` : ""}`
        : `none of the ${inScope.length} app(s) has a published build to compare`
    );
  } else {
    process.stdout.write(
      `${table(reports, [
        { header: "APP", value: (r) => r.app },
        { header: "DRIFT", value: statusText },
        { header: "LIVE COMMIT", value: liveText },
        { header: "CHECKOUT", value: checkoutText },
        { header: "DETAIL", value: (r) => r.detail ?? commitsInline(r.commits ?? []) }
      ])}\n`
    );
    const count = (status: DriftStatus) => reports.filter((r) => r.status === status).length;
    const drafts = inScope.length - rows.length;
    log.info(
      `${reports.length} published app(s): ${count("in_sync")} in sync, ${count("ahead")} ahead, ` +
        `${count("unknown")} unknown` +
        (drafts > 0 ? ` — ${drafts} draft app(s) not compared, nothing of theirs is live` : "")
    );
  }

  const first = failed[0];
  if (first) {
    throw new CliError(`${failed.length} of ${rows.length} builds request(s) failed`, {
      code: first.code,
      detail: first.message,
      hint: "those apps read `unknown — builds request failed`; they were not compared"
    });
  }
}

function statusText(report: DriftReport): string {
  if (report.status === "in_sync") return out.green("in sync");
  if (report.status === "ahead") {
    const n = report.commits?.length ?? 0;
    return out.yellow(`${n} commit${n === 1 ? "" : "s"} ahead`);
  }
  return `unknown — ${REASON_TEXT[report.reason ?? "request_failed"]}`;
}

function liveText(report: DriftReport): string {
  const live = report.live;
  if (!live) return "—";
  if (!live.source_repo || !live.commit_sha) return "not recorded";
  return provenance({ ...live, source_repo: report.repo ?? live.source_repo });
}

function checkoutText(report: DriftReport): string {
  const checkout = report.checkout;
  if (!checkout) return "—";
  return `${checkout.path} (${checkout.branch ?? "detached HEAD"})`;
}

/** The table's one-line form of a commit list: the newest three, then a count. */
const COMMITS_INLINE = 3;
function commitsInline(commits: CommitLine[]): string {
  const shown = commits.slice(0, COMMITS_INLINE).map((c) => `${shortSha(c.sha)} ${c.subject}`);
  if (commits.length > COMMITS_INLINE) shown.push(`+${commits.length - COMMITS_INLINE} more`);
  return shown.join("; ");
}

/** How many commits the one-app form lists before it counts the rest. */
const COMMITS_LISTED = 20;

function printOne(report: DriftReport): void {
  const fields: Array<[string, string]> = [
    ["app", report.app],
    ["drift", statusText(report)]
  ];
  if (report.detail) fields.push(["", report.detail]);
  if (report.live) {
    fields.push(["live build", `${report.live.build_id}  ${liveText(report)}`]);
  }
  if (report.checkout) {
    const head = report.checkout.head ? ` at ${shortSha(report.checkout.head)}` : "";
    fields.push(["checkout", `${checkoutText(report)}${head}`]);
  }
  if (report.app_dir) fields.push(["app directory", report.app_dir]);
  printFields(fields);

  const commits = report.commits ?? [];
  if (commits.length === 0) return;
  const lines = commits
    .slice(0, COMMITS_LISTED)
    .map((c) => `  ${shortSha(c.sha)}  ${c.date}  ${c.subject}  (${c.author})`);
  if (commits.length > COMMITS_LISTED) {
    lines.push(`  and ${commits.length - COMMITS_LISTED} more — \`--json\` has all of them`);
  }
  process.stdout.write(
    `\ncommits touching ${report.app_dir} after the published commit, newest first:\n${lines.join("\n")}\n`
  );
}
