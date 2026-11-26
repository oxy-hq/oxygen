import { ChevronRight } from "lucide-react";
import type { ComponentType, ReactNode } from "react";
import { Link } from "react-router-dom";
import { card, tile } from "@/components/ui/reference/tokens";
import { cn } from "@/libs/shadcn/utils";

/**
 * "This entity links to that entity" row, used inside every Admin detail
 * tab where the operator should be able to traverse to another resource.
 * Example: a member row on the Org page links to the user's detail page;
 * a workspace row on the Org page links to the workspace's detail page.
 *
 * Visually compact — single line on a wide grid (icon · primary · secondary
 * · meta · chevron). Hover gives a small surface lift so the row reads as a
 * navigation target, not just data. Keyboard-focusable via the Link.
 */
export const AdminLinkedRow = ({
  to,
  icon: Icon,
  primary,
  secondary,
  meta,
  trailing,
  className
}: {
  /** Destination route. When omitted, the row renders as a static row. */
  to?: string;
  icon?: ComponentType<{ className?: string }>;
  /** Bold-ish, left-aligned identity (name). */
  primary: ReactNode;
  /** Subdued, second line (email, slug, "Created Jan 12 2026"). Mono. */
  secondary?: ReactNode;
  /** Right-aligned column before the chevron. Free-form (badges, dates). */
  meta?: ReactNode;
  /** Slot at the far right, before the chevron (e.g. role selector). */
  trailing?: ReactNode;
  className?: string;
}) => {
  const body = (
    <div
      className={cn(
        "group flex items-center gap-3 px-4 py-3 transition-colors",
        to && "hover:bg-muted/40",
        className
      )}
    >
      {Icon ? (
        <div
          className={cn(
            tile,
            "flex size-8 shrink-0 items-center justify-center text-muted-foreground"
          )}
        >
          <Icon className='size-3.5' />
        </div>
      ) : null}
      <div className='flex min-w-0 flex-1 flex-col gap-0.5'>
        <div className='truncate font-medium text-xs leading-tight'>{primary}</div>
        {secondary ? (
          <div className='truncate font-mono text-[11px] text-muted-foreground'>{secondary}</div>
        ) : null}
      </div>
      {meta ? (
        <div className='hidden shrink-0 items-center gap-3 text-muted-foreground text-xs tabular-nums sm:flex'>
          {meta}
        </div>
      ) : null}
      {trailing ? <div className='shrink-0'>{trailing}</div> : null}
      {to ? (
        <ChevronRight className='size-3.5 shrink-0 text-muted-foreground/60 transition-transform group-hover:translate-x-0.5 group-hover:text-foreground/70' />
      ) : null}
    </div>
  );

  if (to) {
    return (
      <Link to={to} className='block focus:outline-none focus-visible:bg-muted/40'>
        {body}
      </Link>
    );
  }
  return body;
};

/**
 * Container for a vertical stack of `AdminLinkedRow`s. Renders the
 * bordered card with internal dividers — matches the rest of the
 * console's bordered-card density.
 */
export const AdminLinkedList = ({
  children,
  className
}: {
  children: ReactNode;
  className?: string;
}) => (
  <div className={cn(card, "divide-y divide-border-subtle overflow-hidden", className)}>
    {children}
  </div>
);
