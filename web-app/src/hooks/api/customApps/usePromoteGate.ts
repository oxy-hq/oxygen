import {
  cardGate,
  productionSmokeLine,
  rollbackTarget,
  type SmokeLine
} from "@/pages/admin/AdminCustomApps/components/AppConsole/BuildChecks/promoteGate";
import { promotionGate } from "@/pages/admin/AdminCustomApps/components/AppConsole/promotionChecksModel";
import type { AppBuild, CustomApp } from "@/types/apps";
import type { AppLifecycle } from "./deriveLifecycle";
import { DEFAULT_RELEASE_WINDOW, useAppReleases } from "./useAppReleases";
import { useBuildChecks } from "./useBuildChecks";
import { useFollowFailedSmoke } from "./useFollowFailedSmoke";

export interface PromoteGate {
  /** A check the app requires is still open on the build staging serves. */
  promoteWaits: boolean;
  /** Production's own smoke of the build it serves, once there is one. With it failed,
   *  `rollback` is the build production served before, when one is still retained. */
  smoke: { line: SmokeLine; rollback: AppBuild | null } | null;
}

const OFF: PromoteGate = { promoteWaits: false, smoke: null };

/**
 * What the Live build card needs once checks hold Promote, composed from reads other
 * panels already make: the staging build's checks (the card's Checks row holds the
 * same cache entry), the production attempt of the live build, and, only while that
 * smoke reads failed, the release timeline that names the build to go back to.
 *
 * With the org's `promote_checks_gate` flag off every read here is disabled and the
 * answer is `OFF`: no request is made that was not made before.
 */
export function usePromoteGate(
  app: Pick<CustomApp, "id" | "promotion_checks">,
  lifecycle: Pick<AppLifecycle, "draftBuild" | "draftAhead" | "liveBuild" | "builds">
): PromoteGate {
  const gate = promotionGate(app.promotion_checks);
  const staged = lifecycle.draftAhead ? lifecycle.draftBuild : null;
  const staging = useBuildChecks(app.id, gate && staged ? staged.id : undefined);
  const production = useBuildChecks(
    app.id,
    gate && lifecycle.liveBuild ? lifecycle.liveBuild.id : undefined,
    "production"
  );
  const line = gate ? productionSmokeLine(production.data) : null;
  const failed = line?.state === "failed";
  const releases = useAppReleases(app.id, DEFAULT_RELEASE_WINDOW, failed);
  // Where the app rolls back by itself, production moves a few seconds after a failed
  // verdict with nobody here moving it: follow it.
  useFollowFailedSmoke(app, production.data);

  if (!gate) return OFF;
  return {
    promoteWaits: cardGate(staging.data, gate.effective, gate.seen_required)?.state === "waiting",
    smoke: line
      ? { line, rollback: failed ? rollbackTarget(releases.data, lifecycle.builds) : null }
      : null
  };
}
