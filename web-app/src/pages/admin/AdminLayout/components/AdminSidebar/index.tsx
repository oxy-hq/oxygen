import { Link, useLocation } from "react-router-dom";
import OxyLogo from "@/components/OxyLogo";
import {
  Sidebar as ShadcnSidebar,
  SidebarGroup,
  SidebarGroupLabel,
  SidebarMenu
} from "@/components/ui/shadcn/sidebar";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { useWorkspaceHealth } from "@/hooks/api/workspaceHealth/useWorkspaceHealth";
import ROUTES from "@/libs/utils/routes";
import { ADMIN_NAV, ADMIN_NAV_GROUPS, itemReachable, navItemReachable } from "../../adminNav";
import { Footer } from "./components/Footer";
import { NavItem } from "./components/NavItem";

/**
 * Whether a rail entry is the page you are on.
 *
 * Tenant entries carry a `?type=` query and share one pathname, so they match on the
 * active type; everything else matches by path prefix.
 */
function isItemActive(to: string, pathname: string, tenantType: string): boolean {
  const [itemPath, itemQuery] = to.split("?");
  const itemType = itemQuery ? new URLSearchParams(itemQuery).get("type") : null;
  if (itemType) return pathname === ROUTES.ADMIN.TENANTS && tenantType === itemType;
  return pathname.startsWith(itemPath);
}

export function AdminSidebar() {
  const location = useLocation();
  // The directory's active entity type, so the tenant nav shortcuts light up in
  // lockstep with the directory's own header switcher (both read `?type=`).
  const currentTenantType = new URLSearchParams(location.search).get("type") ?? "orgs";
  const { data: user } = useCurrentUser();
  const isOwner = user?.is_owner ?? false;
  const capabilities = user?.platform_capabilities ?? [];

  // Surface a count of workspaces needing attention right on the nav item,
  // so operators see trouble without opening the Workspace health page.
  // Same 30s-stale rollup the health page reads — worst-first, cross-tenant.
  // Only for someone the item is shown to: the endpoint is `operate_platform`,
  // and an App Operator fetching it got a 403 toast on every admin page.
  const { data: health } = useWorkspaceHealth({
    enabled: navItemReachable(ROUTES.ADMIN.WORKSPACE_HEALTH, { isOwner, capabilities })
  });
  const attentionCount = health?.workspaces.filter((ws) => ws.status !== "healthy").length ?? 0;
  const hasUnhealthy = health?.workspaces.some((ws) => ws.status === "unhealthy") ?? false;

  // One rule per item, in the same order the server applies them: owner-only rooms are
  // a boolean the capability model deliberately cannot reach (the Billing queue and the
  // grant table itself); everything else asks for a capability. An item with neither is
  // open to any staff member who got through the console door.
  // Per item, deliberately — `canReachAdminRoute` answers a different question (see its
  // doc): on `/admin/tenants` any one of three capabilities admits you to the page, but
  // each rail link still shows only to whoever holds its own.
  const visibleItems = ADMIN_NAV.filter((item) => itemReachable(item, { isOwner, capabilities }));

  // The logo goes to the console home, which every staff member who got through the
  // door can open — it gates its own sections. `firstReachableAdminRoute` stays the
  // answer to a different question (where to send someone off a route they can't be on).
  const logoTarget = ROUTES.ADMIN.ROOT;

  return (
    <ShadcnSidebar
      // **`icon`, not the default `offcanvas`** — the same bargain the product's rail
      // strikes (`components/Shell/ToggleRail`): closed it is a 3rem strip of marks, not
      // nothing. Off-canvas bought 256px by taking the navigation away with it, so the
      // toggle was a choice between a wide rail and no rail; now it is a choice between
      // names and no names, and ⌘B is worth pressing on a wide screen.
      collapsible='icon'
      className='border-sidebar-border border-r bg-sidebar-background'
    >
      <div className='flex h-[52px] shrink-0 items-center gap-2 border-sidebar-border/50 border-b px-3 group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:px-0'>
        <Link to={logoTarget} className='flex shrink-0 items-center'>
          <OxyLogo />
        </Link>
        {/* The chip is the one thing with no room at 3rem, and the logo already says
            where you are. */}
        <span className='rounded-sm border border-sidebar-border px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground uppercase tracking-[0.2em] group-data-[collapsible=icon]:hidden'>
          Admin
        </span>
      </div>

      <div className='min-h-0 flex-1 overflow-auto'>
        {(Object.keys(ADMIN_NAV_GROUPS) as (keyof typeof ADMIN_NAV_GROUPS)[]).map((group) => {
          const items = visibleItems.filter((i) => i.group === group);
          if (items.length === 0) return null;
          return (
            <SidebarGroup key={group} className='px-2 pt-2'>
              <SidebarGroupLabel>{ADMIN_NAV_GROUPS[group]}</SidebarGroupLabel>
              <SidebarMenu>
                {items.map((item) => (
                  <NavItem
                    key={item.to}
                    to={item.to}
                    label={item.label}
                    icon={item.icon}
                    isActive={isItemActive(item.to, location.pathname, currentTenantType)}
                    attention={
                      item.to === ROUTES.ADMIN.WORKSPACE_HEALTH
                        ? { count: attentionCount, severe: hasUnhealthy }
                        : undefined
                    }
                  />
                ))}
              </SidebarMenu>
            </SidebarGroup>
          );
        })}
      </div>

      <Footer />
    </ShadcnSidebar>
  );
}
