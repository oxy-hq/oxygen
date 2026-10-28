import type { CheckItem } from "@/pages/ide/Experiments/components/glance/CheckList";
import { complianceLabel, effectFormat } from "@/pages/ide/Experiments/format";
import { grainOf, periods } from "@/pages/ide/Experiments/grain";
import type { ResultResponse } from "@/types/experiment";

/** The engine's reason for a named unit the panel holds no rows for. */
const NO_ROWS = /with no rows in the panel/;

const FALLBACK_WARNING = "This number measures the attempt, not the lever.";

/** One line of the details checklist; its lines are the server's sentences, verbatim. */
export type DetailCheck = CheckItem & { lines: string[] };

type Substitution = ResultResponse["report"]["substitution"];

/** A detected substitution states what was taken; the floor only matters when none was seen. */
function substitutionCheck(s: Substitution, fmt: ReturnType<typeof effectFormat>): DetailCheck {
  const base = {
    id: "substitution",
    testId: "experiment-report-substitution-floor",
    title: "Substitution"
  };
  if (s.sibling === null) {
    return { ...base, status: "Not checked", tone: "neutral", lines: [s.statement] };
  }
  if (s.detected) {
    return { ...base, status: "Detected", tone: "warning", lines: [s.statement] };
  }
  return {
    ...base,
    status: `Floor ${fmt.plain(s.floor)}`,
    tone: "info",
    lines: [`${s.statement}${s.floor !== null ? ` (floor ${fmt.plain(s.floor)})` : ""}`]
  };
}

type Proposals = ResultResponse["report"]["proposals"];

/** How the split came to be: declared, written by hand, or drawn (and how many draws). */
function randomisationCheck(p: Proposals): DetailCheck {
  const base = {
    id: "randomisation",
    testId: "experiment-report-proposal-count",
    title: "Randomisation",
    lines: [p.message]
  };
  if (p.observational) {
    return { ...base, status: "Observational", tone: "warning" };
  }
  if (p.hand_assigned) {
    return { ...base, status: "Written by hand", tone: "warning" };
  }
  const n = p.count;
  return {
    ...base,
    status: n === 0 ? "Not priced by the platform" : `${n} earlier draw${n === 1 ? "" : "s"}`,
    tone: n === 0 ? "neutral" : "info"
  };
}

/**
 * The report's secondary detail as checks. Every sentence is the server's; this
 * adds only the title, the short status and its colour. Compliance appears only
 * when it is measured and below the floor (the shares are on the card's tile),
 * and the semantic-model note only when nothing was proposed and the server says why.
 */
export function detailChecks(result: ResultResponse): DetailCheck[] {
  const { report } = result;
  // Every figure here (the sibling's floor, a restated MDE) is in the measure's own
  // units, so a proportional report's % format does not apply.
  const fmt = effectFormat(report.design, grainOf(report.grain), { share: report.share });
  const out: DetailCheck[] = [];
  if (report.refusal === null) {
    out.push(substitutionCheck(report.substitution, fmt));
    const carry = report.carryover;
    if (carry) {
      const status = carry.lingered
        ? "Lingered"
        : carry.effect.refusal !== null
          ? "Not measured"
          : carry.effect.significant
            ? "Pull-forward"
            : "None measurable";
      out.push({
        id: "carryover",
        testId: "experiment-report-carryover",
        title: `After the lever turned off (${periods(carry.days, grainOf(report.grain))})`,
        status,
        tone: carry.lingered ? "success" : carry.effect.significant ? "warning" : "neutral",
        lines: [carry.statement]
      });
    }
    if (!report.promise && report.mde_note) {
      out.push({
        id: "promise",
        testId: "experiment-report-promise",
        title: "Promised effect size",
        status: "MDE unavailable",
        tone: "neutral",
        lines: [report.mde_note]
      });
    }
    if (report.promise) {
      const below = report.promise.below_promise;
      out.push({
        id: "promise",
        testId: "experiment-report-promise",
        title: "Promised effect size",
        status: below ? "Below the promised size" : "Clears the promised size",
        tone: below ? "warning" : "success",
        lines: [report.promise.message]
      });
    }
  }
  const c = report.compliance;
  if (complianceLabel(c).measured && c.below_floor) {
    const restated =
      c.restated ??
      (c.restated_mde === null
        ? null
        : `At this compliance the design could only have read a lever effect of about ${fmt.plain(c.restated_mde)}.`);
    out.push({
      id: "compliance",
      testId: "experiment-report-compliance",
      title: "Compliance",
      status: "Below the floor",
      tone: "warning",
      lines: [c.warning ?? FALLBACK_WARNING, ...(restated ? [restated] : [])]
    });
  }
  const pre = report.checks.pre_trend;
  out.push({
    id: "pre-trend",
    testId: "experiment-report-pre-trend",
    title: "Parallel-trends pre-check",
    status: pre === null ? "Not run" : pre.diverged ? "Diverged" : "No divergence",
    tone: pre === null ? "neutral" : pre.diverged ? "warning" : "success",
    lines: [report.checks.pre_trend_note]
  });
  const dropped = report.checks.dropped_waves;
  const units = report.checks.dropped_units ?? [];
  // A unit the measure returned no rows for (usually one its filter excludes) was
  // never in the data, so it is counted apart from the units the estimate left out.
  const noRows = units.filter((u) => NO_ROWS.test(u)).length;
  const left = units.length - noRows;
  const parts = [
    ...(left > 0 ? [`${left} unit${left === 1 ? "" : "s"} dropped`] : []),
    ...(noRows > 0 ? [`${noRows} with no data`] : []),
    ...(dropped.length > 0 ? [`${dropped.length} dropped`] : [])
  ];
  out.push({
    id: "dropped",
    testId: "experiment-report-dropped-waves",
    title: units.length > 0 ? "Dropped units, waves or pairs" : "Dropped waves or pairs",
    status: parts.length === 0 ? "None" : parts.join(", "),
    tone: parts.length === 0 ? "success" : "warning",
    lines: [...groupDroppedUnits(units), ...dropped]
  });
  out.push(randomisationCheck(report.proposals));
  const wb = result.write_back;
  if (!(wb.available && wb.edit) && wb.reason) {
    out.push({
      id: "semantic-model",
      testId: "experiment-report-no-writeback",
      title: "Semantic model",
      status: "Nothing proposed",
      tone: "neutral",
      lines: [`Nothing is proposed for the semantic model: ${wb.reason}`]
    });
  }
  return out;
}

/**
 * `unit: reason` lines, with every reason shared by more than three units folded
 * into one line that names a few. A panel can drop a hundred control units for the
 * same missing data; listing each one buries the reasons that differ.
 */
export function groupDroppedUnits(units: string[]): string[] {
  const byReason = new Map<string, string[]>();
  for (const line of units) {
    const at = line.indexOf(": ");
    const [unit, reason] = at === -1 ? [line, ""] : [line.slice(0, at), line.slice(at + 2)];
    byReason.set(reason, [...(byReason.get(reason) ?? []), unit]);
  }
  return [...byReason].flatMap(([reason, names]) => {
    if (names.length <= 3 || reason === "")
      return names.map((n) => (reason ? `${n}: ${reason}` : n));
    const named = `${names.slice(0, 3).join(", ")} and ${names.length - 3} more`;
    return [`${names.length} units: ${reason} (${named})`];
  });
}

/** How many checks want a reader's attention. */
export const toReview = (checks: DetailCheck[]) =>
  checks.filter((c) => c.tone === "warning" || c.tone === "danger").length;
