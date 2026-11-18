import type { ComponentType, ReactNode } from "react";
import { cn } from "@/libs/shadcn/utils";

/**
 * Page-level header for an Admin detail surface (org / user / workspace).
 * No eyebrow: the top bar's breadcrumb already says where this page sits,
 * and a second, uppercase copy of it above the title was one more line to
 * read past. The "identity row" (large avatar +
 * primary name + secondary metadata + actions) is the visual anchor that
 * tells the operator "you are inspecting THIS entity" — large enough to
 * never feel like a list-cell expansion.
 */
export const AdminDetailHeader = ({
  icon: Icon,
  title,
  subtitle,
  status,
  actions
}: {
  icon: ComponentType<{ className?: string }>;
  title: string;
  /** Secondary identifier (slug, email, parent org link). Renders in mono. */
  subtitle?: ReactNode;
  /** Status pill, badges, etc. — rendered to the right of the title row. */
  status?: ReactNode;
  /** Primary destructive / mutating actions. Rendered on the right. */
  actions?: ReactNode;
}) => (
  <header className='space-y-3'>
    <div className='flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between'>
      <div className='flex min-w-0 items-start gap-3'>
        <div
          className={cn(
            "flex size-10 shrink-0 items-center justify-center rounded-lg",
            "border border-border/60 bg-card text-foreground/70"
          )}
        >
          <Icon className='size-5' />
        </div>
        <div className='flex min-w-0 flex-col gap-1'>
          <div className='flex flex-wrap items-center gap-2'>
            <h1 className='truncate font-semibold text-xl tracking-tight'>{title}</h1>
            {status}
          </div>
          {subtitle ? (
            <div className='flex flex-wrap items-center gap-x-2.5 gap-y-0.5 text-muted-foreground text-xs'>
              {subtitle}
            </div>
          ) : null}
        </div>
      </div>
      {actions ? <div className='flex shrink-0 flex-wrap items-center gap-2'>{actions}</div> : null}
    </div>
  </header>
);
