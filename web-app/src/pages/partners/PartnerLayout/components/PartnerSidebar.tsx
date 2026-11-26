import { AppWindow, Building2, ScrollText, UserCog } from "lucide-react";
import type { ComponentType } from "react";
import { Link, useLocation } from "react-router-dom";
import {
  SidebarGroup,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem
} from "@/components/ui/shadcn/sidebar";
import ROUTES from "@/libs/utils/routes";
import { ConsoleRail, consoleNavRow } from "@/pages/admin/components/ConsoleRail";
import { usePartnerConsole } from "../../context";

type NavItem = {
  to: string;
  label: string;
  icon: ComponentType<{ className?: string }>;
  /** Rendered only when this person can actually use it. */
  visible: (v: ReturnType<typeof usePartnerConsole>["active"]) => boolean;
};

/**
 * Same shape as the admin sidebar, because it is the same *kind* of surface: an
 * operations console for someone administering other people's organizations. The
 * only difference is reach.
 *
 * One flat, four-item list — Clients and Custom apps are the two nouns a partner
 * works in; Team and Activity are the supporting surfaces. Workspace health used
 * to be its own item, but it's inherently per-client, so it now lives inside each
 * client's detail. Items are capability-driven — an item nobody can use is not
 * rendered rather than rendered and then 403'd. (The server re-checks regardless.)
 */
const NAV: NavItem[] = [
  {
    to: ROUTES.PARTNERS.ROOT,
    label: "Clients",
    icon: Building2,
    visible: () => true
  },
  {
    to: ROUTES.PARTNERS.APPS,
    label: "Custom apps",
    icon: AppWindow,
    visible: (p) => p.capabilities.manage_apps
  },
  {
    to: ROUTES.PARTNERS.TEAM,
    label: "Team",
    icon: UserCog,
    // Visible to every operator. Granting access is the org owner/admin's job — the
    // server 403s a plain member's toggle — but the roster itself is fine to show.
    visible: () => true
  },
  {
    to: ROUTES.PARTNERS.ACTIVITY,
    label: "Activity",
    icon: ScrollText,
    visible: (p) => p.capabilities.view_audit
  }
];

export function PartnerSidebar() {
  const location = useLocation();
  const { active } = usePartnerConsole();
  const items = NAV.filter((i) => i.visible(active));

  return (
    <ConsoleRail badge='Partner' logoTo={ROUTES.PARTNERS.ROOT}>
      <SidebarGroup className='px-2 pt-2'>
        <SidebarGroupLabel>{active.name}</SidebarGroupLabel>
        <SidebarMenu>
          {items.map(({ to, label, icon: Icon }) => (
            <SidebarMenuItem key={to}>
              <SidebarMenuButton
                asChild
                // ROOT is a prefix of every other partner route, so it only
                // counts as active on an exact match.
                isActive={
                  to === ROUTES.PARTNERS.ROOT
                    ? location.pathname === to
                    : location.pathname.startsWith(to)
                }
                // Required, not decoration: `ConsoleRail` collapses to a 3rem strip, and
                // at that width shadcn hides the row label and the group label, so the
                // tooltip is the only thing left that can name a row. Without it a
                // collapsed rail is four unlabelled icons. The admin rail's `NavItem`
                // passes the same.
                tooltip={label}
                className={consoleNavRow}
              >
                <Link to={to}>
                  <Icon />
                  <span className='tracking-tight'>{label}</span>
                </Link>
              </SidebarMenuButton>
            </SidebarMenuItem>
          ))}
        </SidebarMenu>
      </SidebarGroup>
    </ConsoleRail>
  );
}
