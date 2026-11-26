import { AdminStatusPill } from "@/pages/admin/components/AdminStatusPill";
import type { AppAvailabilityReading } from "@/types/apps";
import {
  availabilityLabel,
  availabilityTone,
  formatObjective,
  formatWindow
} from "../availabilityTone";

/** One sentence saying what the verdict means and what follows from it. */
function explain(data: AppAvailabilityReading): string {
  const objective = formatObjective(data.objective);
  if (data.verdict === "no_opinion") {
    return "Not enough traffic to judge. Deliberately not reported as healthy — an app nobody has used has not been shown to work.";
  }
  if (data.verdict === "healthy") {
    return `Failures are within the ${objective} availability objective across every window.`;
  }
  const parts = [
    data.burn_rate !== undefined && `${data.burn_rate.toFixed(1)}× the ${objective} error budget`,
    data.long_window_minutes !== undefined &&
      `sustained over ${formatWindow(data.long_window_minutes)}`,
    data.short_window_minutes !== undefined &&
      `still failing in the last ${formatWindow(data.short_window_minutes)}`
  ].filter(Boolean) as string[];
  const consequence =
    data.severity === "page"
      ? "Reaches on-call via workspace health."
      : "Degraded only — this grade never pages.";
  return `${parts.join(" · ")}. ${consequence}`;
}

export const AvailabilityVerdict = ({ data }: { data: AppAvailabilityReading }) => (
  <div
    className='flex items-start gap-2 border bg-muted/40 p-3'
    data-testid='admin-app-availability-verdict'
  >
    <AdminStatusPill
      tone={availabilityTone(data)}
      label={availabilityLabel(data)}
      data-testid='admin-app-availability-verdict-pill'
    />
    <p className='min-w-0 text-muted-foreground text-xs leading-relaxed'>{explain(data)}</p>
  </div>
);
