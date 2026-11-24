import { Handshake, House, Shield } from "@phosphor-icons/react";
import type { ReactNode } from "react";
import { useMemo, useState } from "react";
import { type NavigateFunction, useLocation, useNavigate } from "react-router-dom";
import { OxygenFactoryMark } from "@/components/OxygenFactoryMark";
import { iconControl } from "@/components/ui/reference/tokens";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { useOpensFactoryDirectly, useOperatesPlatform } from "@/hooks/useStaffFactory";
import { canUseFactory } from "@/libs/utils/factoryAccess";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import type { CustomAppSummary } from "@/types/apps";
import type { RailItem } from "./RailItem";

/**
 * The shell's navigation, split out of `WorkspaceShell` so each piece stays small:
 * where the page is, the rail's groups (Home · the apps) and its foot (Oxygen
 * Nexus, Partners, Admin), and the staff Factory entry with its dialog's state.
 */

/** Where the page is, as the chrome reads it. */
export function useShellPlace() {
  const path = useLocation().pathname;
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const org = ROUTES.ORG(orgSlug);
  const ws = org.WORKSPACE();
  const inIde = /\/ide(\/|$)/.test(path);
  // The setup wizard owns the screen. Matched against its own address rather than
  // any `/setup` segment, which an id or a name in another page's address could carry.
  const onSetup = [ws.ONBOARDING, "/onboarding"].some(
    (p) => path === p || path.startsWith(`${p}/`)
  );
  // In local mode ws.ROOT is "" — the index route is "/".
  const isHome = [org.HOME, ws.HOME, ws.ROOT || "/"].includes(path);
  return {
    path,
    org,
    ws,
    hideRail: inIde || onSetup,
    isHome,
    // The read-only world model is opened from the home's card and has no rail
    // entry of its own, so Home stays lit there.
    homeLit: isHome || path === ws.WORLD_MODEL
  };
}

type NavEntry = { key: string; label: string; icon: ReactNode; to: string; active: boolean };

/** An entry that is an SPA hop to `to`. */
function navItem(entry: NavEntry, navigate: NavigateFunction): RailItem {
  return {
    key: entry.key,
    label: entry.label,
    testId: `rail-${entry.key}`,
    icon: entry.icon,
    active: entry.active,
    onSelect: () => {
      void navigate(entry.to);
    }
  };
}

/**
 * The apps, A to Z, so a long list stays findable, and no unread badges: the
 * bell's dot says there is news, its panel says where. A tile navigates this tab
 * to the app, which owns the address bar and draws the same rail (`OxyShell`), so
 * moving between HQ and apps reads as one product. Never hosted inside HQ:
 * apps-first-shell.md § 4.1.
 */
export function appRailItems(apps: readonly CustomAppSummary[]): RailItem[] {
  return [...apps]
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }))
    .map((app) => ({
      key: app.id,
      label: app.name,
      testId: `rail-app-${app.slug}`,
      letter: app.name.slice(0, 1).toUpperCase(),
      imageUrl: app.icon_url,
      href: app.url
    }));
}

/** Home on its own; the apps get their own divided group when the org has any. No
 *  Chat entry: the Chat page (`/threads`) opens by address only. */
export function useRailGroups(apps: readonly CustomAppSummary[]): RailItem[][] {
  const navigate = useNavigate();
  const { org, homeLit } = useShellPlace();
  const hq = navItem(
    {
      key: "hq",
      label: "Home",
      icon: <House className={iconControl} />,
      to: org.HOME,
      active: homeLit
    },
    navigate
  );
  const appItems = appRailItems(apps);
  return appItems.length ? [[hq], appItems] : [[hq]];
}

/**
 * The staff Factory entry (staff who operate the platform only): this workspace's
 * Factory in one click, like everyone else's entry. Only an org that needs an
 * assumed session first (staff who are neither a member nor acting as it) opens
 * the Open Factory flow, at this workspace's reason. Decided before anything is
 * drawn: a dialog that opened only to close again read as a glitch.
 */
export function useOpenFactoryEntry() {
  const navigate = useNavigate();
  const { path, ws } = useShellPlace();
  const { workspace } = useCurrentWorkspace();
  const currentOrg = useCurrentOrg((s) => s.org);
  const opensFactoryDirectly = useOpensFactoryDirectly();
  // The Factory admits staff by `operate_platform` (`Ring::Factory`), which an App
  // Operator does not hold although `is_app_admin` is true for one.
  const staff = useOperatesPlatform();
  const [open, setOpen] = useState(false);
  const wsId = workspace?.id ?? "";
  // Stable, so the dialog does not start over on every render while it is open.
  const scope = useMemo(
    () =>
      currentOrg
        ? { orgId: currentOrg.id, orgSlug: currentOrg.slug, orgName: currentOrg.name }
        : null,
    [currentOrg]
  );
  const workspaceName = workspace?.name;
  const target = useMemo(
    () => (scope && wsId && workspaceName ? { ...scope, workspaceId: wsId, workspaceName } : null),
    [scope, wsId, workspaceName]
  );
  const item: RailItem = {
    key: "open-factory",
    label: "Oxygen Nexus",
    testId: "rail-open-factory",
    icon: <OxygenFactoryMark className='size-5' />,
    active: path.startsWith(ws.IDE.ROOT),
    onSelect: () => {
      if (currentOrg && wsId && opensFactoryDirectly(currentOrg.id)) {
        navigate(ws.IDE.ROOT);
        return;
      }
      setOpen(true);
    }
  };
  return { staff, item, open, setOpen, scope, target };
}

/**
 * The rail's foot: Oxygen Nexus, then the Partner console (if any) and the
 * operator-only Admin hop below it. The Factory is for Oxy staff (`factory.item`),
 * the org's owners and admins, and the members an operator granted it to: the
 * server says (`factory_access` on the workspace); this only decides whether to
 * show the entry, the routes refuse everyone else on their own.
 */
export function useFooterItems(factory: { staff: boolean; item: RailItem }): RailItem[] {
  const navigate = useNavigate();
  const { path, ws } = useShellPlace();
  const { workspace } = useCurrentWorkspace();
  const { data: profile } = useCurrentUser();
  const items: RailItem[] = [];
  if (factory.staff) items.push(factory.item);
  else if (canUseFactory(workspace)) {
    items.push(
      navItem(
        {
          key: "core",
          label: "Oxygen Nexus",
          icon: <OxygenFactoryMark className='size-5' />,
          to: ws.IDE.ROOT,
          active: path.startsWith(ws.IDE.ROOT)
        },
        navigate
      )
    );
  }
  // Partner admins get a console hop; the server enforces scope on every call.
  if (profile?.partner_memberships?.length) {
    items.push(
      navItem(
        {
          key: "partner",
          label: "Partners",
          icon: <Handshake className={iconControl} />,
          to: "/partners",
          active: path.startsWith("/partners")
        },
        navigate
      )
    );
  }
  // Global Owners and Global Admins get a hop into the admin console; tenant org
  // roles never do, which mirrors the gate on the admin routes themselves.
  if (profile?.is_owner || profile?.is_app_admin) {
    items.push(
      navItem(
        {
          key: "admin",
          label: "Admin",
          icon: <Shield className={iconControl} />,
          to: "/admin/apps",
          active: path.startsWith("/admin")
        },
        navigate
      )
    );
  }
  return items;
}
