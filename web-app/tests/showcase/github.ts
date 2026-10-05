// GitHub through the `gh` CLI (present on every runner). Its token is handed to
// each call rather than left in the environment — see credentials.ts. Always
// `-R <repo>`: the cwd is not a reliable repo pointer.

import { spawnSync } from "node:child_process";
import { ghAuthEnv } from "./credentials";
import type { PrFacts } from "./detect";
import { COMMENT_MARKER } from "./record";

function gh(args: string[], input?: string): string {
  const res = spawnSync("gh", args, {
    encoding: "utf-8",
    input,
    maxBuffer: 256 * 1024 * 1024,
    env: { ...process.env, ...ghAuthEnv() }
  });
  if (res.status !== 0) {
    throw new Error(`gh ${args.slice(0, 3).join(" ")} failed: ${(res.stderr || "").trim()}`);
  }
  return res.stdout;
}

/** What a PR is at the moment it is read: the filter's facts, and the commit they describe. */
export type PrSnapshot = PrFacts & { headSha: string };

export function prFacts(repo: string, pr: number): PrSnapshot {
  const view = JSON.parse(
    gh(["pr", "view", String(pr), "-R", repo, "--json", "title,body,labels,headRefOid"])
  ) as { title: string; body: string; labels: { name: string }[]; headRefOid: string };
  const files = gh(["api", `repos/${repo}/pulls/${pr}/files`, "--paginate", "--jq", ".[].filename"])
    .split("\n")
    .filter(Boolean);
  return {
    title: view.title,
    body: view.body ?? "",
    labels: view.labels.map((l) => l.name),
    files,
    headSha: view.headRefOid
  };
}

const NOISE = /^(pnpm-lock\.yaml|Cargo\.lock|.*\.snap|.*\.lock|.*\.min\.js|.*\.svg)$/;

/**
 * The PR's diff for the planner: generated files dropped, browser source first
 * so a budget cut falls on the backend rather than the screen that changed.
 */
export function prDiff(repo: string, pr: number): string {
  const blocks = rawDiff(repo, pr)
    .split(/^(?=diff --git )/m)
    .filter(Boolean);
  const path = (b: string) => /^diff --git a\/(\S+)/.exec(b)?.[1] ?? "";
  const kept = blocks.filter((b) => !NOISE.test(path(b).split("/").pop() ?? ""));
  const rank = (b: string) => (path(b).startsWith("web-app/src/") ? 0 : 1);
  return kept.sort((a, b) => rank(a) - rank(b)).join("");
}

/**
 * GitHub refuses the whole-PR diff past 300 files or 20k lines (406). Such a
 * PR still has per-file patches — rebuilt into the same `diff --git` shape;
 * a file too big for a patch contributes its header only.
 */
function rawDiff(repo: string, pr: number): string {
  try {
    return gh(["pr", "diff", String(pr), "-R", repo]);
  } catch {
    return gh([
      "api",
      `repos/${repo}/pulls/${pr}/files`,
      "--paginate",
      "--jq",
      '.[] | "diff --git a/\\(.filename) b/\\(.filename)\\n\\(.patch // "")"'
    ]);
  }
}

export interface Comment {
  id: number;
  body: string;
}

// The comment a release reads is the one CI wrote: a hand-posted copy of the
// marker must not be able to tell a release a PR was already pictured.
const COMMENT_AUTHOR = "github-actions[bot]";

export function findShowcaseComment(repo: string, pr: number): Comment | undefined {
  const out = gh([
    "api",
    `repos/${repo}/issues/${pr}/comments`,
    "--paginate",
    "--jq",
    `.[] | select(.user.login == "${COMMENT_AUTHOR}" and (.body | startswith("${COMMENT_MARKER}"))) | {id, body}`
  ]);
  const found = out
    .split("\n")
    .filter(Boolean)
    .map((l) => JSON.parse(l) as Comment);
  return found.at(-1);
}

export function upsertShowcaseComment(repo: string, pr: number, body: string): void {
  const existing = findShowcaseComment(repo, pr);
  const payload = JSON.stringify({ body });
  if (existing) {
    gh(
      ["api", "-X", "PATCH", `repos/${repo}/issues/comments/${existing.id}`, "--input", "-"],
      payload
    );
  } else {
    gh(["api", "-X", "POST", `repos/${repo}/issues/${pr}/comments`, "--input", "-"], payload);
  }
}

/**
 * Squash subjects between two commits, oldest first — the announcement's own
 * source. Deliberately NOT paginated: the compare API returns at most 250
 * commits, and the announcement renders from exactly that page, so following
 * further pages could picture features the announcement never listed. A cut
 * range is said out loud instead, as the announcement itself does.
 */
export function compareSubjects(repo: string, base: string, head: string): string[] {
  const out = JSON.parse(
    gh([
      "api",
      `repos/${repo}/compare/${base}...${head}`,
      "--jq",
      '{total: .total_commits, subjects: [.commits[].commit.message | split("\\n")[0]]}'
    ])
  ) as { total: number; subjects: string[] };
  if (out.total > out.subjects.length) {
    console.warn(
      `[showcase] ${out.total} commits in range, the compare API returned ${out.subjects.length}: ` +
        "features past them are not in the announcement either, and get no picture"
    );
  }
  return out.subjects;
}
