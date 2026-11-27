import { StatusIcon } from "../AppConsole/BuildChecks/components/StatusIcon";
import type { PlanChecks } from "./planChecks";

/**
 * The promote confirm's Checks line: each check the app requires as a glyph and its
 * label, in the checks list's order, then how many screens changed. The glyph is the
 * status; the words under it are the check's own summary, on hover.
 */
export const PlanChecksLine = ({ checks }: { checks: PlanChecks }) => {
  const changed = checks.changes?.changed ?? 0;
  return (
    <div
      className='flex flex-wrap items-center gap-x-3 gap-y-1 text-xs'
      data-testid='app-lifecycle-plan-checks'
    >
      <span className='text-muted-foreground'>Checks</span>
      {checks.required.length === 0 && <span className='text-muted-foreground'>none required</span>}
      {checks.required.map((row) => (
        <span
          key={row.kind}
          className='inline-flex items-center gap-1'
          title={row.summary ?? undefined}
          data-testid={`app-lifecycle-plan-check-${row.kind}`}
          data-status={row.status}
        >
          <StatusIcon status={row.status} />
          {row.label}
          <span className='sr-only'>: {row.status}</span>
        </span>
      ))}
      {changed > 0 && (
        <span className='text-muted-foreground' data-testid='app-lifecycle-plan-changes'>
          {changed} screen{changed === 1 ? "" : "s"} changed
          {checks.changes?.seen ? ", seen" : ""}
        </span>
      )}
    </div>
  );
};
