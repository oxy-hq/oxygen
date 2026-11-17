// Everything the review writes to GitHub. `gh` is the github.ts module, passed in so
// tests can hand over a fake.

import { commentableLines, hunkText } from "./diff.ts";
import { severityRank } from "./findings.ts";
import { renderInline } from "./render.ts";
import type { DiffFile, Finding, GitHub, IssueComment } from "./types.ts";

const FINDING_TAG = /<!-- oxy-review:finding (F\d+) -->/;
const LEGACY_VERDICT =
  /\b(LGTM|Needs changes|Blocking|No actionable issues found)\b|<!-- claude-review-incomplete -->/;

type Clean = (text: string) => string;
const keep: Clean = (t) => t;

export interface InlinePick {
  finding: Finding;
  line: number;
  /** First line of a multi-line comment: the span a suggestion replaces. */
  startLine?: number;
  /** Post the finding's suggestion: it replaces exactly `startLine`..`line`. */
  suggest: boolean;
}

/**
 * Where a finding's suggestion can be posted: its lines, all in one hunk, and not already
 * reading what it suggests. Null sends the finding out with its fix in words only.
 */
function suggestionSpan(f: Finding, file: DiffFile | undefined): [number, number] | null {
  if (f.suggestion === undefined || !file) return null;
  const end = f.end_line ?? f.line;
  const current = hunkText(file, f.line, end);
  if (current === null || current.replace(/\s+$/, "") === f.suggestion) return null;
  return [f.line, end];
}

/** New findings worth an inline comment, anchored to a line GitHub will accept. */
export function pickInline(
  findings: readonly Finding[],
  diffFiles: readonly DiffFile[],
  max: number
): InlinePick[] {
  const files = new Map(diffFiles.map((f) => [f.path, f]));
  const lines = new Map(diffFiles.map((f) => [f.path, commentableLines(f)]));
  const picked: InlinePick[] = [];
  const sorted = [...findings].sort((a, b) => severityRank(b.severity) - severityRank(a.severity));
  for (const f of sorted) {
    if (picked.length >= max) break;
    if (f.inline || severityRank(f.severity) < severityRank("should_fix")) continue;
    const ok = lines.get(f.file);
    if (!ok) continue;
    const span = suggestionSpan(f, files.get(f.file));
    if (span) {
      const [start, end] = span;
      picked.push({
        finding: f,
        line: end,
        suggest: true,
        ...(start < end ? { startLine: start } : {})
      });
      continue;
    }
    const anchor = [f.line, ...(f.end_line ? [f.end_line] : [])].find((n) => ok.has(n));
    if (anchor) picked.push({ finding: f, line: anchor, suggest: false });
  }
  return picked;
}

/** Post one review carrying the inline comments; returns finding id → comment id. */
export async function postInline({
  gh,
  repo,
  pr,
  head,
  picks,
  summaryUrl,
  clean = keep
}: {
  gh: Pick<GitHub, "createReview" | "listReviewComments">;
  repo: string;
  pr: number;
  head: string;
  picks: readonly InlinePick[];
  summaryUrl: string | undefined;
  clean?: Clean;
}): Promise<Map<string, number>> {
  if (!picks.length) return new Map();
  const ids = picks.map((p) => p.finding.id).join(", ");
  const review = await gh.createReview(repo, pr, {
    commit_id: head,
    event: "COMMENT",
    body: `Automated review of \`${head.slice(0, 9)}\`: ${ids} inline.${summaryUrl ? ` [Summary and verdict](${summaryUrl}).` : ""}`,
    comments: picks.map((p) => ({
      path: p.finding.file,
      ...(p.startLine === undefined ? {} : { start_line: p.startLine, start_side: "RIGHT" }),
      line: p.line,
      side: "RIGHT",
      body: clean(renderInline(p.finding, { suggest: p.suggest }))
    }))
  });
  const posted = await gh.listReviewComments(repo, pr);
  const out = new Map<string, number>();
  for (const c of posted) {
    if (c.pull_request_review_id !== review.id) continue;
    const id = (c.body ?? "").match(FINDING_TAG)?.[1];
    if (id) out.set(id, c.id);
  }
  return out;
}

/**
 * Close the loop on findings that closed this round: a reply in each inline thread says
 * how, then the thread is resolved where the token may (GITHUB_TOKEN may not, so the
 * reply is what readers see). Only a failed reply is reported.
 */
export async function resolveClosed({
  gh,
  repo,
  pr,
  closed,
  clean = keep
}: {
  gh: Pick<GitHub, "replyToReviewComment" | "listReviewThreads" | "resolveThread">;
  repo: string;
  pr: number;
  closed: readonly Finding[];
  clean?: Clean;
}): Promise<{ replied: number; resolved: number }> {
  const withInline = closed.filter(
    (f): f is Finding & { inline: number } => typeof f.inline === "number"
  );
  if (!withInline.length) return { replied: 0, resolved: 0 };
  let replied = 0;
  for (const f of withInline) {
    const why = f.why ? `: ${f.why}` : "";
    await gh.replyToReviewComment(
      repo,
      pr,
      f.inline,
      clean(
        `**${f.id} ${f.status}** in \`${f.closed ?? ""}\`${why}\n<!-- oxy-review:reply ${f.id} -->`
      )
    );
    replied++;
  }
  let resolved = 0;
  const threads = await gh.listReviewThreads(repo, pr).catch(() => []);
  for (const f of withInline) {
    const t = threads.find((x) => x.firstComment === f.inline && !x.resolved);
    if (!t) continue;
    const ok = await gh.resolveThread(t.id).then(
      () => true,
      () => false
    );
    if (ok) resolved++;
  }
  return { replied, resolved };
}

/** Create the summary comment, or edit the one this PR already has. */
export async function upsertSummary({
  gh,
  repo,
  pr,
  existing,
  body
}: {
  gh: Pick<GitHub, "createIssueComment" | "updateIssueComment">;
  repo: string;
  pr: number;
  existing: IssueComment | null;
  body: string;
}): Promise<unknown> {
  if (existing?.id !== undefined) return gh.updateIssueComment(repo, existing.id, body);
  return gh.createIssueComment(repo, pr, body);
}

/** Fold the previous reviewer's verdict comments (the `claude` app), once. */
export async function minimizeLegacy({
  gh,
  comments
}: {
  gh: Pick<GitHub, "minimizeComment">;
  comments: readonly IssueComment[];
}): Promise<number> {
  const legacy = comments.filter(
    (c) =>
      c.user?.login.replace(/\[bot\]$/, "") === "claude" &&
      LEGACY_VERDICT.test(c.body ?? "") &&
      c.node_id
  );
  for (const c of legacy) if (c.node_id) await gh.minimizeComment(c.node_id).catch(() => undefined);
  return legacy.length;
}
