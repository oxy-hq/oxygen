// The review's memory between pushes, carried base64-encoded in the summary comment.
// It is also the record the weekly metrics read, so its fields are a contract.

import type {
  Candidate,
  ClosedStatus,
  Finding,
  FindingStatus,
  IssueComment,
  RecheckResult,
  ReviewState
} from "./types.ts";

const STATE_RE = /<!-- oxy-review:state ([A-Za-z0-9+/=]+) -->/g;
const MAX_ROUNDS = 20;
const MAX_FINDINGS = 80;

export const CLOSED: readonly ClosedStatus[] = ["fixed", "withdrawn", "obsolete"];

export const isClosed = (s: FindingStatus): s is ClosedStatus => s !== "open";

export function encodeState(state: ReviewState): string {
  const json = JSON.stringify(state);
  return `<!-- oxy-review:state ${Buffer.from(json, "utf8").toString("base64")} -->`;
}

/**
 * The state carried in a comment body, or null when absent, malformed or another version.
 * The last block in the body: the renderer writes it after everything the model said, so
 * a state-shaped string in a finding or a reason cannot stand in for it.
 */
export function decodeState(body: string | null | undefined, version: number): ReviewState | null {
  const m = [...(body ?? "").matchAll(STATE_RE)].at(-1);
  if (!m?.[1]) return null;
  try {
    const state = JSON.parse(
      Buffer.from(m[1], "base64").toString("utf8")
    ) as Partial<ReviewState> | null;
    return state?.v === version ? (state as ReviewState) : null;
  } catch {
    return null;
  }
}

/**
 * The newest comment carrying the summary marker, posted by one of `authors` (the
 * workflow's own bot). Anyone can paste the marker into a comment; only the workflow
 * can post as its bot, so a hand-written state cannot pass the gate.
 */
export function findSummaryComment<C extends IssueComment>(
  comments: readonly C[],
  marker: string,
  authors: readonly string[]
): C | null {
  const byReviewer = (c: C) => authors.includes((c.user?.login ?? "").replace(/\[bot\]$/, ""));
  return comments.filter((c) => (c.body ?? "").includes(marker) && byReviewer(c)).at(-1) ?? null;
}

export function emptyState(pr: number, version: number): ReviewState {
  return {
    v: version,
    pr,
    head: null,
    base: null,
    status: "none",
    verdict: null,
    findings: [],
    possible: [],
    rounds: [],
    next: 1
  };
}

export const openFindings = (state: Pick<ReviewState, "findings">): Finding[] =>
  state.findings.filter((f) => !isClosed(f.status));

/** Apply the recheck pass's verdicts to findings that were open. */
export function applyRecheck(
  findings: readonly Finding[],
  results: readonly RecheckResult[],
  sha: string
): Finding[] {
  const byId = new Map(results.map((r) => [r.id, r]));
  return findings.map((f) => {
    const r = byId.get(f.id);
    if (!r || isClosed(f.status) || !isClosed(r.status)) return f;
    return { ...f, status: r.status, closed: sha.slice(0, 9), why: r.reason.slice(0, 300) };
  });
}

/** Give new findings stable ids (F<n>) and append them as open. */
export function addFindings(
  state: Pick<ReviewState, "findings" | "next">,
  findings: readonly Candidate[],
  sha: string
): { findings: Finding[]; added: Finding[]; next: number } {
  let next = state.next;
  const added = findings.map(
    (f): Finding => ({
      id: `F${next++}`,
      title: f.title,
      file: f.file,
      line: f.line,
      end_line: f.end_line,
      severity: f.severity,
      category: f.category,
      confidence: f.confidence,
      lens: f.sources[0] ?? "general",
      sources: f.sources,
      rule: f.rule,
      status: "open",
      first: sha.slice(0, 9),
      reasoning: f.reasoning,
      fix: f.fix
    })
  );
  return { findings: [...state.findings, ...added], added, next };
}

/**
 * Keep the encoded state well under GitHub's 65,536-character comment limit: open
 * findings keep a trimmed explanation (the next round renders them again), closed ones
 * keep only what the metrics count.
 */
export function compactForStorage(
  state: ReviewState,
  { lean = false }: { lean?: boolean } = {}
): ReviewState {
  const findings = state.findings.slice(-MAX_FINDINGS).map((f): Finding => {
    if (lean || isClosed(f.status)) {
      const { reasoning: _reasoning, fix: _fix, ...rest } = f;
      return rest;
    }
    return { ...f, reasoning: (f.reasoning ?? "").slice(0, 700), fix: (f.fix ?? "").slice(0, 400) };
  });
  return { ...state, findings, rounds: state.rounds.slice(-MAX_ROUNDS) };
}
