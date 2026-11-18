// Scoring a review against a golden case: did it find the known bug, at the right
// severity, and how much else did it say? And when it did not, at which stage of the
// pipeline was the bug lost?

import { severityRank } from "./findings.ts";
import type { Debug } from "./round.ts";
import type { Finding, Possible, Severity } from "./types.ts";

export interface Expected {
  file: string;
  line: number;
  tolerance?: number;
  severity?: Severity;
  what: string;
}

export interface GoldenCase {
  id: string;
  pr: number;
  base: string;
  head: string;
  source?: string;
  expect?: Expected[];
}

export type HitResult = "found" | "found-low-severity" | "possible-only" | "missed" | "error";

/**
 * How far the expected bug got before it was lost:
 * - posted: an open finding sits near it
 * - possible: the verifier left it uncertain, so it is shown but not posted
 * - filtered: it was confirmed, then dropped (confidence floor, suppressed category,
 *   nit cap, dedup against known findings) or folded away before posting
 * - rejected: some pass found it and the verifier rejected it
 * - never-found: no pass produced a candidate near it
 * - unknown: no debug data (result.json missing or empty), or the review errored
 */
const STAGES = ["posted", "possible", "filtered", "rejected", "never-found", "unknown"] as const;
export type Stage = (typeof STAGES)[number];

export interface Hit {
  what: string;
  result: HitResult;
  stage: Stage;
  as?: string;
}

/**
 * The slice of a round's result.json the stage check reads (`{}` for a skipped round),
 * plus whether the review was complete. When a pass failed or was skipped for budget, its
 * candidates are missing, so "no pass saw it" cannot be told from "the pass that would
 * have did not run".
 */
export type StageDebug = Partial<
  Pick<Debug, "candidates" | "confirmed" | "possible" | "rejected" | "dropped" | "known">
> & { complete: boolean };

export interface Score {
  id: string;
  pr: number;
  hits: Hit[];
  extra: string[];
  cleanFalseAlarms: number;
  cost: number;
  error?: string;
}

type Located = Pick<Finding, "file" | "line">;

const near = (f: Located, e: Expected): boolean =>
  f.file === e.file && Math.abs(f.line - e.line) <= (e.tolerance ?? 10);

/** Where a bug that did not reach the posted findings was lost, from the round's debug data. */
export function lostStage(e: Expected, debug: StageDebug | undefined): Stage {
  if (!debug?.candidates) return "unknown";
  const at = (list: readonly Located[] | undefined) => list?.some((x) => near(x, e)) ?? false;
  const droppedAt = (list: StageDebug["dropped"]) => at(list?.map((d) => d.finding));
  if (at(debug.possible)) return "possible";
  if (at(debug.confirmed) || droppedAt(debug.dropped) || droppedAt(debug.known)) return "filtered";
  if (at(debug.rejected)) return "rejected";
  // A candidate with no verdict trail was folded into a duplicate: lost after finding.
  if (at(debug.candidates)) return "filtered";
  return debug.complete ? "never-found" : "unknown";
}

export function scoreCase(
  kase: GoldenCase,
  {
    findings,
    possible,
    cost,
    debug
  }: {
    findings: readonly Pick<Finding, "file" | "line" | "severity" | "title">[];
    possible: readonly Pick<Possible, "file" | "line" | "title">[];
    cost: number;
    debug?: StageDebug | undefined;
  }
): Score {
  const expected = kase.expect ?? [];
  const matched = new Set<(typeof findings)[number]>();
  const hits = expected.map((e): Hit => {
    const f = findings.find((x) => near(x, e));
    if (f) {
      matched.add(f);
      return {
        what: e.what,
        stage: "posted",
        result:
          severityRank(f.severity) >= severityRank(e.severity ?? "nit")
            ? "found"
            : "found-low-severity",
        as: `${f.severity}: ${f.title}`
      };
    }
    const p = possible.find((x) => near(x, e));
    return p
      ? { what: e.what, result: "possible-only", stage: "possible", as: p.title }
      : { what: e.what, result: "missed", stage: lostStage(e, debug) };
  });
  const serious = findings.filter((f) => severityRank(f.severity) >= severityRank("should_fix"));
  return {
    id: kase.id,
    pr: kase.pr,
    hits,
    extra: findings
      .filter((f) => !matched.has(f))
      .map((f) => `${f.severity}: ${f.file}:${f.line} ${f.title}`),
    cleanFalseAlarms: expected.length === 0 ? serious.length : 0,
    cost
  };
}

export interface Summary {
  cases: number;
  expected: number;
  found: number;
  foundLow: number;
  possibleOnly: number;
  missed: number;
  recall: number | null;
  /** Share of hits with a known stage that some pass saw, before verification. */
  rawRecall: number | null;
  /** ...and that survived verification and the filters (an uncertain one counts: not rejected). */
  verifiedRecall: number | null;
  stages: Record<Stage, number>;
  cleanFalseAlarms: number;
  extraFindings: number;
  cost: number;
}

const SURVIVED: readonly Stage[] = ["posted", "possible"];
const SEEN: readonly Stage[] = ["posted", "possible", "filtered", "rejected"];

const pct = (n: number, of: number): number | null => (of ? Math.round((100 * n) / of) : null);

export function summarize(scores: readonly Score[]): Summary {
  const hits = scores.flatMap((s) => s.hits);
  const count = (r: HitResult) => hits.filter((h) => h.result === r).length;
  const inStages = (list: readonly Stage[]) => hits.filter((h) => list.includes(h.stage)).length;
  const known = hits.length - inStages(["unknown"]);
  return {
    cases: scores.length,
    expected: hits.length,
    found: count("found"),
    foundLow: count("found-low-severity"),
    possibleOnly: count("possible-only"),
    missed: count("missed"),
    recall: hits.length
      ? Math.round((100 * (count("found") + count("found-low-severity"))) / hits.length)
      : null,
    rawRecall: pct(inStages(SEEN), known),
    verifiedRecall: pct(inStages(SURVIVED), known),
    stages: Object.fromEntries(STAGES.map((st) => [st, inStages([st])])) as Record<Stage, number>,
    cleanFalseAlarms: scores.reduce((n, s) => n + s.cleanFalseAlarms, 0),
    extraFindings: scores.reduce((n, s) => n + s.extra.length, 0),
    cost: Math.round(scores.reduce((n, s) => n + s.cost, 0) * 100) / 100
  };
}
