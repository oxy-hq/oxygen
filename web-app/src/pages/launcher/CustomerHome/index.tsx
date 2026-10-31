import { useEffect, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { AppAccessDialog } from "@/components/appAccess/AppAccessDialog";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useOrgCustomApps from "@/hooks/api/customApps/useOrgCustomApps";
import { useHomeSections, useHomeWidgets } from "@/hooks/api/homeWidgets/useHomeWidgets";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { useWorldModel } from "@/hooks/api/useWorldModel";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import useFactoryAccess from "@/hooks/useFactoryAccess";
import { useMediaQuery } from "@/hooks/useMediaQuery";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import useAskDock from "@/stores/useAskDock";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { CustomAppSummary } from "@/types/apps";
import type { WidgetCard as WidgetCardData } from "@/types/homeWidgets";
import { CriticalAlertBanner } from "../components/CriticalAlertBanner";
import { NeedsAttention } from "../components/NeedsAttention";
import { ProjectSetupToast } from "../components/ProjectSetupToast";
import useWorkspaceReadiness from "../useWorkspaceReadiness";
import { FirstAppsOnTheirWay } from "./components/FirstAppsOnTheirWay";
import { HomeAppGrid } from "./components/HomeAppGrid";
import { HomeAskBox } from "./components/HomeAskBox";
import { HomeRecentQuestions } from "./components/HomeRecentQuestions";
import { HomeWidgets, needsCount, TODAY_ANCHOR, type WidgetGroup } from "./components/HomeWidgets";
import { WorldModelCard, worldModelTitle } from "./components/WorldModelCard";
import { homeGreeting } from "./greeting";
import { HOME_CONTAINER, HOME_GAP, HOME_RAIL_QUERY } from "./layout";

/**
 * The org's home with the apps-first experience on: the org and a greeting, an ask
 * box that hands off to the Ask dock, every app the person can open across the org,
 * how the org's data fits together, their recent questions, and Today, the apps'
 * widgets.
 *
 * The apps and what is happening in them come first. Apps and widgets each have
 * a place, so neither buries the other (`layout.ts`): from `lg` the apps' column
 * and the Today rail scroll on their own; under it, the apps come right after the
 * greeting and Today stacks under them. Recent questions are a
 * short list, not a panel.
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
  const sections = useHomeSections(org?.slug);
  const widgets = useHomeWidgets(org?.slug);
  const wide = useMediaQuery(HOME_RAIL_QUERY);
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
  const showWorldModel = !!worldModel && worldModel.entities.length > 0;
  // The layout may leave insights out; apps always have their place.
  const showInsights = sections.some((s) => s.kind === "insights");
  const groups: WidgetGroup[] = sections.flatMap((s) =>
    s.kind === "widgets" && (s.items.length > 0 || s.title) ? [s] : []
  );
  // Held open while the widgets load, so the apps do not reflow when they arrive.
  const hasToday = widgets.isLoading || widgets.isError || (widgets.data?.length ?? 0) > 0;
  const rail = wide && hasToday;
  const today = (variant: "rail" | "stack") => (
    <HomeWidgets orgSlug={org?.slug} apps={apps} groups={groups} variant={variant} />
  );

  // The org's name is already in the top bar's breadcrumb; the greeting stays
  // small, and the line under it says what the day holds, linking to Today.
  const header = (
    <header className='flex flex-col gap-5 pt-8'>
      <div className='flex flex-col gap-1'>
        <h1 className='font-medium text-xl tracking-tight sm:text-2xl'>
          {homeGreeting(user?.name)}
        </h1>
        <HomeStatus widgets={widgets.data} />
      </div>
      {canAsk && <HomeAskBox orgName={org?.name} />}
    </header>
  );
  const appsBlock = (
    <section className='flex flex-col gap-3' aria-labelledby='home-apps-heading'>
      <h2 id='home-apps-heading' className='font-medium text-base'>
        Your apps
      </h2>
      {apps.length === 0 ? (
        <FirstAppsOnTheirWay orgName={org?.name} />
      ) : (
        <HomeAppGrid
          apps={apps}
          besideRail={rail}
          onManageAccess={manageableOrgId ? setManaging : undefined}
        />
      )}
    </section>
  );
  const insights = showInsights && (
    <div className={cn("flex flex-col", HOME_GAP)} data-testid='home-lower-row'>
      <HomeRecentQuestions />
      {showWorldModel && (
        <WorldModelCard model={worldModel} title={worldModelTitle(org?.name)} to={worldModelTo} />
      )}
    </div>
  );
  // "Needs attention" without the "Not measured" rows for a customer.
  const attention = <NeedsAttention includeUnmeasured={factoryUser} contained={false} />;

  return (
    <div
      className={cn("flex h-full flex-col", rail ? "overflow-hidden" : "overflow-auto")}
      data-testid='customer-home'
      data-today={rail ? "rail" : hasToday ? "stack" : "none"}
    >
      {factoryUser && <ProjectSetupToast gaps={readiness.gaps} />}

      {/* Real alerts only: an app that is down. Hidden in the calm default state. */}
      <CriticalAlertBanner />

      {rail ? (
        <div
          className={cn(HOME_CONTAINER, "grid min-h-0 flex-1 grid-cols-3 gap-8")}
          data-testid='home-body'
        >
          {/* Padded by its own negative margin, so a row's edge-to-edge hover stays inside the scroller. */}
          <div className='col-span-2 -mx-2 flex min-h-0 flex-col gap-8 overflow-y-auto px-2 pb-10'>
            {header}
            {appsBlock}
            {insights}
            {attention}
          </div>
          <aside
            aria-label='Today'
            className='min-h-0 overflow-y-auto pt-8 pb-10'
            data-testid='home-today-rail'
          >
            {today("rail")}
          </aside>
        </div>
      ) : (
        <div className={cn(HOME_CONTAINER, "flex flex-col gap-8 pb-10")} data-testid='home-body'>
          {header}
          {appsBlock}
          {hasToday && today("stack")}
          {insights}
          {attention}
        </div>
      )}

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

/**
 * One line under the greeting: how many widgets have something to do, as a link
 * to Today. On a phone Today is under the apps, so this is how the day's news is
 * known before scrolling. Nothing while there are no widgets.
 */
function HomeStatus({ widgets }: { widgets: WidgetCardData[] | undefined }) {
  if (!widgets?.length) return null;
  const needs = needsCount(widgets);
  const level = widgets.some((w) => w.view?.level === "critical")
    ? "bg-destructive"
    : widgets.some((w) => w.view?.level === "attention")
      ? "bg-warning"
      : null;
  return (
    <a
      href={`#${TODAY_ANCHOR}`}
      data-testid='home-status'
      className='flex min-h-6 pointer-coarse:min-h-11 w-fit items-center gap-2 text-muted-foreground text-sm hover:text-foreground'
    >
      {needs > 0 && level && (
        <span aria-hidden className={cn("size-2 shrink-0 rounded-full", level)} />
      )}
      {needs === 0
        ? "All clear today"
        : `${needs} ${needs === 1 ? "thing needs" : "things need"} you today`}
    </a>
  );
}
