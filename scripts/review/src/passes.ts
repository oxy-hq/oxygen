// Scheduling model passes under one dollar cap, and folding their answers together.

import { dedupFindings, normalizeFinding, severityRank } from "./findings.ts";
import type {
  Candidate,
  FindingsOutput,
  LensPick,
  PassResult,
  RawFinding,
  Skipped,
  VerifyVerdict
} from "./types.ts";

/** Dollars reserved before a pass starts, settled when it ends. */
export class Budget {
  readonly total: number;
  readonly overshoot: number;
  reserved = 0;
  spent = 0;

  constructor(total: number, overshoot: number) {
    this.total = total;
    this.overshoot = overshoot;
  }

  tryReserve(cap: number): boolean {
    const need = cap + this.overshoot;
    if (this.spent + this.reserved + need > this.total + 1e-9) return false;
    this.reserved += need;
    return true;
  }

  settle(cap: number, cost: number): void {
    this.reserved -= cap + this.overshoot;
    this.spent += cost;
  }
}

export type Scheduled =
  | { kind: "general"; cap: number }
  | { kind: "recheck"; cap: number }
  | { kind: "lens"; lens: LensPick; cap: number };

export interface Caps {
  general: number;
  recheck: number;
  lens: number;
}

/**
 * Decide which passes run: general always, recheck when there are open findings to
 * recheck, then lenses in priority order while the budget holds.
 */
export function schedulePasses({
  lenses,
  needRecheck,
  budget,
  caps
}: {
  lenses: readonly LensPick[];
  needRecheck: boolean;
  budget: Budget;
  caps: Caps;
}): { run: Scheduled[]; skipped: Skipped[] } {
  const run: Scheduled[] = [];
  const skipped: Skipped[] = [];
  if (!budget.tryReserve(caps.general))
    throw new Error(`budget $${budget.total} cannot cover the general pass`);
  run.push({ kind: "general", cap: caps.general });
  if (needRecheck) {
    if (budget.tryReserve(caps.recheck)) run.push({ kind: "recheck", cap: caps.recheck });
    else
      skipped.push({ id: "recheck", reason: "no budget left; earlier findings stay as they were" });
  }
  for (const lens of lenses) {
    if (budget.tryReserve(caps.lens)) run.push({ kind: "lens", lens, cap: caps.lens });
    else skipped.push({ id: lens.id, reason: `no budget left under the $${budget.total} cap` });
  }
  return { run, skipped };
}

/** Normalised candidates from every finder pass that finished, merged across passes. */
export function collectCandidates(results: readonly PassResult[]): {
  candidates: Candidate[];
  notReviewed: string[];
} {
  const raw: Candidate[] = [];
  const notReviewed: string[] = [];
  for (const r of results) {
    if (!r.ok || r.name === "recheck" || r.name === "verify") continue;
    const output = r.output as Partial<FindingsOutput> | null;
    for (const f of output?.findings ?? []) raw.push(normalizeFinding(f as RawFinding, r.name));
    for (const n of output?.not_reviewed ?? []) notReviewed.push(`${n} (${r.name})`);
  }
  const candidates = dedupFindings(raw.filter((f) => f.file && f.title)).map((c, i) => ({
    ...c,
    cid: `C${i + 1}`
  }));
  return { candidates, notReviewed };
}

/** The candidate without its suggestion: one nobody vouched for is never posted. */
const unsuggested = ({ suggestion: _suggestion, ...c }: Candidate): Candidate => c;

export interface Verified {
  confirmed: Candidate[];
  possible: Candidate[];
  rejected: Candidate[];
}

/**
 * Apply the verifier's verdicts. Without a verifier run, only high-confidence
 * candidates are posted and the rest are shown as possible.
 */
export function applyVerification(
  candidates: readonly Candidate[],
  verdicts: readonly VerifyVerdict[] | null
): Verified {
  const out: Verified = { confirmed: [], possible: [], rejected: [] };
  if (!verdicts) {
    for (const c of candidates) {
      (c.confidence >= 0.8 ? out.confirmed : out.possible).push({
        ...unsuggested(c),
        unverified: true,
        why: "not verified: the verifier did not run"
      });
    }
    return out;
  }
  const byId = new Map(verdicts.map((v) => [v.id, v]));
  const sources = new Map(candidates.map((c) => [c.cid, [...c.sources]]));
  // Duplicates first, so the finding they fold into carries every pass that saw it.
  const dups = new Set<string | undefined>();
  for (const c of candidates) {
    const into = c.cid === undefined ? undefined : byId.get(c.cid)?.duplicate_of;
    const target = into === undefined || into === c.cid ? undefined : sources.get(into);
    if (!target) continue;
    dups.add(c.cid);
    for (const s of c.sources) if (!target.includes(s)) target.push(s);
  }
  for (const c of candidates) {
    if (dups.has(c.cid)) continue;
    const v = c.cid === undefined ? undefined : byId.get(c.cid);
    const verdict = v?.verdict ?? "uncertain";
    const severity = v?.severity ?? c.severity;
    const bare = unsuggested(c);
    if (verdict === "confirmed") {
      const kept = v?.suggestion_ok === true ? c : bare;
      out.confirmed.push({ ...kept, severity, sources: sources.get(c.cid) ?? c.sources });
    } else if (verdict === "rejected") out.rejected.push({ ...bare, why: v?.reason ?? "" });
    else if (severityRank(severity) >= severityRank("should_fix"))
      out.possible.push({ ...bare, severity, why: v?.reason ?? "no verdict" });
    else out.rejected.push({ ...bare, why: `uncertain nit: ${v?.reason ?? ""}` });
  }
  return out;
}
