// Gather everything a review round needs: the PR, its previous review, its diff, the plan.

import { existsSync } from "node:fs";
import { join } from "node:path";
import * as config from "../config.ts";
import { parseUnifiedDiff } from "./diff.ts";
import { git, run } from "./exec.ts";
import type * as GitHubApi from "./github.ts";
import { buildPlan } from "./plan.ts";
import { decodeState, emptyState, findSummaryComment, isClosed } from "./state.ts";
import type {
  DiffFile,
  IssueComment,
  Plan,
  PrMeta,
  ReviewComment,
  ReviewState,
  ReviewStatus
} from "./types.ts";

type Gh = Pick<typeof GitHubApi, "getPr" | "listIssueComments" | "listReviewComments">;
type Ancestor = (a: string, b: string, cwd: string) => Promise<boolean>;

export interface LoadArgs {
  head?: string | undefined;
  base?: string | undefined;
  post: boolean;
  fresh: boolean;
}

/** Everything one round reads. `since` is the last reviewed head for an incremental round. */
export interface Loaded {
  meta: PrMeta;
  head: string;
  mergeBase: string;
  comments: IssueComment[];
  summary: IssueComment | null;
  prev: ReviewState;
  reviewComments: ReviewComment[];
  files: DiffFile[];
  plan: Plan;
  since: string | null;
}

async function ensureCommit(sha: string, cwd: string): Promise<void> {
  const { code } = await run("git", ["cat-file", "-e", `${sha}^{commit}`], {
    cwd,
    allowFail: true
  });
  if (code !== 0) await git(["fetch", "--no-tags", "origin", sha], { cwd });
}

const isAncestor: Ancestor = async (a, b, cwd) => {
  const { code } = await run("git", ["merge-base", "--is-ancestor", a, b], {
    cwd,
    allowFail: true
  });
  return code === 0;
};

// A round that read the whole diff. Anything else (none, incomplete, skipped for want of
// credentials) leaves parts unread, so the next push reviews the whole PR again.
const READ_IT_ALL: readonly ReviewStatus[] = ["complete", "partial"];

/** PR files changed since the last reviewed head, or null when a full review is due. */
export async function changedSinceLast(
  prev: Pick<ReviewState, "head" | "status">,
  head: string,
  prFiles: readonly Pick<DiffFile, "path">[],
  cwd: string,
  ancestor: Ancestor = isAncestor
): Promise<Set<string> | null> {
  if (!prev.head || prev.head === head || !READ_IT_ALL.includes(prev.status)) return null;
  if (!(await ancestor(prev.head, head, cwd).catch(() => false))) return null;
  const names = (await git(["diff", "--name-only", prev.head, head], { cwd }))
    .split("\n")
    .filter(Boolean);
  const inPr = new Set(prFiles.map((f) => f.path));
  return new Set(names.filter((n) => inPr.has(n)));
}

/**
 * Lenses that finished on an earlier full review of this PR, and so read every file then
 * in it. A lens that finished only on incremental rounds read only the files those
 * rounds changed, so it does not count: on a later push it still matches the whole diff
 * of each file that push touches.
 */
export function lensesFinished(prev: Pick<ReviewState, "rounds">): Set<string> {
  const ids = new Set(config.LENSES.map((l) => l.id));
  const done = new Set<string>();
  for (const r of prev.rounds) {
    if (r.mode !== "full") continue;
    for (const [name, p] of Object.entries(r.passes ?? {}))
      if (p.ok && ids.has(name)) done.add(name);
  }
  return done;
}

export async function loadRound({
  gh,
  repo,
  pr,
  args,
  cwd
}: {
  gh: Gh;
  repo: string;
  pr: number;
  args: LoadArgs;
  cwd: string;
}): Promise<Loaded> {
  const meta = await gh.getPr(repo, pr);
  const head = args.head || meta.head.sha;
  const baseTip = args.base || meta.base.sha || meta.base.ref;
  await ensureCommit(head, cwd);
  await ensureCommit(baseTip, cwd);
  const mergeBase = (await git(["merge-base", baseTip, head], { cwd })).trim();

  const comments = args.post || !args.fresh ? await gh.listIssueComments(repo, pr) : [];
  const summary = findSummaryComment(comments, config.SUMMARY_MARKER, config.REVIEWER_LOGINS);
  const decoded = args.fresh ? null : decodeState(summary?.body, config.STATE_VERSION);
  const prev = decoded ?? emptyState(pr, config.STATE_VERSION);
  const reviewComments = prev.findings.some((f) => f.inline && !isClosed(f.status))
    ? await gh.listReviewComments(repo, pr)
    : [];

  const diffText = await git(["diff", "--no-color", "--no-ext-diff", "-M", mergeBase, head], {
    cwd
  });
  const files = parseUnifiedDiff(diffText);
  const changedSince = await changedSinceLast(prev, head, files, cwd);
  const delta =
    changedSince && prev.head
      ? parseUnifiedDiff(
          await git(["diff", "--no-color", "--no-ext-diff", "-M", prev.head, head], { cwd })
        )
      : null;
  const plan = buildPlan({
    files,
    config,
    exists: (p) => existsSync(join(cwd, p)),
    changedSince,
    delta,
    covered: lensesFinished(prev)
  });
  return {
    meta,
    head,
    mergeBase,
    comments,
    summary,
    prev,
    reviewComments,
    files,
    plan,
    since: changedSince ? prev.head : null
  };
}
