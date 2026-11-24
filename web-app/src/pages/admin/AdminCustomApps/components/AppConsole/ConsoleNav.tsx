import { Link, useLocation } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { CustomApp } from "@/types/apps";
import { CONSOLE_PAGES, type ConsolePageId, consolePagePath } from "./consolePages";
import { useConsoleBadges } from "./useConsoleBadges";

/**
 * The app's own sidebar: one entry per console page, the way Encore lists an app's
 * pages under its switcher. A count appears beside a page only when it is something
 * wrong (`useConsoleBadges`), so a quiet app's nav is just names.
 *
 * At phone width it is a row that scrolls sideways above the page, not a column beside
 * it.
 *
 * **Inside the column browser it renders only below `lg`** (the caller passes
 * `lg:hidden`), because the browser's Pages tier *is* this nav at that width. Same
 * list, same icons, same counts — one source, two shapes.
 */
export const ConsoleNav = ({
  app,
  page,
  environment,
  className
}: {
  app: CustomApp;
  page: ConsolePageId;
  environment: string;
  className?: string;
}) => {
  const { search } = useLocation();
  const badge = useConsoleBadges(app, environment);

  return (
    <nav
      aria-label={`${app.name} console`}
      data-testid='apps-console-nav'
      className={cn(
        "lg:sticky lg:top-0 lg:w-44 lg:shrink-0 lg:self-start max-lg:-mx-1 max-lg:overflow-x-auto",
        className
      )}
    >
      <ul className='flex gap-0.5 lg:flex-col max-lg:w-max max-lg:px-1'>
        {CONSOLE_PAGES.map((p) => {
          const Icon = p.icon;
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
