import {
  Activity,
  Database,
  KeyRound,
  LayoutDashboard,
  type LucideIcon,
  Radio,
  Rocket,
  Settings,
  SquareFunction
} from "lucide-react";
import { useMemo } from "react";
import { Link, useLocation } from "react-router-dom";
import { useAppEventTypes } from "@/hooks/api/customApps/useAppEvents";
import { useAppIssues } from "@/hooks/api/customApps/useAppIssues";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { CustomApp } from "@/types/apps";
import { liveIssueCount } from "../AppDetail/components/Issues/issueBrief";
import { CONSOLE_PAGES, type ConsolePageId, consolePagePath } from "./consolePages";
import { eventsView } from "./Events/eventsView";

const ICON: Record<ConsolePageId, LucideIcon> = {
  overview: LayoutDashboard,
  activity: Activity,
  functions: SquareFunction,
  events: Radio,
  releases: Rocket,
  data: Database,
  secrets: KeyRound,
  settings: Settings
};

/**
 * The app's own sidebar: one entry per console page, the way Encore lists an app's
 * pages under its switcher. A count appears beside a page only when it is something
 * wrong (issues on the live build, dead event deliveries), so a quiet app's nav is just
 * names.
 *
 * At phone width it is a row that scrolls sideways above the page, not a column beside
 * it.
 */
export const ConsoleNav = ({
  app,
  page,
  environment
}: {
  app: CustomApp;
  page: ConsolePageId;
  environment: string;
}) => {
  const { search } = useLocation();
  const issues = useAppIssues(app.id);
  const types = useAppEventTypes(app.org_slug);
  const dead = useMemo(
    () => (types.data ? eventsView(types.data.types, app.slug, environment).totals.dead : 0),
    [types.data, app.slug, environment]
  );
  const badge: Partial<Record<ConsolePageId, number>> = {
    activity: issues.data ? liveIssueCount(issues.data.issues) : 0,
    events: dead
  };

  return (
    <nav
      aria-label={`${app.name} console`}
      data-testid='apps-console-nav'
      className='lg:sticky lg:top-0 lg:w-44 lg:shrink-0 lg:self-start max-lg:-mx-1 max-lg:overflow-x-auto'
    >
      <ul className='flex gap-0.5 lg:flex-col max-lg:w-max max-lg:px-1'>
        {CONSOLE_PAGES.map((p) => {
          const Icon = ICON[p.id];
          const active = p.id === page;
          const count = badge[p.id] ?? 0;
          return (
            <li key={p.id}>
              <Link
                to={consolePagePath(app, p.id, search)}
                aria-current={active ? "page" : undefined}
                data-testid={`apps-console-nav-${p.id}`}
                className={cn(
                  "flex h-8 items-center gap-2 rounded-md px-2 text-xs transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
                  active
                    ? "bg-muted font-medium text-foreground"
                    : "text-muted-foreground hover:bg-muted/50 hover:text-foreground"
                )}
              >
                <Icon className='size-3.5 shrink-0' />
                <span className='whitespace-nowrap'>{p.label}</span>
                {count > 0 ? (
                  <span
                    className={cn(
                      "ml-auto rounded-sm px-1 py-px font-medium text-[10px] tabular-nums max-lg:ml-1",
                      ADMIN_TONE.danger.bg,
                      ADMIN_TONE.danger.text
                    )}
                    data-testid={`apps-console-nav-${p.id}-count`}
                  >
                    {count}
                  </span>
                ) : null}
              </Link>
            </li>
          );
        })}
      </ul>
    </nav>
  );
};
