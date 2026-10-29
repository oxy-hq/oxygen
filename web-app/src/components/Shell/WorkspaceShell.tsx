import { Handshake, House, MessagesSquare, Shield } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { AskDock } from "@/components/Ask/AskDock";
import { OpenFactoryDialog } from "@/components/admin/OpenFactoryDialog";
import { OxygenFactoryMark } from "@/components/OxygenFactoryMark";
import { PreviewBar } from "@/components/PreviewBar";
import WorkspaceStatus from "@/components/WorkspaceStatus";
import { useActingSession } from "@/hooks/api/adminAssume/useActingSession";
import useOrgCustomApps from "@/hooks/api/customApps/useOrgCustomApps";
import { useAppUnreadCounts } from "@/hooks/api/notifications";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import { canUseFactory } from "@/libs/utils/factoryAccess";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import { RailUserMenu } from "./RailUserMenu";
import { RailWorkspaceSwitch } from "./RailWorkspaceSwitch";
import { RailWorkspaceTile } from "./RailWorkspaceTile";
import { ShellNavSheet } from "./ShellNavSheet";
import { type RailItem, ShellRail } from "./ShellRail";
import { TopBar } from "./TopBar";

/** The workspace chrome: icon rail + universal top bar + content column + Ask
 *  dock. Wraps every workspace route. The rail, top bar, and dock hide inside
 *  Oxygen Factory / the IDE (it has its own chrome) and on the onboarding
 *  wizard; the content column is identical everywhere so pages never re-layout
 *  between routes. */
