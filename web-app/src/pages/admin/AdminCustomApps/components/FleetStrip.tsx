import { Info } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { FleetHealthResponse } from "@/types/apps";

/**
 * One line for the facts that are true of the **deployment**, not of an app.
 *
 * This exists because of a measured defect. `GET /admin/apps/health` returns
 * `observability_configured: false` and three apps carrying one identical 66-character
 * `reason` string. The old registry table rendered that string once per row *and* a
 * banner saying the same thing, so a three-app fleet stated one fact **four times** on a
 * single screen — the same disease the workspace-health page had, where nine workspaces
 * printed one shared failure nine times.
 *
 * The rule this encodes: **a fact about the deployment is stated once, at the top.** An
 * app's own panels then say only what is true of that app — a health pill reads "Not
 * measured" and stops, because *why* nothing is measured is this strip's job.
 *
 * It renders nothing when there is nothing deployment-wide to say. A strip that is always
 * present is chrome; one that appears only when it has news is information.
 */
export const FleetStrip = ({
  fleet,
  className
}: {
  fleet: FleetHealthResponse | undefined;
  className?: string;
}) => {
  // Undefined means the fleet endpoint has not answered. Silence is right: claiming
  // capture is configured — or that it is not — would both be guesses, and this strip's
  // whole purpose is to be the one place that states this accurately.
  if (!fleet || fleet.observability_configured) return null;

  return (
    <div
      data-testid='apps-fleet-strip'
      className={cn(
        "flex flex-wrap items-center gap-x-2 gap-y-1 border-b px-4 py-1.5 text-xs",
        ADMIN_TONE.warn.bg,
        className
      )}
    >
      <Info className={cn("size-3 shrink-0", ADMIN_TONE.warn.text)} aria-hidden />
      <span className={cn("font-medium", ADMIN_TONE.warn.text)}>
        Observability capture is off — no app on this deployment is measured.
      </span>
      <span className='text-muted-foreground'>
        Every &ldquo;Not measured&rdquo; below is this one fact, not{" "}
        <span className='tabular-nums'>{fleet.apps.length}</span> separate ones. Request counts and
        error rates are unavailable fleet-wide until{" "}
        <code className='font-mono'>OXY_OBSERVABILITY_BACKEND</code> is set.
      </span>
    </div>
  );
};
