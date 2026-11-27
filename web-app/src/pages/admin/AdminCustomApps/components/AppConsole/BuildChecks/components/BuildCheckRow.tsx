import { ChevronRight } from "lucide-react";
import { useId, useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import { AdminStatusPill } from "@/pages/admin/components/AdminStatusPill";
import type { BuildCheck, BuildChecks } from "@/types/appBuildChecks";
import { checkStatus, openByDefault } from "../buildChecksModel";
import type { ChecksScope } from "../checksScope";
import { awaitsALook } from "../screensModel";
import { CheckEvidence } from "./CheckEvidence";
import { StatusIcon } from "./StatusIcon";

/**
 * One check on a build: icon, label, its one-line summary and its status, opening to
 * the evidence. A failure starts open, unless the attempt stalled and the row has no
 * evidence of its own; so does the changed screens' row while the app waits for a
 * person to see them. An advisory check says it only informs.
 */
export const BuildCheckRow = ({
  scope,
  check,
  stalled
}: {
  /** Which attempt the row belongs to, for evidence that reads more of it. */
  scope: ChecksScope;
  check: BuildCheck;
  /** Why the attempt got no answer, when it got none. */
  stalled?: BuildChecks["stalled"];
}) => {
  const [open, setOpen] = useState(
    () => openByDefault(check, stalled) || awaitsALook(check, scope.seenRequired)
  );
  const bodyId = useId();
  const status = checkStatus(check.status);
  return (
    <li
      className='border-border/40 border-b last:border-b-0'
      data-testid={`build-check-${check.kind}`}
      data-status={check.status}
    >
      <button
        type='button'
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => setOpen((was) => !was)}
        className='flex w-full items-center gap-2.5 py-2 text-left hover:bg-muted/40'
        data-testid={`build-check-${check.kind}-toggle`}
      >
        <ChevronRight
          className={cn(
            "size-3 shrink-0 text-muted-foreground transition-transform",
            open && "rotate-90"
          )}
          aria-hidden
        />
        <StatusIcon status={check.status} />
        <span className='flex min-w-0 flex-1 flex-col'>
          <span className='flex items-center gap-1.5 font-medium text-xs'>
            {check.label}
            {check.advisory && (
              <span
                className='rounded-sm bg-muted px-1 py-px font-normal text-[10px] text-muted-foreground'
                data-testid='build-check-informs'
              >
                informs
              </span>
            )}
          </span>
          {check.summary && (
            <span className='truncate text-muted-foreground text-xs' title={check.summary}>
              {check.summary}
            </span>
          )}
        </span>
        <AdminStatusPill tone={status.tone} label={status.label} className='shrink-0' />
      </button>
      {open && (
        <div id={bodyId} className='pb-3 pl-12'>
          <CheckEvidence scope={scope} check={check} />
        </div>
      )}
    </li>
  );
};
