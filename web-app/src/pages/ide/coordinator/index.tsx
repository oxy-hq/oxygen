import { HeartPulse, Inbox, Radar, Settings2 } from "lucide-react";
import type React from "react";
import { Link, Outlet, useLocation } from "react-router-dom";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";

/**
 * Coordinator → Orchestrator dashboard shell.
 *
 * Three domain tabs (Overview / Jobs / Runs). Job-detail and run-detail are
 * drill-down pages reached by clicking through, never tabs. Recovery and
 * Queue Health are operator internals — tucked behind a System menu so the
 * primary nav stays focused on the three questions the tabs answer.
 */

interface TabDef {
  label: string;
  to: string;
  /** Pathname fragment that keeps this tab lit (incl. its drill-downs). */
  match: string;
  /** `data-testid` for agentic browser flows — the load-bearing nav anchor. */
  testId: string;
}

const CoordinatorTab: React.FC<{ tab: TabDef; active: boolean }> = ({ tab, active }) => (
  <Link
    to={tab.to}
    data-testid={tab.testId}
    className={cn(
      "relative -mb-px border-b-2 px-1 py-2.5 font-medium text-sm transition-colors",
      active
        ? "border-primary text-foreground"
        : "border-transparent text-muted-foreground hover:text-foreground"
    )}
  >
    {tab.label}
  </Link>
);

const CoordinatorLayout: React.FC = () => {
  const location = useLocation();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const coord = ROUTES.ORG(orgSlug).WORKSPACE().IDE.COORDINATOR;

  const tabs: TabDef[] = [
    {
      label: "Overview",
      to: coord.OVERVIEW,
      match: "/coordinator/overview",
      testId: "coordinator-tab-overview"
    },
    {
      label: "Jobs",
      to: coord.JOBS,
      match: "/coordinator/jobs",
      testId: "coordinator-tab-jobs"
    },
    {
      label: "Runs",
      to: coord.RUNS,
      match: "/coordinator/runs",
      testId: "coordinator-tab-runs"
    }
  ];

  return (
    <div className='flex h-full min-h-0 min-w-0 flex-1 flex-col bg-background'>
      <header className='flex shrink-0 items-center gap-6 border-border border-b px-4'>
        <div className='flex items-center gap-2 py-2.5'>
          <Radar className='h-4 w-4 text-primary' />
          <span className='font-semibold text-sm'>Orchestrator</span>
        </div>
        <nav className='flex items-center gap-5'>
          {tabs.map((tab) => (
            <CoordinatorTab key={tab.to} tab={tab} active={location.pathname.includes(tab.match)} />
          ))}
        </nav>
        <div className='ml-auto'>
          <DropdownMenu>
            <DropdownMenuTrigger
              data-testid='coordinator-system-menu'
              className={cn(
                "inline-flex items-center gap-1.5 rounded-md px-2 py-1.5 text-muted-foreground text-xs",
                "hover:bg-muted hover:text-foreground"
              )}
            >
              <Settings2 className='h-3.5 w-3.5' />
              System
            </DropdownMenuTrigger>
            <DropdownMenuContent align='end'>
              <DropdownMenuLabel>Operator internals</DropdownMenuLabel>
              <DropdownMenuSeparator />
              <DropdownMenuItem asChild>
                <Link to={coord.RECOVERY}>
                  <HeartPulse className='h-4 w-4' />
                  Recovery &amp; reliability
                </Link>
              </DropdownMenuItem>
              <DropdownMenuItem asChild>
                <Link to={coord.QUEUE}>
                  <Inbox className='h-4 w-4' />
                  Queue health
                </Link>
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </header>
      <div className='min-h-0 flex-1 overflow-hidden'>
        <Outlet />
      </div>
    </div>
  );
};

export default CoordinatorLayout;
