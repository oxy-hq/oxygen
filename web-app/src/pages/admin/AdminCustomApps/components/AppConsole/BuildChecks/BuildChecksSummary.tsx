import { Link } from "react-router-dom";
import { useBuildChecks } from "@/hooks/api/customApps/useBuildChecks";
import { AdminStatusPill } from "@/pages/admin/components/AdminStatusPill";
import type { AppBuild, PromotionChecks } from "@/types/apps";
import { promotionGate } from "../promotionChecksModel";
import { checksRollup } from "./buildChecksModel";
import { gatedRollup } from "./promoteGate";

/**
 * The Live build card's Checks row: the staging build's checks in one chip — "Starting"
 * until a runner answers, "2 of 4 done" while they run (re-read every few seconds),
 * "1 check failed", "All 4 passed" — linking to that build's Checks list. A build
 * published before checks existed reads "Not checked". It reports only: nothing here
 * holds Promote.
 *
 * With the org's promote-checks gate on (`promotionChecks.enabled`), a required check
 * that did not pass says so in the same chip: "1 required check failed".
 */
export const BuildChecksSummary = ({
  appId,
  build,
  to,
  promotionChecks
}: {
  appId: string;
  build: Pick<AppBuild, "id">;
  /** Where the build's Checks list is. */
  to: string;
  /** The app's gate; absent or off, the chip is the roll-up it always was. */
  promotionChecks?: PromotionChecks | null;
}) => {
  const checks = useBuildChecks(appId, build.id);
  if (checks.isPending) {
    return (
      <span className='text-muted-foreground' data-testid='build-checks-summary-loading'>
        …
      </span>
    );
  }
  if (checks.isError || !checks.data) {
    return (
      <span className='text-muted-foreground' data-testid='build-checks-summary-error'>
        Couldn't read
      </span>
    );
  }
  const gate = promotionGate(promotionChecks);
  const rollup = gate ? gatedRollup(checks.data, gate.effective) : checksRollup(checks.data);
  return (
    <Link
      to={to}
      className='inline-flex rounded-full hover:underline'
      data-testid='build-checks-summary-link'
      data-state={rollup.state}
    >
      {rollup.state === "none" && rollup.tone === "muted" ? (
        <span className='text-muted-foreground'>{rollup.label}</span>
      ) : (
        <AdminStatusPill tone={rollup.tone} label={rollup.label} />
      )}
    </Link>
  );
};
