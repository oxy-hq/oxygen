import { useEffect, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { AppAccessDialog } from "@/components/appAccess/AppAccessDialog";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useOrgCustomApps from "@/hooks/api/customApps/useOrgCustomApps";
import { sectionsFromKinds } from "@/hooks/api/homeWidgets/preferences";
import {
  useHomePreferenceActions,
  useHomePreferences
} from "@/hooks/api/homeWidgets/useHomePreferences";
import {
  useHiddenWidgets,
  useHomeSections,
  useHomeWidgets
} from "@/hooks/api/homeWidgets/useHomeWidgets";
import { useWorldModel } from "@/hooks/api/useWorldModel";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import useFactoryAccess from "@/hooks/useFactoryAccess";
import ROUTES from "@/libs/utils/routes";
import useAskDock from "@/stores/useAskDock";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { CustomAppSummary } from "@/types/apps";
import { CriticalAlertBanner } from "../components/CriticalAlertBanner";
import { NeedsAttention } from "../components/NeedsAttention";
import { ProjectSetupToast } from "../components/ProjectSetupToast";
import useWorkspaceReadiness from "../useWorkspaceReadiness";
import { AppLaunchScreen, useAppLaunch } from "./components/AppLaunch";
import { HomeApps } from "./components/HomeApps";
import { HomeAskBox } from "./components/HomeAskBox";
import { HomeRecentQuestions, useRecentQuestions } from "./components/HomeRecentQuestions";
import { ShowToday } from "./components/HomeStatus";
import { HomeViewSwitch } from "./components/HomeViewSwitch";
import { HomeWidgets, type WidgetGroup } from "./components/HomeWidgets";
import { TodayFold } from "./components/TodayFold";
import { WorldModelCard, worldModelTitle } from "./components/WorldModelCard";
import { HOME_COLUMN } from "./layout";
import { useHomeView } from "./useHomeView";
import { useRecentApps } from "./useRecentApps";
import { useSinceLastVisit } from "./useSinceLastVisit";

/**
 * Home page v3 (`home_v3`). One column, most-used first:
 * the ask box (the answer engine), then the apps, big, then Today folded to a
 * line, then, on the full view only, recent questions and how the org's data fits
 * together. No greeting and no other words: the org's name is in the top bar.
 *
 * Two views (`useHomeView`), one switch at the apps' heading. Simple, for the
 * people who come to open an app: big icons, the ones opened last first, Today
 * folded. Full, for the people who run the org from here: app tiles with their
 * freshness, unread news and access, Today open, and the deeper reads below.
 *
 * Setup machinery is the Factory's. Someone who may not use the Factory sees none of
 * it: no setup toast, no "Not measured" rows (a gap in monitoring, not a problem with
 * the app), only real alerts. A Factory user keeps both. Nothing here says
 * "workspace", and nothing here shows git.
 */
export default function HomeV3() {
  const readiness = useWorkspaceReadiness();
  const { project } = useCurrentProjectBranch();
  const { apps, isPending: appsPending } = useOrgCustomApps(project?.id ?? "");
  const factoryUser = useFactoryAccess();
  const org = useCurrentOrg((s) => s.org);
  const orgRole = useCurrentOrg((s) => s.role);
  const [view, setView] = useHomeView(org?.slug, orgRole);
  // Hidden Today means no widgets request at all, and nothing is read before the
  // person's preference is known (a failed read counts as "shown").
  const preferences = useHomePreferences(org?.slug);
  const todayHidden = preferences.data?.today === "hidden";
  const widgetsOn = !preferences.isPending && !todayHidden;
  const widgetSections = useHomeSections(org?.slug, widgetsOn);
  // With Today hidden the widgets are not read, so the org layout's sections come
  // with the preferences: an org whose layout dropped Insights keeps it dropped.
  const sections = widgetsOn
    ? widgetSections
    : (sectionsFromKinds(preferences.data?.sections) ?? widgetSections);
  const widgets = useHomeWidgets(org?.slug, widgetsOn);
  const hiddenWidgets = useHiddenWidgets(org?.slug, widgetsOn);
  const since = useSinceLastVisit(org?.slug, widgets.data);
  const { recent: recentApps, opened } = useRecentApps(org?.slug, apps);
  const launch = useAppLaunch(apps, opened);
  const { setToday } = useHomePreferenceActions(org?.slug ?? "");
  const location = useLocation();
  const navigate = useNavigate();
  const openAsk = useAskDock((s) => s.open);
  const { data: worldModel } = useWorldModel();
  const { rows: recentQuestions } = useRecentQuestions();

  // The same access gate as the HQ launcher: owners and admins manage an app's
  // access from its tile; the server re-checks.
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

  const full = view === "full";
  const canAsk = !readiness.shouldDisableChat;
  const groups: WidgetGroup[] = sections.flatMap((s) =>
    s.kind === "widgets" && (s.items.length > 0 || s.title) ? [s] : []
  );
  // The layout may leave insights out; apps always have their place.
  const showWorldModel = !!worldModel && worldModel.entities.length > 0;
  const insights =
    full &&
    sections.some((s) => s.kind === "insights") &&
    (recentQuestions.length > 0 || showWorldModel);

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: a delegate, not a control: it sees the clicks of the links inside, which keep their own roles and keys
    // biome-ignore lint/a11y/useKeyWithClickEvents: Enter on a link is a click, so the keyboard launches too
    <div
      // The page is the one scroller: nothing scrolls inside it.
      className='flex h-full flex-col overflow-y-auto'
      data-testid='customer-home'
      data-view={view}
      // Any link in here that opens an app opens it with its launch screen.
      onClick={launch.onClick}
    >
      {factoryUser && <ProjectSetupToast gaps={readiness.gaps} />}

      {/* Real alerts only: an app that is down. Hidden in the calm default state. */}
      <CriticalAlertBanner />

      <h1 className='sr-only'>{org?.name ? `${org.name} home` : "Home"}</h1>
      <div className={HOME_COLUMN} data-testid='home-body'>
        {canAsk && <HomeAskBox orgName={org?.name} />}
        <HomeApps
          view={view}
          apps={apps}
          recent={recentApps}
          orgName={org?.name}
          onManageAccess={manageableOrgId ? setManaging : undefined}
          aside={<HomeViewSwitch view={view} onChange={setView} />}
        />
        {todayHidden ? (
          <ShowToday onShow={() => setToday("shown")} />
        ) : (
          <TodayFold
            view={view}
            widgets={widgets.data}
            hiddenCount={hiddenWidgets.length}
            failed={widgets.isError}
            since={since}
          >
            <HomeWidgets
              orgSlug={org?.slug}
              apps={apps}
              groups={groups}
              since={since}
              variant='stack'
              enabled={widgetsOn}
              heading={false}
            />
          </TodayFold>
        )}
        {insights && (
          <div
            className='grid grid-cols-1 items-start gap-8 md:grid-cols-2'
            data-testid='home-insights'
          >
            <HomeRecentQuestions rows={recentQuestions} />
            {showWorldModel && (
              <WorldModelCard
                model={worldModel}
                title={worldModelTitle()}
                to={ROUTES.ORG(org?.slug ?? "").WORKSPACE(project?.id ?? "").WORLD_MODEL}
                compact
              />
            )}
          </div>
        )}
        {/* "Needs attention" without the "Not measured" rows for a customer. */}
        {full && <NeedsAttention includeUnmeasured={factoryUser} contained={false} />}
      </div>

      <AppLaunchScreen app={launch.launching} name={launch.launchName} grown={launch.grown} />

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
