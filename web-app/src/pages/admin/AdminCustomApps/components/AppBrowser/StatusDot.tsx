import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { type AppStatus, STATUS_LABEL, STATUS_TONE } from "../../appStatus";

/**
 * One app's state, in the smallest thing that can carry it: a column row has no space
 * for the pill `LifecycleVerdictPill` draws, and the status still has to be on the row —
 * otherwise sorting the columns for navigation rather than worst-first would hide what
 * needs a person.
 *
 * **Unknown is hollow, not grey.** A filled muted dot reads as a verdict ("quiet",
 * "draft"); an outline reads as the absence of one, which is what `null` means — health
 * in flight, or nothing measuring. The same rule `statusOf` keeps: never borrow a
 * verdict for an app nobody has looked at.
 */
export const StatusDot = ({
  status,
  className
}: {
  status: AppStatus | null;
  className?: string;
}) =>
  status === null ? (
    <span
      className={cn("size-1.5 shrink-0 rounded-full border border-muted-foreground/60", className)}
      title='Not known yet'
      data-testid='admin-apps-browser-dot-unknown'
    />
  ) : (
    <span
      className={cn(
        "size-1.5 shrink-0 rounded-full",
        ADMIN_TONE[STATUS_TONE[status]].dot,
        className
      )}
      title={STATUS_LABEL[status]}
      data-testid={`admin-apps-browser-dot-${status}`}
    />
  );