export function WorkspaceShell({ children }: { children: ReactNode }) {
  const location = useLocation();
  const navigate = useNavigate();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const { workspace } = useCurrentWorkspace();
  const wsId = workspace?.id ?? "";
  const org = ROUTES.ORG(orgSlug);
  const ws = org.WORKSPACE(wsId);

  // The rail's app marks are the org's: every app the person can open across its
  // workspaces, the same list the org home's grid shows.
  // With the apps-first experience off, the current workspace's apps, as before.
  const appsFirst = useAppsFirstExperience();
  const { apps: customApps } = useOrgCustomApps(wsId, { acrossOrg: appsFirst });
  // Apps-first only: each app's unread count as a badge on its mark, like a dock.
  // The bell's own poll, so it costs no request.
  const unreadByApp = useAppUnreadCounts(appsFirst);
  const { data: profile } = useCurrentUser();
  // Global Owners (`OXY_OWNER`) and Global Admins (`app_admins` table) get a
  // one-click hop into the admin console. Tenant-internal org roles never do —
  // this mirrors the gate on the admin routes themselves.
  const isOperator = !!(profile?.is_owner || profile?.is_app_admin);
  // The Factory admits staff by `operate_platform` (`Ring::Factory`), which an App
  // Operator does not hold although `is_app_admin` is true for one. A Global Owner
  // reports every capability; `is_owner` covers a response that omits the list.
  const operatesPlatform = !!(
    profile?.is_owner || profile?.platform_capabilities?.includes("operate_platform")
  );
  // Partner admins (users who administer ≥1 partner) get a console hop. The
  // server enforces scope; this only decides whether to show the entry.
  const isPartnerAdmin = !!profile?.partner_memberships?.length;
  const [openFactoryOpen, setOpenFactoryOpen] = useState(false);
  const currentOrg = useCurrentOrg((s) => s.org);
  const { session, isActing } = useActingSession();

  const path = location.pathname;
  const inIde = /\/ide(\/|$)/.test(path);
  const hideRail = inIde || /\/onboarding(\/|$)/.test(path);
  // The IDE renders its own ProjectStatus — don't double-banner it.
  const hideStatus = inIde;

  // In local mode ws.ROOT is "" — the index route is "/".
  const isHome = path === org.HOME || path === ws.HOME || path === (ws.ROOT || "/");
  // The read-only world model (apps-first) is opened from the home's card and has
  // no rail entry of its own, so Home stays lit there.
  const onWorldModel = appsFirst && path === ws.WORLD_MODEL;

  // Conceptual groups (divided in the rail): Home + Chat · Apps. (World Model
  // moved into Oxygen Factory as its own IDE sidebar surface.)
  const hq: RailItem = {
    key: "hq",
    label: appsFirst ? "Home" : "HQ",
    testId: "rail-hq",
    icon: <House className='h-4 w-4' />,
    active: isHome || onWorldModel,
    onSelect: () => {
      // The org home is the apps-first experience's; without it, the workspace's.
      navigate(appsFirst ? org.HOME : ws.HOME);
    }
  };
  // The Chat landing (composer + recent threads) reached at /threads. Replaces
  // the old "Threads" list item; "Automations" was removed from the rail.
  const chat: RailItem = {
    key: "chat",
    label: "Chat",
    testId: "rail-chat",
    icon: <MessagesSquare className='h-4 w-4' />,
    active: path.startsWith(ws.THREADS),
    onSelect: () => {
      navigate(ws.THREADS);
    }
  };
  // A tile navigates this tab to the app, which owns the address bar and draws
  // the same rail (`OxyShell`), so moving between HQ and apps reads as one
  // product. Never hosted inside HQ: apps-first-shell.md § 4.1.
  const appItems: RailItem[] = customApps.map((app) => ({
    key: app.id,
    label: app.name,
    testId: `rail-app-${app.slug}`,
    letter: app.name.slice(0, 1).toUpperCase(),
    imageUrl: app.icon_url,
    href: app.url,
    badge: appsFirst ? unreadByApp[app.id] : undefined
  }));
  // System: the intelligence substrate powering the HQ. Pinned at the
  // bottom of the nav, distinct from the operator apps above.
  const core: RailItem = {
    key: "core",
    label: "Oxygen Factory",
    tooltip:
      "Oxygen Factory — the intelligence system behind your HQ: data sources, business model, agents, automations, and deployments",
    testId: "rail-core",
    icon: <OxygenFactoryMark className='h-6 w-6' />,
    active: path.startsWith(ws.IDE.ROOT),
    onSelect: () => {
      navigate(ws.IDE.ROOT);
    }
  };
  // Admin console entry — pinned directly beneath Oxygen Factory in the system
  // zone, visible only to operators. Full-page-ish SPA nav to the customer-apps
  // console (the default admin landing).
  const admin: RailItem = {
    key: "admin",
    label: "Admin",
    tooltip: "Admin console — custom apps, tenants, feature flags, jobs",
    testId: "rail-admin",
    icon: <Shield className='h-4 w-4' />,
    active: path.startsWith("/admin"),
    onSelect: () => {
      navigate("/admin/apps");
    }
  };

  // "Open Factory…" (apps-first, staff who operate the platform only): this org's
  // Factory in one click.
  // Acting as this org already, it goes straight in; otherwise it opens the Open
  // Factory flow on this org's workspaces (one workspace: straight to the reason),
  // with a way to search every organization. It replaces the plain Factory entry
  // for operators, who would otherwise see two.
  const actingHere = isActing && !!currentOrg && session?.org_id === currentOrg.id;
  const openFactory: RailItem = {
    key: "open-factory",
    label: "Open Factory…",
    tooltip: currentOrg
      ? `Open ${currentOrg.name}'s Factory, or search every organization`
      : "Open an organization's Factory",
    testId: "rail-open-factory",
    icon: <OxygenFactoryMark className='h-6 w-6' />,
    active: path.startsWith(ws.IDE.ROOT),
    onSelect: () => {
      if (actingHere && wsId) {
        navigate(ws.IDE.ROOT);
        return;
      }
      setOpenFactoryOpen(true);
    }
  };
  const staffFactory = appsFirst && operatesPlatform;
  // Stable, so the dialog does not start over on every render while it is open.
  const factoryScope = useMemo(
    () =>
      currentOrg
        ? { orgId: currentOrg.id, orgSlug: currentOrg.slug, orgName: currentOrg.name }
        : null,
    [currentOrg]
  );

  // Partner console entry — shown to partner admins (non-operators reach it
  // here; the server enforces scope on every call).
  const partner: RailItem = {
    key: "partner",
    label: "Partners",
    tooltip: "Partner console — manage your partner's organizations, members, and apps",
    testId: "rail-partner",
    icon: <Handshake className='h-4 w-4' />,
    active: path.startsWith("/partners"),
    onSelect: () => {
      navigate("/partners");
    }
  };

  // Home + Chat share one block (no divider — both are primary HQ nav); apps get
  // their own divided group when the workspace has any.
  const groups: RailItem[][] = appItems.length ? [[hq, chat], appItems] : [[hq, chat]];
  // System zone: Oxygen Factory, then the Partner console (if any) and the
  // operator-only Admin hop below it.
  // The Factory is for Oxy staff, the org's owners and admins, and the members an
  // operator granted it to. The
  // server says (`factory_access` on the workspace); this only decides whether to
  // show the entry — the routes refuse everyone else on their own.
  const footerItems: RailItem[] = staffFactory
    ? [openFactory]
    : canUseFactory(workspace)
      ? [core]
      : [];
  if (isPartnerAdmin) footerItems.push(partner);
  if (isOperator) footerItems.push(admin);

  const bottom = (
    <>
      {/* Apps-first drops the switcher: the home lists every app across
          the org, and Oxy engineers move between an org's workspaces in
          the Factory's own picker. */}
      {!appsFirst && <RailWorkspaceSwitch />}
      <RailUserMenu />
    </>
  );

  const layout = (
    <div className='flex min-h-0 w-full flex-1'>
      {/* Below `md` the rail gives way to ShellNavSheet: its labels are hover
          tooltips, which a touch screen never shows, and its 48px is an eighth
          of a phone's width. */}
      {!hideRail && (
        <div className='hidden h-full md:flex'>
          <ShellRail
            top={<RailWorkspaceTile />}
            groups={groups}
            footerItems={footerItems}
            bottom={bottom}
          />
        </div>
      )}
      {/* Content column. The top bar sits to the RIGHT of the rail and is the
          same height as the rail's logo cell (h-12), so the logo anchors the
          top-left corner and the two bottom borders form one continuous line. */}
      <div className='flex h-full min-w-0 flex-1 flex-col'>
        {!hideRail && (
          <TopBar
            leading={
              <div className='flex md:hidden'>
                <ShellNavSheet
                  title={workspace?.name ?? ""}
                  top={<RailWorkspaceTile />}
                  groups={groups}
                  footerItems={footerItems}
                  bottom={bottom}
                />
              </div>
            }
          />
        )}
        {/* Follows the operator INTO the tenant — this is where an unnoticed
            impersonation would actually do damage. */}
        <div className='flex min-h-0 w-full flex-1'>
          <main className='relative flex h-full min-w-0 flex-1 flex-col bg-background'>
            {!hideStatus && <WorkspaceStatus />}
            <div className='w-full min-w-0 flex-1 overflow-hidden'>{children}</div>
          </main>
          {/* The Ask dock is a flex sibling — opening it compacts <main>
              (Cursor-style) rather than floating over it. A custom app is not
              hosted here: it opens in this tab and draws its own dock and Ask
              (`OxyShell`). */}
          {!hideRail && <AskDock />}
        </div>
      </div>
    </div>
  );

  // The preview bar sits above everything — rail, top bar and the IDE's own
  // chrome alike: while a page is pinned to a preview it is the only thing
  // saying so. It renders nothing on a live page.
  return (
    <div className='flex h-full w-full flex-col'>
      <PreviewBar />
      {layout}
      {staffFactory && (
        <OpenFactoryDialog
          open={openFactoryOpen}
          onOpenChange={setOpenFactoryOpen}
          initialScope={factoryScope}
        />
      )}
    </div>
  );
}
