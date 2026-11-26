import { CheckCircle2, Clock, Loader2, Terminal, XCircle } from "lucide-react";
import { useEffect, useState } from "react";
import { useFunctionRun } from "@/hooks/api/customApps/useAppFunctions";
import { cn } from "@/libs/shadcn/utils";

/** After this long without the job leaving the queue, hint that a worker may not
 *  be draining it (function jobs run on the durable queue, not inline). */
const QUEUED_HINT_AFTER_MS = 12_000;

/**
 * A just-triggered function job run: polls its status + persisted `function_log`
 * output until terminal (via `useFunctionRun`), so an operator follows a manual
 * run through to completion without leaving the AppDetail surface. Logs are
 * drained after the isolate returns (not live-tailed), so they appear on the
 * poll once the run finishes. A job is `queued` until a worker claims it, then
 * `running`; if it stays queued we surface a hint that a worker may not be
 * running.
 */
export const RunPanel = ({ appId, runId }: { appId: string; runId: string }) => {
  const { data, isLoading } = useFunctionRun(appId, runId);
  const status = data?.status;

  // Flip a "slow" flag once the run has been open a while without progressing
  // past queued — the signal that nothing is draining the queue.
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    const t = setTimeout(() => setSlow(true), QUEUED_HINT_AFTER_MS);
    return () => clearTimeout(t);
  }, []);
  const stuckQueued = slow && status === "queued";

  return (
    <div className=' border border-border bg-muted/20'>
      <div className='flex items-center gap-2 border-border/60 border-b px-3 py-1.5'>
        <StatusIcon status={status} />
        <span className='font-medium text-xs'>{statusLabel(status, isLoading)}</span>
        <span
          className='ml-auto truncate font-mono text-[10px] text-muted-foreground'
          title={runId}
        >
          run {runId.slice(0, 8)}
        </span>
      </div>

      {stuckQueued && (
        <p className='border-border/60 border-b px-3 py-2 text-muted-foreground text-xs'>
          Still queued — a background worker hasn't picked this up. Function jobs run on the durable
          queue, so a worker must be draining it (the in-process global worker, or a{" "}
          <code>oxy worker</code> node).
        </p>
      )}

      {data?.error && (
        <pre className='overflow-x-auto whitespace-pre-wrap px-3 py-2 text-destructive text-xs'>
          {data.error}
        </pre>
      )}

      {data?.answer && (
        <pre className='max-h-40 overflow-auto whitespace-pre-wrap px-3 py-2 text-foreground text-xs'>
          {data.answer}
        </pre>
      )}

      <div className='px-3 py-2'>
        <div className='mb-1 flex items-center gap-1 text-[10px] text-muted-foreground uppercase tracking-wide'>
          <Terminal className='size-3' />
          Logs
        </div>
        {!data || data.logs.length === 0 ? (
          <p className='text-muted-foreground text-xs'>{logsPlaceholder(status, isLoading)}</p>
        ) : (
          <div className='overflow-x-auto font-mono text-xs'>
            {data.logs.map((line) => (
              <div key={line.seq} className='flex gap-2 py-0.5'>
                <span className='w-9 shrink-0 select-none text-muted-foreground uppercase'>
                  {line.level}
                </span>
                <span className={cn("whitespace-pre-wrap break-all", logTone(line.level))}>
                  {line.message}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
};

const StatusIcon = ({ status }: { status?: string | null }) => {
  if (status === "done") return <CheckCircle2 className='size-3.5 text-success' />;
  if (status === "failed" || status === "cancelled" || status === "timed_out")
    return <XCircle className='size-3.5 text-destructive' />;
  if (status === "queued") return <Clock className='size-3.5 text-muted-foreground' />;
  return <Loader2 className='size-3.5 animate-spin text-primary' />;
};

const statusLabel = (status: string | null | undefined, loading: boolean): string => {
  if (loading && !status) return "Starting…";
  switch (status) {
    case "queued":
      return "Queued…";
    case "done":
      return "Succeeded";
    case "failed":
      return "Failed";
    case "cancelled":
      return "Cancelled";
    case "timed_out":
      return "Timed out";
    default:
      return "Running…";
  }
};

const logsPlaceholder = (status: string | null | undefined, loading: boolean): string => {
  if (status === "queued") return "Queued — logs appear once a worker starts the run.";
  if (status === "running" || loading) return "Waiting for output…";
  return "No log output.";
};

const logTone = (level: string): string =>
  level === "error"
    ? "text-destructive"
    : level === "warn" || level === "warning"
      ? "text-warning"
      : "text-foreground";
