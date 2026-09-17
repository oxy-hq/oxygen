import { cn } from "@/libs/shadcn/utils";
import { AdminStatusPill, type AdminStatusTone } from "@/pages/admin/components/AdminStatusPill";
import { type AppStatus, STATUS_LABEL } from "../useAppsTable";

/**
 * The tone of each status that needs someone. Existing tones only:
 * `AdminStatusPill` asks that new resource statuses map onto its set rather than
 * add to it, and an earlier fleet view that invented its own glyphs was one more
 * visual language on a page that already had too many.
 *
 * `not_measured` shares `warn` with `degraded` on purpose. Both need a look, and
 * the label says which.
 */
export const ATTENTION_TONE: Partial<Record<AppStatus, AdminStatusTone>> = {
  down: "danger",
  degraded: "warn",
  not_measured: "warn"
};

/**
 * How one app's status reads in a row.
 *
 * **A pill is a call for attention**, so only the three states that need someone
 * get one. Quiet, operational and draft are plain muted text: in a fleet of a
 * hundred mostly-fine apps, a hundred pills would say "look here" a hundred times
 * and mean nothing. Colour on this page means something is wrong.
 *
 * `null` is "not known yet" — health still loading, or the app past the fleet
 * endpoint's page cap — and shows a dash rather than borrowing a verdict.
 */
export const AppStatusLabel = ({
  status,
  className
}: {
  status: AppStatus | null;
  className?: string;
}) => {
  if (status === null) {
    return (
      <span
        className={cn("text-muted-foreground/50", className)}
        title='Health not known yet'
        data-testid='admin-apps-status-unknown'
      >
        —
      </span>
    );
  }
  const tone = ATTENTION_TONE[status];
  if (tone) {
    return (
      <AdminStatusPill
        tone={tone}
        label={STATUS_LABEL[status]}
        className={className}
        data-testid={`admin-apps-status-${status}`}
      />
    );
  }
  return (
    <span
      className={cn("text-muted-foreground", className)}
      data-testid={`admin-apps-status-${status}`}
    >
      {STATUS_LABEL[status]}
    </span>
  );
};
