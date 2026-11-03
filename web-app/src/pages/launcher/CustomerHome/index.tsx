import { Eye } from "lucide-react";
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
import { AppLaunchScreen, useAppLaunch } from "./components/AppLaunch";
import { FirstAppsOnTheirWay } from "./components/FirstAppsOnTheirWay";
import { HomeAppGrid } from "./components/HomeAppGrid";
import { HomeAskBox } from "./components/HomeAskBox";
import { HomeRecentQuestions, useRecentQuestions } from "./components/HomeRecentQuestions";
import { HomeWidgets, needsCount, TODAY_ANCHOR, type WidgetGroup } from "./components/HomeWidgets";
import { isUnavailable } from "./components/HomeWidgets/utils";
import { WorldModelCard, worldModelTitle } from "./components/WorldModelCard";
import { homeGreeting } from "./greeting";
import { HOME_CONTAINER, HOME_GAP, HOME_RAIL_QUERY } from "./layout";
import { type Since, useSinceLastVisit } from "./useSinceLastVisit";
import { useSideHint, useTodayHint, useTransitionedFlag } from "./useTodayHint";

/**
 * The org's home with the apps-first experience on: the org and a greeting, an ask
 * box that hands off to the Ask dock, every app the person can open across the org,
 * how the org's data fits together, their recent questions, and Today, the apps'
 * widgets.
 *
 * The apps and what is happening in them come first. Apps and widgets each have
 * a place, so neither buries the other (`layout.ts`): from `lg` the apps fill the
 * main column and a side column holds Today, recent questions and the data map;
 * under it, one column, the apps right after the greeting. The page is the one
 * scroller: nothing scrolls inside it.
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
  // Hidden Today means no widgets request at all, and nothing is read before the
  // person's preference is known (a failed read counts as "shown").
  const preferences = useHomePreferences(org?.slug);
  const todayHidden = preferences.data?.today === "hidden";
  const todayKnown = !preferences.isPending;
  const widgetsOn = todayKnown && !todayHidden;
  const widgetSections = useHomeSections(org?.slug, widgetsOn);
  // With Today hidden the widgets are not read, so the org layout's sections come
  // with the preferences: an org whose layout dropped Insights keeps it dropped.
  const sections = widgetsOn
    ? widgetSections
    : (sectionsFromKinds(preferences.data?.sections) ?? widgetSections);
  const widgets = useHomeWidgets(org?.slug, widgetsOn);
  const hiddenWidgets = useHiddenWidgets(org?.slug, widgetsOn);
  const since = useSinceLastVisit(org?.slug, widgets.data);
  const launch = useAppLaunch(apps);
  const { setToday } = useHomePreferenceActions(org?.slug ?? "");
  // Known once the widgets load; until then, what this browser saw last time, so
  // the frame does not change under the apps on every visit. Copies the person
  // hid still count: Today is there to bring them back.
  const todayAnswer = todayHidden
    ? false
    : widgets.isError
      ? true
      : widgets.data
        ? widgets.data.length > 0 || hiddenWidgets.length > 0
        : undefined;
  const todayHint = useTodayHint(org?.slug, todayAnswer);
  // Today appearing (or going) moves the apps: animate that, don't jump.
  const hasToday = useTransitionedFlag(todayHint === true);
  const wide = useMediaQuery(HOME_RAIL_QUERY);
  const orgRole = useCurrentOrg((s) => s.role);
  const location = useLocation();
  const navigate = useNavigate();
  const openAsk = useAskDock((s) => s.open);
  // Read here, not only in their cards: whether there is anything for the side column decides it.
  const { data: worldModel, isLoading: worldModelLoading } = useWorldModel();
  const { rows: recent, settled: recentSettled } = useRecentQuestions();
  // Whether the side column holds anything besides Today, once both reads are in.
  const insightsAnswer = !sections.some((s) => s.kind === "insights")
    ? false
    : recentSettled && !worldModelLoading
      ? recent.length > 0 || (worldModel?.entities.length ?? 0) > 0
      : undefined;
  const sideAnswer =
    todayAnswer === true || insightsAnswer === true
      ? true
      : todayAnswer === false && insightsAnswer === false
        ? false
        : undefined;
  // Remembered, so the apps' grid has its final width on the first paint instead of
  // reflowing when the reads land; a change animates, like Today's arrival.
  const hasSide = useTransitionedFlag(useSideHint(org?.slug, sideAnswer) === true);

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
  const hasInsights = showInsights && (recent.length > 0 || showWorldModel);
  // The side column, from `lg`, whenever it has something to hold (or held something
  // last time, while the reads are out). With it, the apps keep their place when
  // Today arrives: only the side column changes.
  const side = wide && hasSide;
  const today = (variant: "rail" | "stack") => (
    <HomeWidgets
      orgSlug={org?.slug}
      apps={apps}
      groups={groups}
      since={since}
      variant={variant}
      enabled={widgetsOn}
    />
  );

  // The org's name is already in the top bar's breadcrumb; the greeting stays
  // small, and the line under it says what the day holds, linking to Today.
  const header = (
    <header className='flex flex-col gap-5 pt-8'>
      <div className='flex flex-col gap-1'>
        <h1 className='font-medium text-xl tracking-tight sm:text-2xl'>
          {homeGreeting(user?.name)}
        </h1>
        {/* With Today, inside the same transition, so it does not push the apps first. */}
        {todayHidden ? (
          <ShowToday onShow={() => setToday("shown")} />
        ) : (
          hasToday && <HomeStatus widgets={widgets.data} since={since} />
        )}
      </div>
      {canAsk && <HomeAskBox orgName={org?.name} />}
    </header>
  );
  const appsBlock = (
    // The container the grid's columns are measured against (`@container`).
    <section className='@container flex flex-col gap-3' aria-labelledby='home-apps-heading'>
      <h2 id='home-apps-heading' className='font-medium text-base'>
        Your apps
      </h2>
      {apps.length === 0 ? (
        <FirstAppsOnTheirWay orgName={org?.name} />
      ) : (
        <HomeAppGrid apps={apps} onManageAccess={manageableOrgId ? setManaging : undefined} />
      )}
    </section>
  );
  const insights = hasInsights && (
    <div
      className={cn("flex flex-col", HOME_GAP)}
      data-testid='home-lower-row'
      // Moves when Today arrives above it, rather than cross-fading.
      style={{ viewTransitionName: "home-lower" }}
    >
      <HomeRecentQuestions rows={recent} />
      {showWorldModel && (
        <WorldModelCard
          model={worldModel}
          // The org's name is in the breadcrumb; in the side column it would cut the title.
          title={worldModelTitle(side ? undefined : org?.name)}
          to={worldModelTo}
          compact={side}
        />
      )}
    </div>
  );
  // "Needs attention" without the "Not measured" rows for a customer.
  const attention = <NeedsAttention includeUnmeasured={factoryUser} contained={false} />;

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: a delegate, not a control: it sees the clicks of the links inside, which keep their own roles and keys
    // biome-ignore lint/a11y/useKeyWithClickEvents: Enter on a link is a click, so the keyboard launches too
    <div
      // The page is the one scroller, so its scrollbar is the window's edge, never
      // a bar between the apps and Today.
      className='flex h-full flex-col overflow-y-auto'
      data-testid='customer-home'
      data-today={side && hasToday ? "rail" : hasToday ? "stack" : "none"}
      // Any link in here that opens an app opens it with its launch screen.
      onClick={launch.onClick}
    >
      {factoryUser && <ProjectSetupToast gaps={readiness.gaps} />}

      {/* Real alerts only: an app that is down. Hidden in the calm default state. */}
      <CriticalAlertBanner />

      <div className={cn(HOME_CONTAINER, "flex items-start gap-10 pb-10")} data-testid='home-body'>
        <div className='flex min-w-0 flex-1 flex-col gap-8'>
          {header}
          {appsBlock}
          {!side && hasToday && today("stack")}
          {!side && insights}
          {attention}
        </div>
        {side && (
          <aside
            aria-label={hasToday ? "Today" : "Recent"}
            // Eases in from the side the first time an org has a Today in this browser.
            className='flex w-80 shrink-0 starting:translate-x-3 flex-col gap-8 pt-8 starting:opacity-0 transition-[opacity,translate] duration-(--duration-slow) motion-reduce:starting:translate-x-0 xl:w-96'
            data-testid='home-side'
          >
            {hasToday && today("rail")}
            {insights}
          </aside>
        )}
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

