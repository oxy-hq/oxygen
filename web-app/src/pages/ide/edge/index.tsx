import { Boxes, ChevronDown, Settings } from "lucide-react";
import type React from "react";
import { NavLink, Outlet, useLocation, useSearchParams } from "react-router-dom";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import SitePicker from "./components/SitePicker";

/**
 * Edge tab shell. Houses every camera-fleet surface under one route
 * so the operator stops hunting between Settings and the IDE for
 * related actions.
 *
 * Tab grouping: Fleet, Timeline, and other day-to-day operator tabs
 * are siblings. Rarely-touched config (Rollouts / Audit / Domain pack)
 * lives behind a Settings dropdown so the top nav stays scannable
 * even as more tabs land.
 */
const EdgeLayout: React.FC = () => {
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const edge = ROUTES.ORG(orgSlug).WORKSPACE().IDE.EDGE;
  const { pathname } = useLocation();
  // Carry global selections (notably `?site=` from `SitePicker`)
  // across tab navigation. Without this, NavLinks below would drop
  // search params on every click and the operator would reset to
  // "All sites" every time they switched tabs. Only the keys
  // explicitly listed here propagate — per-tab UI state (`?tab=`
  // on the EdgeBoxDetailPage, etc.) is intentionally NOT carried
  // so each surface owns its own scoped params.
  const [searchParams] = useSearchParams();
  const carry = new URLSearchParams();
  const carriedSite = searchParams.get("site");
  if (carriedSite) carry.set("site", carriedSite);
  const withCarry = (to: string) =>
    carry.toString().length > 0 ? `${to}?${carry.toString()}` : to;

  // Top-level tabs — high-frequency operator surfaces. Ordered by
  // expected daily attention: Dashboard ("glance and leave") first,
  // then Playback (where operators sit), then Detections (analyst
  // flow), then Topology (visual reference), then Devices (config
  // surface, less frequent than the four above).
  const tabs = [
    { to: edge.ROOT, label: "Dashboard", end: true },
    { to: edge.PLAYBACK, label: "Playback" },
    { to: edge.DETECTIONS, label: "Detections" },
    { to: edge.TOPOLOGY, label: "Topology" },
    { to: edge.DEVICES, label: "Devices" }
  ];

  // Settings dropdown contents — config surfaces. Order matches
  // expected frequency-of-use (Rollouts most, Pack least).
  const settingsItems = [
    { to: edge.ROLLOUTS, label: "Rollouts" },
    { to: edge.AUDIT, label: "Audit" },
    { to: edge.PACK, label: "Domain pack" }
  ];
  const settingsActive = settingsItems.some((s) => pathname.startsWith(s.to));

  return (
    <div
      className='flex h-full min-h-0 min-w-0 flex-1 flex-col bg-background'
      data-testid='edge-layout'
    >
      <header className='flex shrink-0 items-center gap-3 border-border border-b px-4 py-2.5'>
        <div className='flex items-center gap-2'>
          <Boxes className='h-4 w-4 text-primary' />
          <span className='font-semibold text-sm'>Edge</span>
        </div>
        {/* Site picker — global scope filter, persists in URL via
            ?site=. Lives between the section label and tab nav so
            it reads as "context for everything to the right." */}
        <SitePicker />
        <nav className='flex items-center gap-1'>
          {tabs.map((t) => (
            <NavLink
              key={t.to}
              to={withCarry(t.to)}
              end={t.end}
              className={({ isActive }) =>
                cn(
                  "rounded-md px-2.5 py-1 font-medium text-sm transition-colors",
                  isActive
                    ? "bg-sidebar-accent text-sidebar-accent-foreground"
                    : "text-muted-foreground hover:bg-muted hover:text-foreground"
                )
              }
            >
              {t.label}
            </NavLink>
          ))}

          <DropdownMenu>
            <DropdownMenuTrigger
              className={cn(
                "flex items-center gap-1 rounded-md px-2.5 py-1 font-medium text-sm outline-none transition-colors",
                settingsActive
                  ? "bg-sidebar-accent text-sidebar-accent-foreground"
                  : "text-muted-foreground hover:bg-muted hover:text-foreground"
              )}
              data-testid='edge-settings-trigger'
            >
              <Settings className='h-3.5 w-3.5' />
              Settings
              <ChevronDown className='h-3 w-3' />
            </DropdownMenuTrigger>
            <DropdownMenuContent align='start' className='min-w-40'>
              {settingsItems.map((s) => (
                <DropdownMenuItem key={s.to} asChild>
                  <NavLink
                    to={withCarry(s.to)}
                    className={({ isActive }) =>
                      cn("cursor-pointer", isActive && "bg-accent text-accent-foreground")
                    }
                  >
                    {s.label}
                  </NavLink>
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        </nav>
      </header>
      <div className='min-h-0 flex-1 overflow-auto'>
        <Outlet />
      </div>
    </div>
  );
};

export default EdgeLayout;
