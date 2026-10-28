import { useCustomAppsWithVerdict } from "@/hooks/api/customApps/useCustomApps";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { type AlertSeverity, type HqSignalSeed, healthSeeds } from "./healthSeeds";

export type { AlertSeverity } from "./healthSeeds";

export interface HqSignal {
  id: string;
  severity: AlertSeverity;
  category: string;
  title: string;
  destLabel: string;
  /** Link to a custom app (absolute, same-origin), opened in this tab. */
  href?: string;
  /** In-SPA route (e.g. Oxygen Factory). */
  route?: string;
}

/** HQ signals resolved to real destinations. App-targeted signals link to the
 *  published app (dropped if absent so links never dead-end); core-targeted
 *  signals route to Oxygen Factory.
 *
 *  The one real feed today is each app's health verdict (`CustomAppSummary.verdict`,
 *  the same evaluator as the admin fleet-health table). The roadmap's other feeds —
 *  monitors, anomalies, source health — join `seeds` here; the resolution below is
 *  feed-agnostic. */
export function useHqAlerts(): HqSignal[] {
  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const { data: apps = [] } = useCustomAppsWithVerdict(project?.id ?? "");
  const coreRoute = ROUTES.ORG(orgSlug).WORKSPACE(project?.id ?? "").IDE.ROOT;

  const seeds: HqSignalSeed[] = healthSeeds(apps);

  return seeds.flatMap(({ target, ...rest }): HqSignal[] => {
    if ("core" in target) {
      return [{ ...rest, destLabel: "Oxygen Factory", route: coreRoute }];
    }
    const app = apps.find((a) => a.slug === target.app);
    return app
      ? [
          {
            ...rest,
            destLabel: app.name,
            href: app.url
          }
        ]
      : [];
  });
}
