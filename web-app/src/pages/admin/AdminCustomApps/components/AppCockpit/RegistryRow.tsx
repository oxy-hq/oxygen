import { forwardRef } from "react";
import { AppMark } from "@/components/apps/AppMark";
import { cn } from "@/libs/shadcn/utils";
import type { CustomApp } from "@/types/apps";
import { AppHoverCard } from "../AppsTable/components/AppHoverCard";
import { AppStatusLabel } from "../AppsTable/components/AppStatus";
import { type AppStatus, ATTENTION, formatRelativeTime } from "../AppsTable/useAppsTable";

interface RegistryRowProps {
  app: CustomApp;
  status: AppStatus | null;
  selected: boolean;
  /** Show the org slug — hidden when the rail is grouped by org. */
  showOrg: boolean;
  onSelect: (app: CustomApp) => void;
  onPublish: (app: CustomApp) => void;
  onUnpublish: (app: CustomApp) => void;
}

/**
 * One line in the registry rail: the app's mark and name, then when it was last
 * active. The mark is what tells two same-named apps apart at a glance.
 *
 * A status shows only when the app needs someone — the same rule as the list.
 * In a 288px rail a label on every row would crowd the names out, and the rail's
 * job is walking the fleet: the rows that need attention stand out, the rest
 * stay quiet. The monospace source tag each row used to carry is gone.
 *
 * A real button (the hover card's actions float in a portal, so nothing nests),
 * with a left accent bar on the selected row. `forwardRef` so the rail can
 * scroll the active row into view during ↑/↓ navigation.
 */
export const RegistryRow = forwardRef<HTMLButtonElement, RegistryRowProps>(
  ({ app, status, selected, showOrg, onSelect, onPublish, onUnpublish }, ref) => (
    <AppHoverCard app={app} showOrg={showOrg} onPublish={onPublish} onUnpublish={onUnpublish}>
      <button
        ref={ref}
        type='button'
        data-state={selected ? "selected" : undefined}
        aria-current={selected}
        onClick={() => onSelect(app)}
        className={cn(
          "group relative flex w-full items-center gap-2 rounded-md py-1.5 pr-2 pl-3 text-left text-xs outline-none transition-colors",
          "hover:bg-muted/60 focus-visible:ring-2 focus-visible:ring-ring",
          "data-[state=selected]:bg-primary/10"
        )}
        data-testid={`admin-apps-rail-row-${app.org_slug}-${app.slug}`}
      >
        <span
          aria-hidden
          className={cn(
            "absolute top-1.5 bottom-1.5 left-0 w-0.5 rounded-full bg-primary transition-opacity",
            selected ? "opacity-100" : "opacity-0"
          )}
        />
        <AppMark iconUrl={app.icon_url} name={app.name} size='sm' />
        <span className='min-w-0 flex-1'>
          <span
            className={cn(
              "block truncate",
              selected ? "font-medium text-foreground" : "text-foreground/90"
            )}
          >
            {app.name}
          </span>
          {showOrg && <span className='block truncate text-muted-foreground'>{app.org_slug}</span>}
        </span>
        {status !== null && ATTENTION.has(status) && (
          <AppStatusLabel status={status} className='shrink-0' />
        )}
        <span className='w-8 shrink-0 text-right text-muted-foreground tabular-nums'>
          {formatRelativeTime(app.last_active_at ?? app.last_synced_at)}
        </span>
      </button>
    </AppHoverCard>
  )
);
RegistryRow.displayName = "RegistryRow";
