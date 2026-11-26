import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { CustomApp } from "@/types/apps";
import { CONSOLE_PAGES, type ConsolePageId, consolePagePath } from "../AppConsole/consolePages";
import { useConsoleBadges } from "../AppConsole/useConsoleBadges";
import { ColumnPane } from "./ColumnPane";
import type { SelectHow } from "./useColumnKeys";

/**
 * The third tier: the selected app's console pages.
 *
 * Its own component because it reads the badge counts, and a hook cannot be called
 * conditionally — with no app selected this tier does not exist at all, rather than
 * rendering as an empty column. Same rule the fleet table's `fleetColumns` follows: a
 * column nothing can answer is absent, not full of dashes.
 *
 * At `lg` and up this *is* `ConsoleNav`, which is why both read `CONSOLE_PAGES` and
 * `useConsoleBadges` instead of keeping their own icons and their own counts.
 */
export const PagesColumn = ({
  app,
  page,
  environment,
  search,
  onSelect,
  onLeaveColumn
}: {
  app: CustomApp;
  /** `null` when the URL named a page the console does not have. */
  page: ConsolePageId | null;
  environment: string;
  search: string;
  onSelect: (id: ConsolePageId, how: SelectHow) => void;
  onLeaveColumn: (delta: -1 | 1) => void;
}) => {
  const badge = useConsoleBadges(app, environment);
  return (
    <ColumnPane
      id='pages'
      label='Pages'
      depth={2}
      // Alone among the tiers this one does not grow at `2xl`: its rows are eight fixed
      // one-word labels, so the extra width would be padding, and it is the tier that
      // stays on screen when the other two are collapsed.
      width='w-44'
      selectedId={page}
      empty='This app has no console pages.'
      // Every row below is built from `CONSOLE_PAGES`, so the id coming back is one of
      // their ids. The cast is the narrowing that `ColumnPane`'s string ids cannot carry.
      onSelect={(id, how) => onSelect(id as ConsolePageId, how)}
      onLeaveColumn={onLeaveColumn}
      rows={CONSOLE_PAGES.map((p) => {
        const count = badge[p.id] ?? 0;
        return {
          id: p.id,
          label: p.label,
          href: consolePagePath(app, p.id, search),
          children: (
            <>
              <p.icon className='size-3.5 shrink-0' aria-hidden />
              <span className='min-w-0 flex-1 truncate'>{p.label}</span>
              {count > 0 && (
                <span
                  className={cn(
                    "shrink-0 px-1 py-px font-medium text-[10px] tabular-nums",
                    ADMIN_TONE.danger.bg,
                    ADMIN_TONE.danger.text
                  )}
                  data-testid={`admin-apps-browser-pages-${p.id}-count`}
                >
                  {count}
                </span>
              )}
            </>
          )
        };
      })}
    />
  );
};
