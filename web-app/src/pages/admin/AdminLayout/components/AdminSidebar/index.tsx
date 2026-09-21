import { Link, useLocation } from "react-router-dom";
import OxyLogo from "@/components/OxyLogo";
import {
  Sidebar as ShadcnSidebar,
  SidebarGroup,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem
} from "@/components/ui/shadcn/sidebar";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { useWorkspaceHealth } from "@/hooks/api/workspaceHealth/useWorkspaceHealth";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { ADMIN_NAV, ADMIN_NAV_GROUPS, itemReachable } from "../../adminNav";
import { Footer } from "./components/Footer";

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
  const { data: health } = useWorkspaceHealth();
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
    <ShadcnSidebar className='border-sidebar-border border-r bg-sidebar-background'>
      <div className='flex h-[52px] shrink-0 items-center gap-2 border-sidebar-border/50 border-b px-3'>
        <Link to={logoTarget} className='flex shrink-0 items-center'>
          <OxyLogo />
        </Link>
        <span className='rounded-sm border border-sidebar-border px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground uppercase tracking-[0.2em]'>
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
                {items.map(({ to, label, icon: Icon }) => {
                  // Tenant items carry a `?type=` query and share one pathname,
                  // so match on the active type; everything else matches by path.
                  const [itemPath, itemQuery] = to.split("?");
                  const itemType = itemQuery ? new URLSearchParams(itemQuery).get("type") : null;
                  const isActive = itemType
                    ? location.pathname === ROUTES.ADMIN.TENANTS && currentTenantType === itemType
                    : location.pathname.startsWith(itemPath);
                  const showHealthBadge =
                    to === ROUTES.ADMIN.WORKSPACE_HEALTH && attentionCount > 0;
                  return (
                    <SidebarMenuItem key={to}>
                      <SidebarMenuButton
                        asChild
                        isActive={isActive}
                        className='gap-2.5 text-[13px] data-[active=true]:font-medium [&>svg]:size-3.5'
                      >
                        <Link to={to}>
                          <Icon />
                          <span className='tracking-tight'>{label}</span>
                          {showHealthBadge && (
                            <span
                              data-testid='workspace-health-nav-badge'
                              title={`${attentionCount} workspace${attentionCount === 1 ? "" : "s"} need attention`}
                              className={cn(
                                "ml-auto inline-flex h-4 min-w-4 items-center justify-center rounded-full px-1 font-medium text-[10px] tabular-nums ring-1 ring-inset",
                                hasUnhealthy
                                  ? cn(
                                      ADMIN_TONE.danger.bg,
                                      ADMIN_TONE.danger.text,
                                      ADMIN_TONE.danger.ring
                                    )
                                  : cn(
                                      ADMIN_TONE.warn.bg,
                                      ADMIN_TONE.warn.text,
                                      ADMIN_TONE.warn.ring
                                    )
                              )}
                            >
                              {attentionCount}
                            </span>
                          )}
                        </Link>
                      </SidebarMenuButton>
                    </SidebarMenuItem>
                  );
                })}
              </SidebarMenu>
            </SidebarGroup>
          );
        })}
      </div>

      <Footer />
    </ShadcnSidebar>
  );
}
