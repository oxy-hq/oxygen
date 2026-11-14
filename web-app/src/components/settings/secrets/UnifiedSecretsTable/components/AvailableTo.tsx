import { AppWindow, Building2 } from "lucide-react";
import { Badge } from "@/components/ui/shadcn/badge";
import type { UnifiedRow } from "../types";

/** How many app names a row shows before "+N". */
const SHOWN = 3;

/**
 * Who can read a secret, in one glance:
 *
 * - an app's own secret — that app alone;
 * - a workspace secret shared with apps — the workspace, and each app by name;
 * - a workspace secret shared with none — the workspace only (its databases,
 *   integrations and agents), no app.
 */
export const AvailableTo = ({ row, compact = true }: { row: UnifiedRow; compact?: boolean }) => {
  if (row.app) {
    return (
      <Badge
        variant='outline'
        className='w-fit gap-1 font-normal'
        title={`Only ${row.app.name} reads it, as ctx.env.${row.name}`}
      >
        <AppWindow className='size-3' />
        Only {row.app.name}
      </Badge>
    );
  }
  const apps = compact ? row.sharedWith.slice(0, SHOWN) : row.sharedWith;
  const more = row.sharedWith.length - apps.length;
  return (
    <div className='flex flex-wrap items-center gap-1' data-testid='secret-available-to'>
      <Badge variant='secondary' className='gap-1 font-normal'>
        <Building2 className='size-3' />
        Workspace
      </Badge>
      {apps.map((app) => (
        <Badge
          key={app.id}
          variant='outline'
          className='gap-1 font-normal'
          title={`${app.name} reads it as ctx.env.${row.name}`}
        >
          <AppWindow className='size-3' />
          {app.name}
        </Badge>
      ))}
      {more > 0 && (
        <span
          className='text-muted-foreground text-xs'
          title={row.sharedWith.map((a) => a.name).join(", ")}
        >
          +{more} more
        </span>
      )}
    </div>
  );
};
