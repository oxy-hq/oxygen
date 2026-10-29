import { useEffect, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { AppAccessDialog } from "@/components/appAccess/AppAccessDialog";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useOrgCustomApps from "@/hooks/api/customApps/useOrgCustomApps";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { useWorldModel } from "@/hooks/api/useWorldModel";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import useFactoryAccess from "@/hooks/useFactoryAccess";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import useAskDock from "@/stores/useAskDock";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { CustomAppSummary } from "@/types/apps";
import { CriticalAlertBanner } from "../components/CriticalAlertBanner";
import { NeedsAttention } from "../components/NeedsAttention";
import { ProjectSetupToast } from "../components/ProjectSetupToast";
import useWorkspaceReadiness from "../useWorkspaceReadiness";
import { FirstAppsOnTheirWay } from "./components/FirstAppsOnTheirWay";
import { HomeAppGrid } from "./components/HomeAppGrid";
import { HomeAskBox } from "./components/HomeAskBox";
import { HomeRecentQuestions } from "./components/HomeRecentQuestions";
import { HomeWidgets } from "./components/HomeWidgets";
import { WorldModelCard, worldModelTitle } from "./components/WorldModelCard";
import { homeGreeting } from "./greeting";
import { appRow, HOME_CONTAINER, HOME_GAP, lowerRowColumns } from "./layout";

/**
 * The org's home with the apps-first experience on: the org and a greeting, an ask
 * box that hands off to the Ask dock, every app the person can open across the org,
 * how the org's data fits together, and their recent questions.
 *
 * Setup machinery is the Factory's. Someone who may not use the Factory sees none of
 * it: no setup toast, no "Not measured" rows (a gap in monitoring, not a problem with
 * the app), only real alerts. A Factory user keeps both. Nothing here says
 * "workspace", and nothing here shows git.
 */
export default function CustomerHome() {
  const readiness = useWorkspaceReadiness();
  const { project } = useCurrentProjectBranch();
  const { apps, isPending: appsPending } = useOrgCustomApps(project?.id ?? "");
  const factoryUser = useFactoryAccess();
  const { data: user } = useCurrentUser();
  const org = useCurrentOrg((s) => s.org);
  const orgRole = useCurrentOrg((s) => s.role);
  const location = useLocation();
  const navigate = useNavigate();
  const openAsk = useAskDock((s) => s.open);
  // Read here, not only in the card: whether there is a model changes the row's columns.
  const { data: worldModel } = useWorldModel();

  // The same access gate as the HQ launcher: owners and admins manage an app's
  // access from its card; the server re-checks.
  const manageableOrgId = org && (orgRole === "owner" || orgRole === "admin") ? org.id : null;
  const [managing, setManaging] = useState<CustomAppSummary | null>(null);

  // `navigate(HOME, { state: { prefillQuestion } })` callers open the Ask dock
  // prefilled, as they do on the HQ launcher.
  const locationState = location.state as {
    prefillQuestion?: string;
    agentPath?: string;
    autoSubmit?: boolean;
  } | null;
  useEffect(() => {
    if (readiness.status !== "ready") return;
    if (!locationState?.prefillQuestion && !locationState?.agentPath) return;
    openAsk({
      message: locationState.prefillQuestion,
      agentPath: locationState.agentPath,
      autoSubmit: locationState.autoSubmit
    });
    navigate(location.pathname + location.search, { replace: true, state: null });
  }, [readiness.status, locationState, openAsk, navigate, location.pathname, location.search]);

  if (readiness.status === "loading" || appsPending) {
    return (
      <div className='flex h-full items-center justify-center'>
        <Spinner className='size-6' />
      </div>
    );
  }

  const canAsk = !readiness.shouldDisableChat;
  const worldModelTo = ROUTES.ORG(org?.slug ?? "").WORKSPACE(project?.id ?? "").WORLD_MODEL;
  const row = appRow(apps.length);
  const showWorldModel = !!worldModel && worldModel.entities.length > 0;
  const lower = lowerRowColumns(row, showWorldModel);

  return (
    <div className='flex h-full flex-col overflow-auto' data-testid='customer-home'>
      {factoryUser && <ProjectSetupToast gaps={readiness.gaps} />}

      {/* Every block below sits in HOME_CONTAINER, the banners' container too, so
          they all share one left and one right edge. */}
      <header className={cn(HOME_CONTAINER, "flex flex-col gap-6 pt-10 pb-8")}>
        <div className='flex flex-col gap-1'>
          {org?.name && (
            <p className='text-muted-foreground text-sm' data-testid='home-org-name'>
              {org.name}
            </p>
          )}
          <h1 className='font-medium text-2xl tracking-tight sm:text-3xl'>
            {homeGreeting(user?.name)}
          </h1>
        </div>
        {canAsk && <HomeAskBox orgName={org?.name} />}
      </header>

      {/* Real alerts only: an app that is down. Hidden in the calm default state. */}
      <CriticalAlertBanner />

      <div className={cn(HOME_CONTAINER, "flex flex-col gap-8 pb-10")} data-testid='home-body'>
        {/* What needs doing in each app now, above the apps themselves. */}
        <HomeWidgets orgSlug={org?.slug} apps={apps} />

        <section className='flex flex-col gap-3' aria-labelledby='home-apps-heading'>
          <h2 id='home-apps-heading' className='font-medium text-base'>
            Your apps
          </h2>
          {row === "none" ? (
            <FirstAppsOnTheirWay orgName={org?.name} />
          ) : (
            <HomeAppGrid
              apps={apps}
              row={row}
              onManageAccess={manageableOrgId ? setManaging : undefined}
            />
          )}
        </section>

        <div className={cn("grid", HOME_GAP, lower.row)} data-testid='home-lower-row'>
          {showWorldModel && (
            <WorldModelCard
              model={worldModel}
              title={worldModelTitle(org?.name)}
              to={worldModelTo}
              half={row === "two" || row === "dense"}
              className={lower.worldModel}
            />
          )}
          <HomeRecentQuestions canAsk={canAsk} />
        </div>
      </div>

      {/* "Needs attention" without the "Not measured" rows for a customer. */}
      <NeedsAttention includeUnmeasured={factoryUser} />

      {manageableOrgId && (
        <AppAccessDialog
          open={managing !== null}
          onOpenChange={(open) => !open && setManaging(null)}
          scope={{ kind: "org", orgId: manageableOrgId }}
          appId={managing?.id ?? null}
          appName={managing?.name ?? ""}
        />
      )}
    </div>
  );
}
