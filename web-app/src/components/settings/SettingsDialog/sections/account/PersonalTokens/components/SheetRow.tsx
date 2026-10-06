import type React from "react";
import { cn } from "@/libs/shadcn/utils";

interface Props {
  /** Left out for a row that continues the one above: the gutter is kept, empty. */
  label?: string;
  /** The one input this row labels. Without it the label names a group, by `labelId`. */
  htmlFor?: string;
  labelId?: string;
  className?: string;
  /** For a row whose control gives up height when the form is short of it. */
  contentClassName?: string;
  children: React.ReactNode;
}

const LABEL = "w-24 shrink-0 text-muted-foreground text-sm sm:leading-9";

/**
 * One line of a token form: its label in a gutter, its control on the edge every control shares.
 * Below `sm` the label sits above its control.
 *
 * A row keeps its height in a form too tall for the window, unless it says otherwise: the one
 * that holds a list is the one to give way.
 */
const SheetRow: React.FC<Props> = ({
  label,
  htmlFor,
  labelId,
  className,
  contentClassName,
  children
}) => (
  <div className={cn("flex min-w-0 shrink-0 flex-col gap-1 sm:flex-row sm:gap-0", className)}>
    {htmlFor ? (
      <label htmlFor={htmlFor} id={labelId} className={LABEL}>
        {label}
      </label>
    ) : (
      <span id={labelId} className={cn(LABEL, !label && "hidden sm:block")}>
        {label}
      </span>
    )}
    <div className={cn("min-w-0 flex-1", contentClassName)}>{children}</div>
  </div>
);

export default SheetRow;
