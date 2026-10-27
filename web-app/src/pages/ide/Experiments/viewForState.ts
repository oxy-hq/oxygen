import type { ExperimentStatus } from "@/types/experiment";

/** Which body the detail page renders. `unknown` is version skew, never a result. */
export type DetailView =
  | "pricing"
  | "registrable"
  | "operations"
  | "report"
  | "refused"
  | "unknown";

/** The one place a status becomes a view. */
export function viewForState(status: string): DetailView {
  switch (status as ExperimentStatus) {
    case "draft":
      return "pricing";
    case "registrable":
      return "registrable";
    case "registered":
    case "live":
    case "closing":
      return "operations";
    case "reported":
    case "aborted":
      return "report";
    case "registration_refused":
      return "refused";
    default:
      return "unknown";
  }
}

/** `/result` is requested only once the run has reported or aborted. */
export function mayFetchResult(status: string): boolean {
  return viewForState(status) === "report";
}

const LABELS: Record<ExperimentStatus, string> = {
  draft: "Draft",
  registrable: "Registrable",
  registration_refused: "Registration refused",
  registered: "Registered",
  live: "Live",
  closing: "Closing",
  reported: "Reported",
  aborted: "Aborted"
};

export function statusLabel(status: string): string {
  return LABELS[status as ExperimentStatus] ?? status;
}

type BadgeVariant = "default" | "secondary" | "destructive" | "outline";

const VARIANTS: Record<ExperimentStatus, BadgeVariant> = {
  draft: "outline",
  registrable: "secondary",
  registration_refused: "destructive",
  registered: "secondary",
  live: "default",
  closing: "default",
  reported: "secondary",
  aborted: "destructive"
};

export function badgeVariantFor(status: string): BadgeVariant {
  return VARIANTS[status as ExperimentStatus] ?? "outline";
}
