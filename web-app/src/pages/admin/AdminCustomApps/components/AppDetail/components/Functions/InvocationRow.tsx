import { ChevronRight } from "lucide-react";
import { useId, useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { FunctionInvocation } from "@/types/apps";
import { InvocationCalls } from "../CallWaterfall/InvocationCalls";

/** Tooltip on a `success` the platform counted as a failure. */
const FAILED_SUCCESS_HINT =
  "Returned without throwing, but the platform counted it as a failure — it answered 5xx or caught a failed ctx call.";

/** The status word's tone. A `success` that answered 5xx or caught a failed
 *  ctx call is a failure to the pager; showing it green is how a week of
 *  refused writes looked fine. Anything else the platform did not count as a
 *  failure (a cancel, a shed) is muted, not red. */
function invocationTone(iv: Pick<FunctionInvocation, "status" | "failed">): string {
  if (iv.status === "success") return iv.failed ? ADMIN_TONE.danger.text : ADMIN_TONE.ok.text;
  if (iv.status === "running") return ADMIN_TONE.info.text;
  return iv.failed ? ADMIN_TONE.danger.text : ADMIN_TONE.muted.text;
}

/** One invocation in a function's history. The whole line toggles its call
 *  waterfall, which is fetched only once opened. */
export const InvocationRow = ({ appId, iv }: { appId: string; iv: FunctionInvocation }) => {
  const [open, setOpen] = useState(false);
  const callsId = useId();
  const failedSuccess = iv.status === "success" && iv.failed;
  return (
    <li
      className='border-border/40 border-b last:border-b-0'
      data-testid='admin-app-invocation-row'
    >
      <button
        type='button'
        aria-expanded={open}
        aria-controls={callsId}
        onClick={() => setOpen((was) => !was)}
        className='flex w-full items-center gap-2 py-1 text-left text-xs hover:bg-muted/40'
        data-testid='admin-app-invocation-toggle'
      >
        <ChevronRight
          className={cn(
            "size-3 shrink-0 text-muted-foreground transition-transform",
            open && "rotate-90"
          )}
          aria-hidden
        />
        {/* Wide enough for "failed · 500", so every row's columns line up. */}
        <span
          className={cn("w-20 shrink-0 font-medium", invocationTone(iv))}
          title={failedSuccess ? FAILED_SUCCESS_HINT : undefined}
        >
          {failedSuccess
            ? `failed${iv.result_status != null ? ` · ${iv.result_status}` : ""}`
            : iv.status}
        </span>
        <span className='w-16 shrink-0 text-muted-foreground'>{iv.mode}</span>
        <span className='w-14 shrink-0 text-muted-foreground'>
          {iv.duration_ms != null ? formatMs(iv.duration_ms) : "—"}
        </span>
        <span
          className='shrink-0 text-muted-foreground'
          title={new Date(iv.created_at).toLocaleString()}
        >
          {timeAgo(iv.created_at)}
        </span>
        {iv.error && (
          <span className={cn("truncate", ADMIN_TONE.danger.text)} title={iv.error}>
            {iv.error}
          </span>
        )}
      </button>
      {open && (
        <div id={callsId} className='flex flex-col gap-2 pb-2 pl-5'>
          {/* The error is the most important text in the row and the one the row
              has least room for. Truncated with only a `title`, its cause sat past
              the cut on every row of the 2026-10-21 outage (`… could not open a
              tr…`), and a tooltip is unreadable for a stack and uncopyable. */}
          {iv.error && (
            <pre
              className={cn(
                "max-h-60 overflow-auto whitespace-pre-wrap break-words rounded bg-muted/40 px-3 py-2 font-mono text-xs leading-relaxed",
                ADMIN_TONE.danger.text
              )}
              data-testid='admin-app-function-invocation-error-full'
            >
              {iv.error}
            </pre>
          )}
          <InvocationCalls appId={appId} invocationId={iv.id} />
        </div>
      )}
    </li>
  );
};

const formatMs = (ms: number): string => (ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(1)}s`);

const RTF = new Intl.RelativeTimeFormat("en", { numeric: "auto" });
const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["day", 86_400_000],
  ["hour", 3_600_000],
  ["minute", 60_000],
  ["second", 1_000]
];
function timeAgo(iso: string): string {
  const ms = new Date(iso).getTime();
  if (Number.isNaN(ms)) return "";
  const diff = ms - Date.now();
  for (const [unit, per] of UNITS) {
    if (Math.abs(diff) >= per || unit === "second") return RTF.format(Math.round(diff / per), unit);
  }
  return "";
}
