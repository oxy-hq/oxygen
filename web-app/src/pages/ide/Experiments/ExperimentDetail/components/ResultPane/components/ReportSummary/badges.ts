import type { Tone } from "@/pages/ide/Experiments/components/glance/tone";
import { complianceLabel, formatShare } from "@/pages/ide/Experiments/format";
import { predictionVerdict } from "@/pages/ide/Experiments/prediction";
import type { ReportView } from "@/types/experiment";

export interface ReportBadge {
  id: string;
  tone: Tone;
  label: string;
}

/** The run's standing. The label is a plain name; the server's own sentence sits beside it. */
export function runBadge(kind: ReportView["status"]["kind"]): ReportBadge {
  switch (kind) {
    case "aborted":
      return { id: "run", tone: "danger", label: "Aborted run" };
    case "retrospective":
      return { id: "run", tone: "warning", label: "Retrospective" };
    case "registered":
      return { id: "run", tone: "success", label: "Registered result" };
    case "interim":
      return { id: "run", tone: "warning", label: "Interim, not final" };
    default: {
      const unreachable: never = kind;
      return unreachable;
    }
  }
}

/** What the verdict reads from a report: the full report, or a list row's glance of it. */
type VerdictFacts = Pick<ReportView, "directional"> & {
  headline: { significant: boolean } | null;
};

/**
 * The verdict's colour. The words are always the server's `verdict`; `significant`
 * and `directional` only pick a colour: clears a strict α → success, clears a
 * looser registered α → caution, misses → neutral.
 */
export function verdictTone(report: VerdictFacts): Tone {
  const h = report.headline;
  if (!h?.significant) return "neutral";
  return report.directional ? "warning" : "success";
}

/** Every caveat that changes how far the number can be trusted, as a coloured pill. */
export function caveatBadges(report: ReportView): ReportBadge[] {
  const out: ReportBadge[] = [];
  const c = report.compliance;
  const compliance = complianceLabel(c);
  if (!compliance.measured) {
    out.push({ id: "compliance", tone: "neutral", label: compliance.text });
  } else if (c.below_floor) {
    out.push({
      id: "compliance",
      tone: "warning",
      label: `Compliance ${compliance.text}, below the ${formatShare(c.floor)} floor`
    });
  } else {
    out.push({ id: "compliance", tone: "success", label: `Compliance ${compliance.text}` });
  }
  if (report.direction.mismatch) {
    out.push({ id: "direction", tone: "danger", label: "Direction differs from the prediction" });
  }
  if (report.prediction) {
    const v = predictionVerdict(report.prediction);
    if (v === "agrees") out.push({ id: "prior", tone: "success", label: "Prior agrees" });
    if (v === "contradicts") out.push({ id: "prior", tone: "danger", label: "Prior contradicted" });
  }
  if (report.checks.pre_trend?.diverged) {
    out.push({ id: "pre-trend", tone: "warning", label: "Pre-trend diverged" });
  }
  if (report.projection?.warning) {
    out.push({ id: "bias", tone: "warning", label: "Projection likely biased" });
  }
  if (report.lever_note) {
    out.push({ id: "lever", tone: "warning", label: "Lever changed mid-run" });
  }
  return out;
}

/** The verdict in plain words, from the same colour rule. The server's exact wording goes on hover. */
export function plainVerdict(report: VerdictFacts & Pick<ReportView, "status">): string {
  // An interim reading is a peek, never a verdict: every word says "so far".
  if (report.status.kind === "interim") {
    switch (verdictTone(report)) {
      case "success":
        return "Looks significant so far";
      case "warning":
        return "Looks weakly significant so far";
      default:
        return "Not distinguishable so far";
    }
  }
  switch (verdictTone(report)) {
    case "success":
      return "Strong evidence";
    case "warning":
      return "Weak evidence";
    default:
      return "No clear effect";
  }
}

/** The confidence level a registered α sets: `0.05` → "95%". Null when α is not a usable share. */
export function confidencePct(alpha: number): string | null {
  if (!Number.isFinite(alpha) || alpha <= 0 || alpha >= 1) return null;
  return `${Number(((1 - alpha) * 100).toPrecision(3))}%`;
}

/** α as a reader would say it: `0.05` → "95% confidence: a 1 in 20 chance of a false alarm". */
export function alphaInWords(alpha: number): string {
  const pct = confidencePct(alpha);
  if (pct === null) return `α = ${alpha}`;
  return `${pct} confidence: a 1 in ${Math.round(1 / alpha)} chance of a false alarm`;
}
