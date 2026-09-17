import { AppMark } from "@/components/apps/AppMark";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { cn } from "@/libs/shadcn/utils";
import type { AppHealthRow, CustomApp } from "@/types/apps";
import { type AppStatus, ATTENTION, formatRelativeTime } from "../useAppsTable";
import { AppActionsMenu } from "./AppActionsMenu";
import { AppStatusLabel } from "./AppStatus";
import { SourceWarning } from "./SourceWarning";

interface AppTableRowProps {
  app: CustomApp;
  status: AppStatus | null;
  /** This app's health row, when the fleet endpoint returned one. */
  health: AppHealthRow | undefined;
  showOrg: boolean;
  isSelected: boolean;
  /** Any row is selected — checkboxes stay visible while a selection exists. */
  selecting: boolean;
  onToggle: (shiftKey: boolean) => void;
  onOpen: (app: CustomApp) => void;
  onPublish: (app: CustomApp) => void;
  onUnpublish: (app: CustomApp) => void;
}

/**
 * One app, one line: mark and name · org · status · requests · last active.
 *
 * The app's mark stays: in a fleet where several apps share a name ("Oxy
 * Starter" in two orgs), the picture is what an operator's eye finds first.
 * It is the shared `AppMark`, so a row shows what the launcher shows.
 *
 * What left the resting row, and why:
 * - **The workspace-id prefix.** Not a triage fact; it is in the detail.
 * - **Copy-URL and open-in-new-tab buttons.** They duplicated two items already
 *   in the ⋯ menu, three icons repeated down every row.
 *
 * The checkbox and ⋯ menu appear on hover and keyboard focus rather than at rest,
 * and the checkboxes stay visible while any row is selected so a multi-select is
 * never done blind.
 *
 * The reason sentence renders only for the states that need someone. For a quiet
 * or operational app it would be the same line repeated down the list.
 */
export const AppTableRow = ({
  app,
  status,
  health,
  showOrg,
  isSelected,
  selecting,
  onToggle,
  onOpen,
  onPublish,
  onUnpublish
}: AppTableRowProps) => {
  const needsAttention = status !== null && ATTENTION.has(status);
  const revealed = isSelected || selecting;
  return (
    <TableRow
      data-state={isSelected ? "selected" : undefined}
      className='group cursor-pointer'
      onClick={() => onOpen(app)}
      data-testid={`admin-apps-row-${app.org_slug}-${app.slug}`}
    >
      <TableCell className='w-9 pr-0' onClick={(e) => e.stopPropagation()}>
        <Checkbox
          checked={isSelected}
          onClick={(e) => {
            e.stopPropagation();
            onToggle(e.shiftKey);
          }}
          aria-label={`Select ${app.name}`}
          className={cn(
            "transition-opacity focus-visible:opacity-100 group-hover:opacity-100",
            revealed ? "opacity-100" : "opacity-0"
          )}
        />
      </TableCell>

      <TableCell className='max-w-64'>
        <span className='flex items-center gap-2'>
          <AppMark iconUrl={app.icon_url} name={app.name} size='sm' />
          <span className='truncate font-medium text-foreground'>{app.name}</span>
          <SourceWarning unrecorded={app.source_unrecorded} />
        </span>
      </TableCell>

      {showOrg && (
        <TableCell className='max-w-36 truncate text-muted-foreground'>{app.org_slug}</TableCell>
      )}

      <TableCell className='max-w-96'>
        <span className='flex min-w-0 items-center gap-2'>
          <AppStatusLabel status={status} className='shrink-0' />
          {needsAttention && health?.reason && (
            <span className='truncate text-muted-foreground' title={health.reason}>
              {health.reason}
            </span>
          )}
        </span>
      </TableCell>

      <TableCell className='text-right text-muted-foreground tabular-nums'>
        {health && status !== "not_measured" ? health.requests.toLocaleString() : "—"}
      </TableCell>

      <TableCell className='text-right text-muted-foreground tabular-nums'>
        {formatRelativeTime(app.last_active_at ?? app.last_synced_at)}
      </TableCell>

      <TableCell className='w-10' onClick={(e) => e.stopPropagation()}>
        <AppActionsMenu
          app={app}
          onOpen={onOpen}
          onPublish={onPublish}
          onUnpublish={onUnpublish}
          // Only this row's own selection pins the menu open. A multi-select is
          // acted on from the bulk bar, and a column of ⋯ would be the clutter
          // this row was rebuilt to remove.
          triggerClassName={cn(
            "transition-opacity focus-visible:opacity-100 group-hover:opacity-100 data-[state=open]:opacity-100",
            isSelected ? "opacity-100" : "opacity-0"
          )}
        />
      </TableCell>
    </TableRow>
  );
};