/** With Today hidden, the one way back: small and quiet, where the day's status line would be. */
function ShowToday({ onShow }: { onShow: () => void }) {
  return (
    <button
      type='button'
      data-testid='home-today-show'
      onClick={onShow}
      className='flex min-h-6 pointer-coarse:min-h-11 w-fit items-center gap-1.5 text-muted-foreground text-sm transition-colors hover:text-foreground'
    >
      <Eye className='size-3.5' />
      Show Today
    </button>
  );
}

/**
 * One line under the greeting: how many widgets have something to do, as a link
 * to Today. On a phone Today is under the apps, so this is how the day's news is
 * known before scrolling, and how many widgets changed since the person was last
 * here (`useSinceLastVisit`). Nothing while there are no widgets.
 */
function HomeStatus({
  widgets,
  since
}: {
  widgets: WidgetCardData[] | undefined;
  since: ReadonlyMap<string, Since>;
}) {
  if (!widgets?.length) return null;
  // Nothing answered: there is no day to report, clear or otherwise.
  if (widgets.every(isUnavailable)) return null;
  const needs = needsCount(widgets);
  const failed = widgets.filter(isUnavailable).length;
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
      {needs > 0
        ? `${needs} ${needs === 1 ? "thing needs" : "things need"} you today`
        : failed > 0
          ? `All clear, but ${failed} couldn't load`
          : "All clear today"}
      {/* What changed while the person was away, so a return visit says why to look. */}
      {since.size > 0 && (
        <>
          <span aria-hidden className='size-1 shrink-0 rounded-full bg-muted-foreground/50' />
          <span data-testid='home-status-since'>
            {since.size} {since.size === 1 ? "update" : "updates"} since your last visit
          </span>
        </>
      )}
    </a>
  );
}
