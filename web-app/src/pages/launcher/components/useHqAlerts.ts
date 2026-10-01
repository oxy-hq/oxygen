import { useCustomApps } from "@/hooks/api/customApps/useCustomApps";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { appWindowName } from "@/libs/utils/appWindowName";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";

export type AlertSeverity = "critical" | "warning" | "info";

/** Where a signal is addressed: a custom app, or Oxygen Factory (the system
 *  layer — for data/source/pipeline issues). */
type SignalTarget = { app: string } | { core: true };

/** An unresolved signal — the shape a real monitors/anomalies feed will emit,
 *  and the shape the demo set conforms to. `useHqAlerts` resolves the `target`
 *  into a concrete destination. */
interface HqSignalSeed {
  id: string;
  severity: AlertSeverity;
  /** Short signal category shown before the message (e.g. "Labor risk"). */
  category: string;
  title: string;
  target: SignalTarget;
}

export interface HqSignal {
  id: string;
  severity: AlertSeverity;
  category: string;
  title: string;
  destLabel: string;
  /** Link to a custom app (absolute, same-origin). */
  href?: string;
  /** Window `href` opens in — set for app destinations only, per
   *  `appWindowName`: an app is never hosted inside HQ's browsing context. */
  target?: string;
  /** In-SPA route (e.g. Oxygen Factory). */
  route?: string;
}

/** HQ signals resolved to real destinations. App-targeted signals link to the
 *  published app (dropped if absent so links never dead-end); core-targeted
 *  signals route to Oxygen Factory.
 *
 *  There is no real signal feed yet (roadmap: monitors, anomalies, source
 *  health), so this surfaces no signals today and `NeedsAttention` /
 *  `CriticalAlertBanner` self-hide. When a feed lands, populate `seeds` from it
 *  here — the resolution below is feed-agnostic. */
export function useHqAlerts(): HqSignal[] {
  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const { data: apps = [] } = useCustomApps(project?.id ?? "");
  const coreRoute = ROUTES.ORG(orgSlug).WORKSPACE(project?.id ?? "").IDE.ROOT;

  // No real signal feed yet — populate from monitors/anomalies/source-health
  // when one is wired. Until then HQ surfaces no signals.
  const seeds: HqSignalSeed[] = [];

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
            href: app.url,
            target: appWindowName(app.org_slug, app.slug)
          }
        ]
      : [];
  });
}
