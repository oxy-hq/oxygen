import { AppWindow, Building2, Plus } from "lucide-react";
import { Badge } from "@/components/ui/shadcn/badge";
import type { AppRef, UnifiedRow } from "../types";

/** How many app names a cell shows before "+N". */
const SHOWN = 3;

const AppBadge = ({ app, title }: { app: AppRef; title: string }) => (
  <Badge variant='outline' className='gap-1 font-normal' title={title}>
    <AppWindow className='size-3' />
    {app.name}
  </Badge>
);

/**
 * The apps that read a workspace secret, as the control that changes them.
 *
 * Assigning a secret to apps is the main thing anyone comes to this page to
 * do, so it is the cell itself rather than an item in the row's ⋯ menu: the
 * badges are the current answer and clicking them is how you change it.
 */
export const SharedWith = ({
  row,
  onEdit
}: {
  row: UnifiedRow;
  /** Open the app picker. Omitted on a read-only surface. */
  onEdit?: (row: UnifiedRow) => void;
}) => {
  const apps = row.sharedWith.slice(0, SHOWN);
  const more = row.sharedWith.length - apps.length;
  const label =
    row.sharedWith.length === 0
      ? `Choose which apps read ${row.name}`
      : `Change which apps read ${row.name}`;

  const body =
    row.sharedWith.length === 0 ? (
      <span className='flex items-center gap-1 text-muted-foreground text-xs'>
        {onEdit && <Plus className='size-3' />}
        No apps
      </span>
    ) : (
      <>
        {apps.map((app) => (
          <AppBadge key={app.id} app={app} title={`${app.name} reads ctx.env.${row.name}`} />
        ))}
        {more > 0 && (
          <span
            className='text-muted-foreground text-xs'
            title={row.sharedWith.map((a) => a.name).join(", ")}
          >
            +{more} more
          </span>
        )}
      </>
    );

  if (!onEdit) {
    return (
      <div className='flex flex-wrap items-center gap-1' data-testid='secret-available-to'>
        {body}
      </div>
    );
  }
  return (
    <button
      type='button'
      className='-mx-1.5 flex flex-wrap items-center gap-1 rounded-md px-1.5 py-1 text-left transition hover:bg-accent focus-visible:outline-2'
      aria-label={label}
      title={label}
      data-testid='secret-available-to'
      onClick={(e) => {
        e.stopPropagation();
        onEdit(row);
      }}
    >
      {body}
    </button>
  );
};

/**
 * Who can read a secret, for the places that only report it: the detail
 * dialog, and the Custom apps tab where the answer is always one app.
 */
export const AvailableTo = ({ row }: { row: UnifiedRow }) => {
  if (row.app) {
    return (
      <AppBadge app={row.app} title={`Only ${row.app.name} reads it, as ctx.env.${row.name}`} />
    );
  }
  return (
    <div className='flex flex-wrap items-center gap-1' data-testid='secret-available-to'>
      <Badge variant='secondary' className='gap-1 font-normal'>
        <Building2 className='size-3' />
        Organization
      </Badge>
      {row.sharedWith.map((app) => (
        <AppBadge key={app.id} app={app} title={`${app.name} reads it as ctx.env.${row.name}`} />
      ))}
    </div>
  );
};
