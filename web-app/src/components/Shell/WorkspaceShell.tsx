import { Handshake, House, MessagesSquare, Shield } from "lucide-react";
import type { ReactNode } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { AskDock } from "@/components/Ask/AskDock";
import { OxygenFactoryMark } from "@/components/OxygenFactoryMark";
import { PreviewBar } from "@/components/PreviewBar";
import WorkspaceStatus from "@/components/WorkspaceStatus";
import { useCustomApps } from "@/hooks/api/customApps/useCustomApps";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { appWindowName } from "@/libs/utils/appWindowName";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import { RailUserMenu } from "./RailUserMenu";
import { RailWorkspaceSwitch } from "./RailWorkspaceSwitch";
import { RailWorkspaceTile } from "./RailWorkspaceTile";
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
  const ws = ROUTES.ORG(orgSlug).WORKSPACE(wsId);

  const { data: customApps = [] } = useCustomApps(wsId);
  const { data: profile } = useCurrentUser();
  // Global Owners (`OXY_OWNER`) and Global Admins (`app_admins` table) get a
  // one-click hop into the admin console. Tenant-internal org roles never do —
  // this mirrors the gate on the admin routes themselves.
  const isOperator = !!(profile?.is_owner || profile?.is_app_admin);
  // Partner admins (users who administer ≥1 partner) get a console hop. The
  // server enforces scope; this only decides whether to show the entry.
  const isPartnerAdmin = !!profile?.partner_memberships?.length;

  const path = location.pathname;
  const inIde = /\/ide(\/|$)/.test(path);
  const hideRail = inIde || /\/onboarding(\/|$)/.test(path);
  // The IDE renders its own ProjectStatus — don't double-banner it.
  const hideStatus = inIde;

  // In local mode ws.ROOT is "" — the index route is "/".
  const isHome = path === ws.HOME || path === (ws.ROOT || "/");

  // Conceptual groups (divided in the rail): Home + Chat · Apps. (World Model
  // moved into Oxygen Factory as its own IDE sidebar surface.)
  const hq: RailItem = {
    key: "hq",
    label: "HQ",
    testId: "rail-hq",
    icon: <House className='h-4 w-4' />,
    active: isHome,
    onSelect: () => navigate(ws.HOME)
  };
  // The Chat landing (composer + recent threads) reached at /threads. Replaces
  // the old "Threads" list item; "Automations" was removed from the rail.
  const chat: RailItem = {
    key: "chat",
    label: "Chat",
    testId: "rail-chat",
    icon: <MessagesSquare className='h-4 w-4' />,
    active: path.startsWith(ws.THREADS),
    onSelect: () => navigate(ws.THREADS)
  };
  // Apps open in their own tab, never inside the shell — see `appWindowName`
  // for why HQ must not host an app in its own browsing context.
  const appItems: RailItem[] = customApps.map((app) => ({
    key: app.id,
    label: app.name,
    testId: `rail-app-${app.slug}`,
    letter: app.name.slice(0, 1).toUpperCase(),
    imageUrl: app.icon_url,
    href: app.url,
    newTab: appWindowName(app.org_slug, app.slug)
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
    onSelect: () => navigate(ws.IDE.ROOT)
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
    onSelect: () => navigate("/admin/apps")
  };

  // Partner console entry — shown to partner admins (non-operators reach it
  // here; the server enforces scope on every call).
  const partner: RailItem = {
    key: "partner",
    label: "Partners",
    tooltip: "Partner console — manage your partner's organizations, members, and apps",
    testId: "rail-partner",
    icon: <Handshake className='h-4 w-4' />,
    active: path.startsWith("/partners"),
    onSelect: () => navigate("/partners")
  };

  // Home + Chat share one block (no divider — both are primary HQ nav); apps get
  // their own divided group when the workspace has any.
  const groups: RailItem[][] = appItems.length ? [[hq, chat], appItems] : [[hq, chat]];
  // System zone: Oxygen Factory, then the Partner console (if any) and the
  // operator-only Admin hop below it.
  const footerItems: RailItem[] = [core];
  if (isPartnerAdmin) footerItems.push(partner);
  if (isOperator) footerItems.push(admin);

  const layout = (
    <div className='flex min-h-0 w-full flex-1'>
      {!hideRail && (
        <ShellRail
          top={<RailWorkspaceTile />}
          groups={groups}
          footerItems={footerItems}
          bottom={
            <>
              <RailWorkspaceSwitch />
              <RailUserMenu />
            </>
          }
        />
      )}
      {/* Content column. The top bar sits to the RIGHT of the rail and is the
          same height as the rail's logo cell (h-12), so the logo anchors the
          top-left corner and the two bottom borders form one continuous line. */}
      <div className='flex h-full min-w-0 flex-1 flex-col'>
        {!hideRail && <TopBar />}
        {/* Follows the operator INTO the tenant — this is where an unnoticed
            impersonation would actually do damage. */}
        <div className='flex min-h-0 w-full flex-1'>
          <main className='relative flex h-full min-w-0 flex-1 flex-col bg-background'>
            {!hideStatus && <WorkspaceStatus />}
            <div className='w-full min-w-0 flex-1 overflow-hidden'>{children}</div>
          </main>
          {/* The Ask dock is a flex sibling — opening it compacts <main>
              (Cursor-style) rather than floating over it. Custom apps
              deliberately get no equivalent: they open in their own tab. */}
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
    </div>
  );
}
