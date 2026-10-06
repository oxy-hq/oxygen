import type { ReactNode } from "react";

/** The title, one line of what it is for, and the pane's single primary action. */
export function PaneHeader({
  title,
  description,
  action
}: {
  title: string;
  description: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className='flex flex-col gap-2 sm:flex-row sm:items-end sm:justify-between sm:gap-4'>
      <div className='flex min-w-0 flex-col gap-1'>
        <h5 className='font-semibold text-sm'>{title}</h5>
        <p className='max-w-xl text-muted-foreground text-xs leading-relaxed'>{description}</p>
      </div>
      {action && <div className='shrink-0'>{action}</div>}
    </div>
  );
}
