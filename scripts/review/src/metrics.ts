// Weekly review metrics: one record per merged PR, then the aggregate the report prints.
// Pure: metrics.ts does the fetching.

import type * as Config from "../config.ts";
import { isBot, threadReplies } from "./feedback.ts";
import { matchesAny } from "./globs.ts";
import { decodeState, findSummaryComment } from "./state.ts";
import type {
  Category,
  FindingStatus,
  IssueComment,
  PrMeta,
  PrReview,
  ReviewComment,
  ReviewStatus,
  Round,
  Severity,
  Verdict
} from "./types.ts";

type MetricsConfig = Pick<
  typeof Config,
  | "SUMMARY_MARKER"
  | "REVIEWER_LOGINS"
  | "STATE_VERSION"
  | "BOT_LOGINS"
  | "HIGH_RISK"
  | "OVERRIDE_LABEL"
>;

const LEGACY_VERDICT =
  /\b(LGTM with nits|LGTM|Needs changes|Blocking|No actionable issues found)\b/;

export interface RecordFinding {
  id: string;
  title: string;
  file: string;
  line: number;
  severity: Severity;
  category: Category;
  lens: string;
  rule: string | null;
  status: FindingStatus;
  /** Comments by anyone but the reviewer: replies in its thread, PR comments naming its id. */
  replies: number;
  /** Its inline comment offered a one-click suggestion. */
  suggested: boolean;
}

export interface PrRecord {
  number: number;
  title: string;
  author: string;
  mergedAt: string | null | undefined;
  lines: number;
  override: boolean;
  approved: boolean;
  highRisk: string[];
  review: "v2" | "legacy" | "none";
  reviewedHead: boolean;
  verdict: Verdict | string | null;
  findings: RecordFinding[];
  status?: ReviewStatus;
  possible?: number;
  cost?: number;
  candidates?: number;
  rejected?: number;
  rounds?: number;
  ms?: number[];
}

function legacyReview(
  comments: readonly IssueComment[],
  lastCommitAt: string | null
): { verdict: string; reviewedHead: boolean } | null {
  const verdicts = comments.filter(
    (c) => c.user?.login.replace(/\[bot\]$/, "") === "claude" && LEGACY_VERDICT.test(c.body ?? "")
  );
  const last = verdicts.at(-1);
  if (!last) return null;
  return {
    verdict: (last.body ?? "").match(LEGACY_VERDICT)?.[1] ?? "unknown",
    reviewedHead: !lastCommitAt || (last.created_at ?? "") >= lastCommitAt
  };
}

function rollup(
  rounds: readonly Round[]
): Pick<PrRecord, "cost" | "candidates" | "rejected" | "rounds" | "ms"> {
  const sum = (pick: (r: Round) => number | undefined) =>
    rounds.reduce((n, r) => n + (pick(r) ?? 0), 0);
  const ran = rounds.filter((r) => r.cost > 0);
  return {
    cost: sum((r) => r.cost),
    candidates: sum((r) => r.candidates),
    rejected: sum((r) => r.rejected),
    rounds: ran.length,
    ms: ran.map((r) => r.ms)
  };
}

export interface RecordInput {
  pr: PrMeta;
  comments: readonly IssueComment[];
  reviewComments: readonly ReviewComment[];
  reviews: readonly PrReview[];
  files: readonly string[];
  lastCommitAt: string | null;
  config: MetricsConfig;
}

/** Everything the report needs about one merged PR. */
export function prRecord({
  pr,
  comments,
  reviewComments,
  reviews,
  files,
  lastCommitAt,
  config
}: RecordInput): PrRecord {
  const summary = findSummaryComment(comments, config.SUMMARY_MARKER, config.REVIEWER_LOGINS);
  const state = decodeState(summary?.body, config.STATE_VERSION);
  const labels = pr.labels.map((l) => l.name);
  const author = pr.user.login;
  const approvals = reviews.filter(
    (r) => r.state === "APPROVED" && r.user?.login !== author && !isBot(r.user, config.BOT_LOGINS)
  );
  const highRisk = config.HIGH_RISK.filter((a) => files.some((f) => matchesAny(f, a.paths))).map(
    (a) => a.id
  );
  const base = {
    number: pr.number,
    title: pr.title,
    author,
    mergedAt: pr.merged_at,
    lines: (pr.additions ?? 0) + (pr.deletions ?? 0),
    override: labels.includes(config.OVERRIDE_LABEL),
    approved: approvals.length > 0,
    highRisk
  };
  if (!state) {
    const legacy = legacyReview(comments, lastCommitAt);
    return {
      ...base,
      review: legacy ? "legacy" : "none",
      reviewedHead: legacy?.reviewedHead ?? false,
      verdict: legacy?.verdict ?? null,
      findings: []
    };
  }
  const replies = threadReplies({
    issueComments: comments,
    reviewComments,
    findings: state.findings,
    reviewerLogins: config.REVIEWER_LOGINS
  });
  const findings = state.findings.map(
    (f): RecordFinding => ({
      id: f.id,
      title: f.title,
      file: f.file,
      line: f.line,
      severity: f.severity,
      category: f.category,
      lens: f.lens,
      rule: f.rule,
      status: f.status,
      replies: replies.get(f.id)?.length ?? 0,
      suggested: f.suggested === true
    })
  );
  return {
    ...base,
    review: "v2",
    reviewedHead: state.head === pr.head.sha,
    status: state.status,
    verdict: state.verdict,
    possible: state.possible.length,
    findings,
    ...rollup(state.rounds)
  };
}

