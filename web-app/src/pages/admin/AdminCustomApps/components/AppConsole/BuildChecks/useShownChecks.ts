import { useAppBuilds } from "@/hooks/api/customApps/useAppBuilds";
import { useBuildChecks } from "@/hooks/api/customApps/useBuildChecks";
import type { BuildChecksEnvironment } from "@/types/appBuildChecks";
import { checksTarget, productionTarget } from "./buildChecksModel";
import { hasProductionAttempt } from "./promoteGate";

/**
 * Which build the Checks panel shows, and which of its attempts.
 *
 * Staging's attempt, for the build `?checks=` names or else the one staging serves.
 * With the promote-checks gate on (`gated`), a build production serves also has an
 * attempt run on production after the move: the panel offers it once one exists
 * (`offersProduction`) and shows it when `environment` asks for it. `shown` is the
 * environment whose attempt `target` and `checks` are.
 */
export function useShownChecks(
  appId: string,
  requested: string | null,
  { gated, environment }: { gated: boolean; environment: BuildChecksEnvironment }
) {
  const builds = useAppBuilds(appId);
  const staged = builds.data ? checksTarget(builds.data.builds, requested) : null;
  const live = gated && builds.data ? productionTarget(builds.data.builds, requested) : null;
  const staging = useBuildChecks(appId, staged?.ref);
  // Disabled (no build named) while the gate is off: no request that was not made before.
  const production = useBuildChecks(appId, live?.ref, "production");
  const offersProduction = hasProductionAttempt(production.data);
  const onProduction =
    !!live && environment === "production" && (production.isPending || offersProduction);
  const shown: BuildChecksEnvironment = onProduction ? "production" : "staging";
  return {
    builds,
    offersProduction,
    shown,
    target: onProduction ? live : staged,
    checks: onProduction ? production : staging
  };
}
