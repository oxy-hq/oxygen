import type { ReactNode } from "react";
import { cn } from "@/libs/shadcn/utils";

/**
 * A count that fades when it is zero.
 *
 * In a column of counts the eye should land on what exists. "**2** public · 0 auth ·
 * 0 private" reads at a glance; "2 · 0 · 0" in one weight makes you read every cell.
 * A zero is still printed, because zero is an answer. It is just not the one you are
 * looking for.
 *
 * `null` is not zero: it is "not measured", the third outcome this console keeps
 * apart from both. It renders as a muted dash, and the caller says why somewhere once.
 */
export const isQuiet = (value: number | null | undefined): boolean =>
  value === null || value === undefined || value === 0;

export const AdminCount = ({
  value,
  format = (n) => n.toLocaleString(),
  suffix,
  className,
  "data-testid": dataTestId
}: {
  value: number | null | undefined;
  format?: (n: number) => ReactNode;
  /** A unit or noun after the number, e.g. "public". Fades with it. */
  suffix?: ReactNode;
  className?: string;
  "data-testid"?: string;
}) => (
  <span
    data-testid={dataTestId}
    data-quiet={isQuiet(value) || undefined}
    className={cn(
      "tabular-nums",
      isQuiet(value) ? "text-muted-foreground/60" : "font-medium text-foreground",
      className
    )}
  >
    {value === null || value === undefined ? "—" : format(value)}
    {suffix ? <> {suffix}</> : null}
  </span>
);
