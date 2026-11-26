import { cn } from "@/libs/shadcn/utils";

/**
 * Operator-console section divider. Uppercase, narrow tracking, tiny —
 * pitched as a "table of contents" stripe over dense data. Stays out of
 * the way visually while letting users scan vertically through a long
 * detail surface. Doesn't bring its own spacing; callers wrap it in the
 * grid that owns the column layout.
 */
export const AdminSectionLabel = ({
  children,
  className,
  trailing
}: {
  children: React.ReactNode;
  className?: string;
  /** Optional trailing element (e.g. a count or a small action button). */
  trailing?: React.ReactNode;
}) => (
  <div
    className={cn(
      "flex items-baseline justify-between border-border-subtle border-b pb-2 font-semibold text-[10px] text-subtle-foreground uppercase tracking-eyebrow",
      className
    )}
  >
    <span>{children}</span>
    {trailing ? <span className='tracking-normal'>{trailing}</span> : null}
  </div>
);