const pct = (a: number, b: number): number | null => (b ? Math.round((100 * a) / b) : null);

function quantile(values: readonly number[], q: number): number | null {
  if (!values.length) return null;
  const s = [...values].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.floor(q * s.length))] ?? null;
}

export interface Bucket {
  n: number;
  addressed: number;
  withdrawn: number;
  open: number;
  /** Findings someone answered: a reply in the thread or a PR comment naming the id. */
  replied: number;
}

/**
 * Outcomes, not ratings. Uber's "addressed" is "no longer reproduced on the final
 * commit": here fixed or obsolete. Withdrawn means the reviewer conceded it was wrong.
 * Open means it was merged past: noise, or a risk someone accepted.
 */
function tally(findings: readonly RecordFinding[]): Bucket {
  const t: Bucket = { n: 0, addressed: 0, withdrawn: 0, open: 0, replied: 0 };
  for (const f of findings) {
    t.n++;
    if (f.status === "fixed" || f.status === "obsolete") t.addressed++;
    else if (f.status === "withdrawn") t.withdrawn++;
    else t.open++;
    t.replied += f.replies ? 1 : 0;
  }
  return t;
}

function bucket(
  findings: readonly RecordFinding[],
  key: (f: RecordFinding) => string
): Record<string, Bucket> {
  const groups = Map.groupBy(findings, key);
  return Object.fromEntries([...groups].map(([k, fs]) => [k, tally(fs)]));
}

function recurringRules(records: readonly PrRecord[]): { rule: string; prs: number[] }[] {
  const seen = new Map<string, Set<number>>();
  for (const r of records)
    for (const f of r.findings) {
      if (!f.rule) continue;
      const prs = seen.get(f.rule) ?? new Set<number>();
      prs.add(r.number);
      seen.set(f.rule, prs);
    }
  return [...seen]
    .filter(([, prs]) => prs.size >= 2)
    .map(([rule, prs]) => ({ rule, prs: [...prs] }))
    .sort((a, b) => b.prs.length - a.prs.length);
}

export interface Aggregate {
  prs: number;
  coverage: {
    v2: number;
    legacy: number;
    none: number;
    reviewedHead: number;
    mergedUnreviewed: number[];
    incomplete: number[];
  };
  findings: Bucket & {
    addressedPct: number | null;
    withdrawnPct: number | null;
    openPct: number | null;
    repliedPct: number | null;
  };
  byCategory: Record<string, Bucket>;
  bySeverity: Record<string, Bucket>;
  byLens: Record<string, Bucket>;
  /** Findings posted with a one-click suggestion, against the other should-fix and blocking ones. */
  suggestions: { with: Bucket; without: Bucket };
  verifier: { candidates: number; rejected: number };
  cost: { total: number; perPr: number; p50ms: number | null; p90ms: number | null };
  size: { median: number | null; over1000: number };
  people: {
    approved: number;
    highRisk: { number: number; areas: string[]; approved: boolean }[];
    overrides: number[];
  };
  recurring: { rule: string; prs: number[] }[];
}

/** The numbers the weekly report prints. */
export function aggregate(records: readonly PrRecord[]): Aggregate {
  const v2 = records.filter((r) => r.review === "v2");
  const findings = v2.flatMap((r) => r.findings);
  const totals = tally(findings);
  const ms = v2.flatMap((r) => r.ms ?? []);
  const lines = records.map((r) => r.lines);
  const cost = v2.reduce((n, r) => n + (r.cost ?? 0), 0);
  return {
    prs: records.length,
    coverage: {
      v2: v2.length,
      legacy: records.filter((r) => r.review === "legacy").length,
      none: records.filter((r) => r.review === "none").length,
      reviewedHead: records.filter((r) => r.reviewedHead).length,
      mergedUnreviewed: records.filter((r) => !r.reviewedHead).map((r) => r.number),
      incomplete: v2.filter((r) => r.status === "incomplete").map((r) => r.number)
    },
    findings: {
      ...totals,
      addressedPct: pct(totals.addressed, totals.n),
      withdrawnPct: pct(totals.withdrawn, totals.n),
      openPct: pct(totals.open, totals.n),
      repliedPct: pct(totals.replied, totals.n)
    },
    byCategory: bucket(findings, (f) => f.category),
    bySeverity: bucket(findings, (f) => f.severity),
    byLens: bucket(findings, (f) => f.lens),
    suggestions: {
      with: tally(findings.filter((f) => f.suggested)),
      without: tally(findings.filter((f) => !f.suggested && f.severity !== "nit"))
    },
    verifier: {
      candidates: v2.reduce((n, r) => n + (r.candidates ?? 0), 0),
      rejected: v2.reduce((n, r) => n + (r.rejected ?? 0), 0)
    },
    cost: {
      total: cost,
      perPr: v2.length ? cost / v2.length : 0,
      p50ms: quantile(ms, 0.5),
      p90ms: quantile(ms, 0.9)
    },
    size: { median: quantile(lines, 0.5), over1000: records.filter((r) => r.lines > 1000).length },
    people: {
      approved: records.filter((r) => r.approved).length,
      highRisk: records
        .filter((r) => r.highRisk.length)
        .map((r) => ({ number: r.number, areas: r.highRisk, approved: r.approved })),
      overrides: records.filter((r) => r.override).map((r) => r.number)
    },
    recurring: recurringRules(v2)
  };
}
