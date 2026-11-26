import { CheckCircle2, Play, Terminal, XCircle } from "lucide-react";
import type React from "react";
import { cn } from "@/libs/shadcn/utils";
import type { RunEventEntry } from "@/services/api/coordinator";

/**
 * Run detail body for a custom-app **Function Job** (`source_type =
 * "app_function"`): a scheduled or manually-triggered background run of a
 * single Oxy Function. Unlike DAG/ELT runs there's no sub-step graph — the
 * debugging unit is the run's persisted log. We surface the return body (or
 * error) and the `function_log` lines the runner drained onto the run's event
 * log (see the Oxy Function Jobs design, 2026-07-10).
 */
export const FunctionBody: React.FC<{
  events: RunEventEntry[];
  answer?: string;
  errorMessage?: string;
  /** Run-level status (`done`/`failed`/…). Fallback for the pill when a run
   *  failed before the isolate started (app/build/artifact resolution errors)
   *  and so never emitted an `app_function_completed` event. */
  runStatus?: string;
}> = ({ events, answer, errorMessage, runStatus }) => {
  const logs = events.filter((e) => e.event_type === "function_log");
  const completed = events.find((e) => e.event_type === "app_function_completed");
  const status = str(completed?.payload.status) ?? runStatus;
  const durationMs = num(completed?.payload.duration_ms);

  return (
    <div className='space-y-4 p-4'>
      {status && (
        <div className='flex flex-wrap items-center gap-3 text-xs'>
          <StatusPill status={status} />
          {durationMs != null && (
            <span className='text-muted-foreground'>ran in {formatDuration(durationMs)}</span>
          )}
          <span className='text-muted-foreground'>
            {logs.length} log line{logs.length === 1 ? "" : "s"}
          </span>
        </div>
      )}

      {errorMessage && (
        <section className='space-y-1'>
          <h3 className='font-medium text-destructive text-xs'>Error</h3>
          <pre className='overflow-x-auto whitespace-pre-wrap border border-destructive/30 bg-destructive/5 p-3 text-destructive text-xs'>
            {errorMessage}
          </pre>
        </section>
      )}

      {answer && (
        <section className='space-y-1'>
          <h3 className='font-medium text-foreground text-xs'>Return value</h3>
          <pre className='max-h-64 overflow-auto whitespace-pre-wrap border border-border bg-muted/30 p-3 text-foreground text-xs'>
            {answer}
          </pre>
        </section>
      )}

      <section className='space-y-1'>
        <h3 className='flex items-center gap-1.5 font-medium text-foreground text-xs'>
          <Terminal className='h-3.5 w-3.5' />
          Logs
        </h3>
        {logs.length === 0 ? (
          <p className='border border-border border-dashed p-3 text-muted-foreground text-xs'>
            This run produced no log output. Use <code>ctx.log(...)</code> or{" "}
            <code>console.log(...)</code> in the function to record progress.
          </p>
        ) : (
          <div className='overflow-x-auto border border-border bg-muted/20 font-mono text-xs'>
            {logs.map((e) => (
              <LogRow key={e.seq} level={str(e.payload.level)} message={str(e.payload.message)} />
            ))}
          </div>
        )}
      </section>
    </div>
  );
};

const LogRow: React.FC<{ level?: string; message?: string }> = ({ level, message }) => {
  const tone =
    level === "error"
      ? "text-destructive"
      : level === "warn" || level === "warning"
        ? "text-warning"
        : "text-foreground";
  return (
    <div className='flex gap-2 border-border/50 border-b px-3 py-1 last:border-b-0'>
      <span className='w-10 shrink-0 select-none text-muted-foreground uppercase'>
        {level ?? "log"}
      </span>
      <span className={cn("whitespace-pre-wrap break-all", tone)}>{message}</span>
    </div>
  );
};

const StatusPill: React.FC<{ status: string }> = ({ status }) => {
  // Accept both vocabularies: the invocation status (`success`) from the
  // completed event, and the run status (`done`) used as a pre-start fallback.
  const ok = status === "success" || status === "done";
  const running = status === "running";
  const Icon = ok ? CheckCircle2 : running ? Play : XCircle;
  const tone = ok
    ? "bg-success/10 text-success"
    : running
      ? "bg-primary/10 text-primary"
      : "bg-destructive/10 text-destructive";
  return (
    <span className={cn("inline-flex items-center gap-1 rounded-full px-2 py-0.5", tone)}>
      <Icon className='h-3 w-3' />
      {status}
    </span>
  );
};

const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const num = (v: unknown): number | undefined => (typeof v === "number" ? v : undefined);

const formatDuration = (ms: number): string =>
  ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)}s`;
