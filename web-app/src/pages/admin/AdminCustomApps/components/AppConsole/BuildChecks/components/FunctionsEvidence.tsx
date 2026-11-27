import { ChevronRight } from "lucide-react";
import { useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { BuildCheckFunctionRun } from "@/types/appBuildChecks";
import { InvocationCalls } from "../../../AppDetail/components/CallWaterfall/InvocationCalls";
import { durationText } from "../buildChecksModel";

/**
 * One line per `"check": true` function the build ran: passed or what it ended as,
 * how long it took, and its error. A line with an invocation opens that invocation's
 * call trace under it, as the Functions page's rows do.
 */
export const FunctionsEvidence = ({
  appId,
  functions
}: {
  appId: string;
  functions: readonly BuildCheckFunctionRun[];
}) => (
  <ul className='flex flex-col' data-testid='build-check-functions'>
    {functions.map((fn) => (
      <FunctionLine key={fn.run_id || fn.name} appId={appId} fn={fn} />
    ))}
  </ul>
);

/** One function's word: `passed`, `failed` (it ran and said no), or how its run ended. */
function verdictOf(fn: BuildCheckFunctionRun): string {
  if (fn.passed) return "passed";
  if (fn.status === "done") return "failed";
  // Staging served another build by the time the worker ran it: no verdict on this one.
  if (fn.status === "not_this_build") return "ran another build";
  return fn.status.replaceAll("_", " ");
}

const FunctionLine = ({ appId, fn }: { appId: string; fn: BuildCheckFunctionRun }) => {
  const [open, setOpen] = useState(false);
  const verdict = verdictOf(fn);
  const line = (
    <>
      <span className='min-w-0 truncate font-mono' title={fn.name}>
        {fn.name}
      </span>
      <span
        className={cn(
          "shrink-0 font-medium",
          fn.passed ? ADMIN_TONE.ok.text : ADMIN_TONE.danger.text
        )}
      >
        {verdict}
      </span>
      <span className='shrink-0 text-muted-foreground'>
        {fn.duration_ms != null ? durationText(fn.duration_ms) : "—"}
      </span>
      {fn.error && (
        <span className={cn("min-w-0 truncate", ADMIN_TONE.danger.text)} title={fn.error}>
          {fn.error}
        </span>
      )}
    </>
  );
  return (
    <li
      className='border-border/40 border-b last:border-b-0'
      data-testid={`build-check-function-${fn.name}`}
      data-passed={fn.passed}
    >
      {fn.invocation_id ? (
        <button
          type='button'
          aria-expanded={open}
          onClick={() => setOpen((was) => !was)}
          className='flex w-full items-baseline gap-2 py-1 text-left text-xs hover:bg-muted/40'
        >
          <ChevronRight
            className={cn(
              "size-3 shrink-0 self-center text-muted-foreground transition-transform",
              open && "rotate-90"
            )}
            aria-hidden
          />
          {line}
        </button>
      ) : (
        <div className='flex items-baseline gap-2 py-1 pl-5 text-xs'>{line}</div>
      )}
      {open && fn.invocation_id && (
        <div className='pb-2 pl-5'>
          <InvocationCalls appId={appId} invocationId={fn.invocation_id} />
        </div>
      )}
    </li>
  );
};
