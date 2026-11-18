// Weekly review metrics: one record per merged PR, then the aggregate the report prints.
// Pure: metrics.ts does the fetching.

import type * as Config from "../config.ts";
import type { Escape } from "./escapes.ts";
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
  /** What each pass cost over every round of this PR, by pass name. */
  passes?: Record<string, PassTotals>;
  /** Observe mode: whether the PR body says how the change was verified. */
  verification?: Verification;
  /** Set when this PR reverts an earlier one; `of` is that PR's number when the text names it. */
  revert?: { of: number | null } | null;
}

export type Verification = "stated" | "empty";

/** A record from before the stamp was kept counts as empty. */
const verificationOf = (r: PrRecord): Verification => r.verification ?? "empty";

const HTML_COMMENT = /<!--[\s\S]*?-->/g;
const VERIFIED_HEADING = /^##(?!#)[ \t]+how it was verified[ \t]*$/im;
const NEXT_HEADING = /^##(?!#)[ \t]/m;
/** A bare non-answer: nothing after it, so it gives no reason. */
const BARE_NONE = /^(?:none|n\/a|na|nothing|-+|\u2013|\u2014|\.)\.?$/i;

/**
 * Whether a PR body states how the change was verified. Observe mode: this reports, it
 * never gates. HTML comments are stripped first (the template's placeholder is one). The
 * section is "empty" when it is missing, blank, or only a bare `none` / `n/a` / `-`.
 * `None -- needs a live account; covered by X` is "stated": it names a reason, which is
 * the point of the stamp. A bare `none` is an honest "unverified" and counts as empty.
 */
export function verificationState(body: string | null | undefined): Verification {
  const text = (body ?? "").replace(HTML_COMMENT, "");
  const m = VERIFIED_HEADING.exec(text);
  if (!m) return "empty";
  const rest = text.slice(m.index + m[0].length);
  const end = rest.search(NEXT_HEADING);
  const section = (end < 0 ? rest : rest.slice(0, end)).trim();
  return section && !BARE_NONE.test(section) ? "stated" : "empty";
}

