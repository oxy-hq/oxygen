import { AppMark } from "@/components/apps/AppMark";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { cn } from "@/libs/shadcn/utils";
import type { CustomApp } from "@/types/apps";
import { type AppStatus, formatRelativeTime } from "../useAppsTable";
import { AppActionsMenu } from "./AppActionsMenu";
import { AppHoverCard } from "./AppHoverCard";
import { AppStatusLabel } from "./AppStatus";
import { SourceWarning } from "./SourceWarning";

interface AppCardProps {
  app: CustomApp;
  status: AppStatus | null;
  showOrg: boolean;
  isSelected: boolean;
  onToggle: (shiftKey: boolean) => void;
  onOpen: (app: CustomApp) => void;
  onPublish: (app: CustomApp) => void;
  onUnpublish: (app: CustomApp) => void;
}

/**
 * A card in the Cards layout: the app's mark and name, then status and when it
 * was last active. Two lines.
 *
 * The icon stays here — it is what the Cards layout is for — but the rest of what
 * the card used to carry went: a URL line truncated to `127.0.0.1:5173/custom…`
 * (a full row, a link glyph and a copy button, conveying nothing legible), a
 * monospace source badge, and a separate live/draft dot that the status label now
 * covers. URLs are one hover away in the hover card and in the ⋯ menu.
 *
 * The mark doubles as the selection target: hover or selection swaps it for a
 * checkbox.
 */
export const AppCard = ({
  app,
  status,
  showOrg,
  isSelected,
  onToggle,
  onOpen,
  onPublish,
  onUnpublish
}: AppCardProps) => (
  <AppHoverCard app={app} showOrg={showOrg} onPublish={onPublish} onUnpublish={onUnpublish}>
    {/* biome-ignore lint/a11y/useSemanticElements: the card nests interactive
        controls (checkbox, menu), so a real <button> would be invalid
        button-in-button; a div with role/tabIndex reproduces the semantics. */}
    <div
      role='button'
      tabIndex={0}
      data-state={isSelected ? "selected" : undefined}
      onClick={() => onOpen(app)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen(app);
        }
      }}
      className={cn(
        "group flex cursor-pointer flex-col gap-3 rounded-lg border bg-card p-3 text-left outline-none transition-colors",
        "hover:border-foreground/20 focus-visible:ring-2 focus-visible:ring-ring",
        "data-[state=selected]:border-primary data-[state=selected]:bg-primary/5"
      )}
      data-testid={`admin-apps-card-${app.org_slug}-${app.slug}`}
    >
      <div className='flex items-center gap-2.5'>
        <div className='relative flex size-5 shrink-0 items-center justify-center'>
          <AppMark
            iconUrl={app.icon_url}
            name={app.name}
            size='sm'
            className={cn("transition-opacity group-hover:opacity-0", isSelected && "opacity-0")}
          />
          <Checkbox
            checked={isSelected}
            onClick={(e) => {
              e.stopPropagation();
              onToggle(e.shiftKey);
            }}
            aria-label={`Select ${app.name}`}
            className={cn(
              "absolute opacity-0 transition-opacity focus-visible:opacity-100 group-hover:opacity-100",
              isSelected && "opacity-100"
            )}
          />
        </div>
        <div className='min-w-0 flex-1'>
          <span className='flex items-center gap-1.5'>
            <span className='truncate font-medium text-foreground text-xs'>{app.name}</span>
            <SourceWarning unrecorded={app.source_unrecorded} />
          </span>
          {showOrg && (
            <span className='block truncate text-muted-foreground text-xs'>{app.org_slug}</span>
          )}
        </div>
        <AppActionsMenu
          app={app}
          onOpen={onOpen}
          onPublish={onPublish}
          onUnpublish={onUnpublish}
          triggerClassName={cn(
            "-mr-1 transition-opacity focus-visible:opacity-100 group-hover:opacity-100 data-[state=open]:opacity-100",
            isSelected ? "opacity-100" : "opacity-0"
          )}
        />
      </div>

      <div className='flex items-center justify-between gap-2 text-xs'>
        <AppStatusLabel status={status} />
        <span className='text-muted-foreground tabular-nums'>
          {formatRelativeTime(app.last_active_at ?? app.last_synced_at)}
        </span>
      </div>
    </div>
  </AppHoverCard>
);
