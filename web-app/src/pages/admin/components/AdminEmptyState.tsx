import type { ComponentType } from "react";
import { cn } from "@/libs/shadcn/utils";

/**
 * Bordered empty state with icon, headline, and a soft suggestion line.
 *
 * Two sizes. `page` (the default) is a whole page or a whole list with nothing in it:
 * centred, with room. `inline` is one section of a detail page with nothing in it, such
 * as an org with no workspaces: one line. A 180px dashed box for "No direct members"
 * pushed everything below it off the screen, and said less than a sentence does.
 */
export const AdminEmptyState = ({
  icon: Icon,
  title,
  description,
  action,
  size = "page",
  className,
  "data-testid": dataTestId
}: {
  icon: ComponentType<{ className?: string }>;
  title: string;
  description?: string;
  action?: React.ReactNode;
  size?: "page" | "inline";
  className?: string;
  /** Key it off the page's own id — `AdminAsync`'s generic `admin-async-empty` cannot
   *  tell one page's empty state from another's. */
  "data-testid"?: string;
}) =>
  size === "inline" ? (
    <div
      data-testid={dataTestId}
      className={cn(
        "flex flex-wrap items-center gap-x-2 gap-y-1 rounded-md border border-border/60 border-dashed px-3 py-2.5 text-xs",
        className
      )}
    >
      <Icon className='size-3.5 shrink-0 text-muted-foreground' />
      <span className='font-medium'>{title}</span>
      {description ? <span className='text-muted-foreground'>{description}</span> : null}
      {action ? <span className='ml-auto'>{action}</span> : null}
    </div>
  ) : (
    <PageEmptyState
      icon={Icon}
      title={title}
      description={description}
      action={action}
      className={className}
      data-testid={dataTestId}
    />
  );

const PageEmptyState = ({
  icon: Icon,
  title,
  description,
  action,
  className,
  "data-testid": dataTestId
}: {
  icon: ComponentType<{ className?: string }>;
  title: string;
  description?: string;
  action?: React.ReactNode;
  className?: string;
  "data-testid"?: string;
}) => (
  <div
    data-testid={dataTestId}
    className={cn(
      "flex flex-col items-center justify-center gap-3 rounded-lg border border-border/60 border-dashed bg-muted/20 px-6 py-12 text-center",
      className
    )}
  >
    <div className='rounded-full border border-border/60 bg-background p-2 text-muted-foreground'>
      <Icon className='size-5' />
    </div>
    <div className='space-y-1'>
      <p className='font-medium text-xs'>{title}</p>
      {description ? (
        <p className='mx-auto max-w-md text-muted-foreground text-xs'>{description}</p>
      ) : null}
    </div>
    {action ? <div className='pt-1'>{action}</div> : null}
  </div>
);
