import { useEffect, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { AppAccessDialog } from "@/components/appAccess/AppAccessDialog";
import { NoticeList } from "@/components/Notifications/NoticeList";
import { NOTIFICATIONS_DOCK_QUERY } from "@/components/Shell/NotificationsDock";
import { DESIGN_REFERENCE, text2xl } from "@/components/ui/reference/tokens";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useWorkspaceTimezone from "@/hooks/api/agents/useWorkspaceTimezone";
import useOrgCustomApps from "@/hooks/api/customApps/useOrgCustomApps";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import useFactoryAccess from "@/hooks/useFactoryAccess";
import { useMediaQuery } from "@/hooks/useMediaQuery";
import { cn } from "@/libs/shadcn/utils";
import useAskDock from "@/stores/useAskDock";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { CustomAppSummary } from "@/types/apps";
import { CriticalAlertBanner } from "../components/CriticalAlertBanner";
import { ProjectSetupToast } from "../components/ProjectSetupToast";
import useWorkspaceReadiness from "../useWorkspaceReadiness";
import { AppLaunchScreen, useAppLaunch } from "./components/AppLaunch";
import { HomeAppsSection } from "./components/HomeAppsSection";
import { PhoneBell } from "./components/PhoneBell";
import { homeGreeting } from "./greeting";
import { useRecentApps } from "./useRecentApps";

/** How many opened apps "Recently used" remembers: enough to order any org's list. */
const RECENT_KEEP = 100;

/**
 * The org's home (`/<org>/home`, and a workspace's index and `home`), direction A
 * of the customer-home design: the page
 * holds the apps, and the top bar's bell opens the notifications in a panel
 * docked on the right (`NotificationsDock`, the shell's). A greeting, then every
 * app the person can open across the org as a preview card. Nothing else: no ask
 * box (the top bar has no Ask either, and ⌘K opens nothing), no Today, no recent
 * questions, no data map.
 *
 * On a phone the apps are one to a row and the bell sits at the greeting's right
 * end: it shows the notifications in place of the apps, the heading becomes
 * "Notifications", and the bell an X that closes them.
 *
 * Kept from the homes before it, none of which the design draws: an app that is
 * down still raises its banner, a Factory user still gets the setup toast, a
 * pressed card still grows into the app's launch screen, and
 * `navigate(HOME, { state: { prefillQuestion } })` still opens Ask prefilled.
 *
 * Transcribed from oxy-hq/reference-app `src/views/Home.tsx` (`Home` and
 * `PhoneHome`, branch design/home-match-oxygen).
 */
export default function CustomerHome() {
  const readiness = useWorkspaceReadiness();
  const { project } = useCurrentProjectBranch();
  const { apps, isPending: appsPending } = useOrgCustomApps(project?.id ?? "");
  const factoryUser = useFactoryAccess();
  const org = useCurrentOrg((s) => s.org);
  const { recent, opened } = useRecentApps(org?.slug, apps, RECENT_KEEP);
  const launch = useAppLaunch(apps, opened);
  const phone = !useMediaQuery(NOTIFICATIONS_DOCK_QUERY);
  const [phoneNotices, setPhoneNotices] = useState(false);
  const access = useManageAccess();
  usePrefilledAsk(readiness.status === "ready");

  if (readiness.status === "loading" || appsPending) {
    return (
      <div className='flex h-full items-center justify-center'>
        <Spinner className='size-6' />
      </div>
    );
  }

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: a delegate, not a control: it sees the clicks of the links inside, which keep their own roles and keys
    // biome-ignore lint/a11y/useKeyWithClickEvents: Enter on a link is a click, so the keyboard launches too
    <div
      className={cn(DESIGN_REFERENCE, "flex h-full flex-col overflow-y-auto bg-background")}
      data-testid='customer-home'
      // Any link in here that opens an app opens it with its launch screen.
      onClick={launch.onClick}
    >
      {factoryUser && <ProjectSetupToast gaps={readiness.gaps} />}
      {/* Real alerts only: an app that is down. Hidden in the calm default state. */}
      <CriticalAlertBanner />
      <div
        className={cn(
          "mx-auto flex w-full max-w-6xl flex-col",
          phone ? "gap-6 px-4 pt-6 pb-10" : "gap-16 px-6 pt-12 pb-16 md:px-10"
        )}
        data-testid='home-body'
      >
        <HomeHeading
          phone={phone}
          notices={phone && phoneNotices}
          onToggleNotices={() => setPhoneNotices((o) => !o)}
        />
        {phone && phoneNotices ? (
          // Edge to edge: the page's own inset is the rows' inset.
          <div id='home-notifications' className='-mx-4'>
            <NoticeList onFollow={() => setPhoneNotices(false)} />
          </div>
        ) : (
          <HomeAppsSection
            apps={apps}
            recent={recent}
            phone={phone}
            orgName={org?.name}
            onManage={access.orgId ? access.setManaging : undefined}
          />
        )}
      </div>

      <AppLaunchScreen app={launch.launching} name={launch.launchName} grown={launch.grown} />
      <HomeAccessDialog access={access} />
    </div>
  );
}

/** The greeting (or "Notifications" while a phone shows them) and, on a phone, its bell. */
function HomeHeading({
  phone,
  notices,
  onToggleNotices
}: {
  phone: boolean;
  notices: boolean;
  onToggleNotices: () => void;
}) {
  const { data: user } = useCurrentUser();
  const timeZone = useWorkspaceTimezone();
  return (
    <div className='flex items-center justify-between gap-3'>
      <h1 className={cn(text2xl, "m-0 text-foreground")} data-testid='home-greeting'>
        {notices ? "Notifications" : homeGreeting(user?.name, new Date(), timeZone)}
      </h1>
      {phone && <PhoneBell open={notices} onToggle={onToggleNotices} />}
    </div>
  );
}

/** Owners and admins manage an app's access from its card; the server re-checks. */
function useManageAccess() {
  const org = useCurrentOrg((s) => s.org);
  const orgRole = useCurrentOrg((s) => s.role);
  const [managing, setManaging] = useState<CustomAppSummary | null>(null);
  const officer = orgRole === "owner" || orgRole === "admin";
  return { orgId: org && officer ? org.id : null, managing, setManaging };
}

/** The access dialog the card's gear opens, in the reference design. */
function HomeAccessDialog({ access }: { access: ReturnType<typeof useManageAccess> }) {
  if (!access.orgId) return null;
  const { managing, setManaging } = access;
  return (
    <AppAccessDialog
      open={managing !== null}
      onOpenChange={(open) => !open && setManaging(null)}
      scope={{ kind: "org", orgId: access.orgId }}
      appId={managing?.id ?? null}
      appName={managing?.name ?? ""}
      reference
    />
  );
}

/**
 * `navigate(HOME, { state: { prefillQuestion } })` callers open the Ask dock
 * prefilled, as they do on every home, once the workspace is ready.
 */
function usePrefilledAsk(ready: boolean) {
  const location = useLocation();
  const navigate = useNavigate();
  const openAsk = useAskDock((s) => s.open);
  const state = location.state as {
    prefillQuestion?: string;
    agentPath?: string;
    autoSubmit?: boolean;
  } | null;
  useEffect(() => {
    if (!ready) return;
    if (!state?.prefillQuestion && !state?.agentPath) return;
    openAsk({
      message: state.prefillQuestion,
      agentPath: state.agentPath,
      autoSubmit: state.autoSubmit
    });
    navigate(location.pathname + location.search, { replace: true, state: null });
  }, [ready, state, openAsk, navigate, location.pathname, location.search]);
}
