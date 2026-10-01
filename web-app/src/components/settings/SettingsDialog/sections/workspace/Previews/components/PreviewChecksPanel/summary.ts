import type { PreviewChecksSummary } from "@/types/workspace";

/**
 * The row-level verdict text derived from the cheap embedded `checks`
 * summary — never the per-pipeline detail, which is a separate fetch made
 * only once the row is expanded.
 *
 *  - no analyze run yet (`null`), or one still running (`"pending"`) → "checking…"
 *  - the analyzer itself failed → "check failed"
 *  - done, and nothing needs attention → "no pipeline changes"
 *  - done, and something does → "N need reset" and/or "M warnings", plus
 *    "K transform builds" when the branch also changed transforms
 */
export function previewChecksSummaryText(checks: PreviewChecksSummary | null): string {
  if (!checks || checks.status === "pending") return "checking…";
  if (checks.status === "failed") return "check failed";

  const parts: string[] = [];
  if (checks.needs_reset > 0) parts.push(`${checks.needs_reset} need reset`);
  if (checks.warnings > 0) {
    parts.push(`${checks.warnings} warning${checks.warnings === 1 ? "" : "s"}`);
  }
  if (checks.transforms > 0) {
    parts.push(`${checks.transforms} transform build${checks.transforms === 1 ? "" : "s"}`);
  }
  if (parts.length === 0) return "no pipeline changes";
  return parts.join(" · ");
}

export type PreviewChecksSummaryTone = "pending" | "muted" | "warning" | "danger";

export function previewChecksSummaryTone(
  checks: PreviewChecksSummary | null
): PreviewChecksSummaryTone {
  if (!checks || checks.status === "pending") return "pending";
  if (checks.status === "failed") return "warning";
  if (checks.needs_reset > 0) return "danger";
  if (checks.warnings > 0) return "warning";
  return "muted";
}

/** Whether there is anything worth expanding into per-pipeline detail. */
export function previewChecksExpandable(checks: PreviewChecksSummary | null): boolean {
  return !!checks && checks.status !== "pending";
}