const REVERT_TITLE = /^revert(?:\([^)]*\))?!?:|^revert "/i;
const REVERT_BODY = /\b(?:reverts|this reverts)\s+(?:[\w.-]+\/[\w.-]+)?#(\d+)/i;
const QUOTED_TITLE = /^revert "(.*)"(?:\s*\(#\d+\))?\s*$/i;
const TITLE_PR_REF = /\(#(\d+)\)\s*$/;

/**
 * Whether a merged PR reverts an earlier one: GitHub's `Revert "..."` title or a
 * conventional `revert:` one. `of` is the reverted PR's number from the body
 * (`Reverts owner/repo#123`, `This reverts #123`) or the `(#123)` in the quoted title,
 * or null when the text does not name one.
 */
export function detectRevert(
  title: string,
  body: string | null | undefined,
  self?: number
): { of: number | null } | null {
  const t = title.trim();
  if (!REVERT_TITLE.test(t)) return null;
  const fromBody = REVERT_BODY.exec(body ?? "")?.[1];
  const quoted = QUOTED_TITLE.exec(t)?.[1] ?? "";
  const fromTitle = TITLE_PR_REF.exec(quoted)?.[1];
  const n = Number(fromBody ?? fromTitle ?? NaN);
  return { of: Number.isFinite(n) && n !== self ? n : null };
}

/**
 * One pass's spend, summed. The token fields count only `metered` runs: rounds recorded
 * before usage was kept carry a cost and no tokens.
 */
export interface PassTotals {
  runs: number;
  cost: number;
  metered: number;
  input: number;
  cacheWrite: number;
  cacheRead: number;
  output: number;
}

const noSpend = (): PassTotals => ({
  runs: 0,
  cost: 0,
  metered: 0,
  input: 0,
  cacheWrite: 0,
  cacheRead: 0,
  output: 0
});

/** Add one pass run, or one PR's totals, into the entry for `name`. */
function addSpend(out: Record<string, PassTotals>, name: string, add: PassTotals): void {
  const into = out[name] ?? noSpend();
  for (const k of Object.keys(into) as (keyof PassTotals)[]) into[k] += add[k];
  out[name] = into;
}

/** Per-pass spend across rounds: the split the cost levers are chosen from. */
export function passTotals(rounds: readonly Round[]): Record<string, PassTotals> {
  const out: Record<string, PassTotals> = {};
  for (const r of rounds)
    for (const [name, p] of Object.entries(r.passes ?? {})) {
      const u = p.usage;
      addSpend(out, name, {
        runs: 1,
        cost: p.cost,
        metered: u ? 1 : 0,
        input: u?.input ?? 0,
        cacheWrite: u?.cacheWrite ?? 0,
        cacheRead: u?.cacheRead ?? 0,
        output: u?.output ?? 0
      });
    }
  return out;
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
): Pick<PrRecord, "cost" | "candidates" | "rejected" | "rounds" | "ms" | "passes"> {
  const sum = (pick: (r: Round) => number | undefined) =>
    rounds.reduce((n, r) => n + (pick(r) ?? 0), 0);
  const ran = rounds.filter((r) => r.cost > 0);
  return {
    cost: sum((r) => r.cost),
    candidates: sum((r) => r.candidates),
    rejected: sum((r) => r.rejected),
    rounds: ran.length,
    ms: ran.map((r) => r.ms),
    passes: passTotals(rounds)
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
    highRisk,
    verification: verificationState(pr.body),
    revert: detectRevert(pr.title, pr.body, pr.number)
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

/** `a` as a percentage of `b`, to `digits` decimals; null when `b` is 0. */
const pct = (a: number, b: number, digits = 0): number | null =>
  b ? Math.round((100 * 10 ** digits * a) / b) / 10 ** digits : null;

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
  /** Spend by pass name, summed over every round of every PR in the window. */
  byPass: Record<string, PassTotals>;
  size: { median: number | null; over1000: number };
  people: {
    approved: number;
    highRisk: { number: number; areas: string[]; approved: boolean }[];
    overrides: number[];
  };
  recurring: { rule: string; prs: number[] }[];
  /** Guardrails: did the time saved turn into breakage? */
  guardrails: {
    verification: { emptyPct: number | null; emptyPrs: number[] };
    reverts: { ratePct: number | null; prs: number[]; reverted: number[] };
  };
}

export interface GuardrailCrossCut {
  /** Source PRs of the possible escaped bugs, split by their verification section. */
  escaped: { stated: number; empty: number };
  /** Reverted PRs that merged in the window, split the same way. */
  reverted: { stated: number; empty: number };
}

const split = (nums: Iterable<number>, by: Map<number, Verification>) => {
  const out = { stated: 0, empty: 0 };
  for (const n of nums) {
    const v = by.get(n);
    if (v) out[v]++;
  }
  return out;
};

/** The escaped-bug source PRs and reverted PRs, by whether they stated a verification. */
export function guardrailCrossCut(
  records: readonly PrRecord[],
  escapes: readonly Escape[]
): GuardrailCrossCut {
  const by = new Map(records.map((r) => [r.number, verificationOf(r)] as const));
  const sources = new Set(
    escapes.flatMap((e) => (!e.broad && e.flagged === false && !e.unreviewed ? [e.intro] : []))
  );
  const reverted = new Set(records.flatMap((r) => (r.revert?.of ? [r.revert.of] : [])));
  return { escaped: split(sources, by), reverted: split(reverted, by) };
}

function guardrails(records: readonly PrRecord[]): Aggregate["guardrails"] {
  const empty = records.filter((r) => verificationOf(r) === "empty");
  const reverts = records.filter((r) => r.revert);
  return {
    verification: {
      emptyPct: pct(empty.length, records.length),
      emptyPrs: empty.map((r) => r.number)
    },
    reverts: {
      ratePct: pct(reverts.length, records.length, 1),
      prs: reverts.map((r) => r.number),
      reverted: reverts.flatMap((r) => (r.revert?.of ? [r.revert.of] : []))
    }
  };
}

function sumPasses(records: readonly PrRecord[]): Record<string, PassTotals> {
  const out: Record<string, PassTotals> = {};
  for (const r of records)
    for (const [name, t] of Object.entries(r.passes ?? {})) addSpend(out, name, t);
  return out;
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
    byPass: sumPasses(v2),
    size: { median: quantile(lines, 0.5), over1000: records.filter((r) => r.lines > 1000).length },
    people: {
      approved: records.filter((r) => r.approved).length,
      highRisk: records
        .filter((r) => r.highRisk.length)
        .map((r) => ({ number: r.number, areas: r.highRisk, approved: r.approved })),
      overrides: records.filter((r) => r.override).map((r) => r.number)
    },
    recurring: recurringRules(v2),
    guardrails: guardrails(records)
  };
}
