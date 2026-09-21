import type { ReactNode } from "react";
import { Skeleton } from "@/components/ui/shadcn/skeleton";

/**
 * Shared detail-pane primitives — the density lever for every tenant pane.
 *
 * Type scale (deliberately narrow; an operator console earns nothing from
 * variety):
 *   17px semibold  page title
 *   11px uppercase eyebrow / section label
 *   13px           body + row primaries
 *   11px           row secondaries, meta, and IDs
 *
 * Numbers are `tabular-nums` so columns of counts line up on the digit, and IDs
 * and slugs are `font-mono` — both are "efficient fonts" in the only sense that
 * matters here: they make a value scannable without reading it.
 */

/** Detail-pane header: eyebrow + title + subtitle, with status and action slots. */
export function PaneHeader({
  eyebrow,
  title,
  subtitle,
  status,
  actions
}: {
  eyebrow: string;
  title: string;
  subtitle?: ReactNode;
  status?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className='flex flex-wrap items-start justify-between gap-2 border-b px-4 py-2.5'>
      <div className='min-w-0'>
        <div className='font-medium font-mono text-[10px] text-muted-foreground uppercase tracking-wider'>
          {eyebrow}
        </div>
        <div className='mt-0.5 flex items-center gap-1.5'>
          <h1 className='truncate font-semibold text-[17px] leading-tight'>{title}</h1>
          {status}
        </div>
        {subtitle && (
          <div className='mt-0.5 truncate font-mono text-[11px] text-muted-foreground'>
            {subtitle}
          </div>
        )}
      </div>
      {actions && <div className='flex flex-wrap items-center gap-1.5'>{actions}</div>}
    </div>
  );
}

/** A labeled section with an optional right-aligned action (e.g. an "Add" button). */
export function PaneSection({
  title,
  action,
  children
}: {
  title: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className='space-y-2'>
      <div className='flex items-center justify-between gap-2'>
        <h2 className='font-medium text-[10px] text-muted-foreground uppercase tracking-wider'>
          {title}
        </h2>
        {action}
      </div>
      {children}
    </section>
  );
}

/** A row in a list (member, org, workspace) with a trailing inline-controls slot. */
export function RowLine({
  primary,
  secondary,
  trailing,
  onClick
}: {
  primary: ReactNode;
  secondary?: ReactNode;
  trailing?: ReactNode;
  /** When set, the label area becomes a button that opens the related entity. */
  onClick?: () => void;
}) {
  const label = (
    <>
      <div className='truncate font-medium text-[13px] leading-tight'>{primary}</div>
      {secondary && (
        <div className='truncate font-mono text-[11px] text-muted-foreground leading-tight'>
          {secondary}
        </div>
      )}
    </>
  );
  return (
    <div className='flex items-center gap-2 rounded-md border border-border/60 px-2.5 py-1.5 transition-colors hover:bg-muted/40'>
      {onClick ? (
        <button
          type='button'
          onClick={onClick}
          className='min-w-0 flex-1 text-left hover:underline'
        >
          {label}
        </button>
      ) : (
        <div className='min-w-0 flex-1'>{label}</div>
      )}
      {trailing && <div className='flex shrink-0 items-center gap-1'>{trailing}</div>}
    </div>
  );
}

export function Stat({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className='rounded-md border border-border/60 px-2.5 py-1.5'>
      <div className='font-medium text-[10px] text-muted-foreground uppercase tracking-wider'>
        {label}
      </div>
      <div className='mt-0.5 font-semibold text-sm tabular-nums leading-tight'>{value}</div>
    </div>
  );
}

export function PaneLoading() {
  return (
    <div className='space-y-3 p-4'>
      <Skeleton className='h-12 w-full' />
      <Skeleton className='h-20 w-full' />
      <Skeleton className='h-32 w-full' />
    </div>
  );
}
