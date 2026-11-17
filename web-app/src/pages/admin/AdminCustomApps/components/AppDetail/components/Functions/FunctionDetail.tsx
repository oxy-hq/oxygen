import { Loader2, Play } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { useFunctionInvocations, useRunFunction } from "@/hooks/api/customApps/useAppFunctions";
import { PRODUCTION } from "@/types/appEnvironments";
import type { AppFunctionSummary } from "@/types/apps";
import { CopyButton } from "../../../UrlActions";
import { environmentLabel } from "../Secrets/environmentLabel";
import { functionUrl } from "./functionUrl";
import { type AppRef, InvocationRow } from "./InvocationRow";
import { RunPanel } from "./RunPanel";

/** How many invocations to show before "Show more" (the endpoint returns up to
 *  the 50 most recent; this paginates them client-side). */
const INVOCATIONS_PAGE = 10;

/** Expanded body of a function row: manifest config, a "Run now" trigger that
 *  surfaces the resulting job run, and recent invocation history. */
export const FunctionDetail = ({
  appId,
  fn,
  environment,
  appUrl,
  appRef
}: {
  appId: string;
  fn: AppFunctionSummary;
  /** Where Run now runs and whose invocations are listed. `undefined` is production. */
  environment?: string;
  /** The app's canonical URL; a route function is served under it as `fn/<name>`. */
  appUrl?: string;
  /** When given, failed invocations offer "Copy for an agent". */
  appRef?: AppRef;
}) => {
  const where = environment ?? PRODUCTION;
  const [runId, setRunId] = useState<string | null>(null);
  const [visible, setVisible] = useState(INVOCATIONS_PAGE);
  // Prefill the params box with the author-declared example (manifest
  // `inputExample`), so an operator sees the expected shape and can tweak it.
  const [inputText, setInputText] = useState(() =>
    fn.input_example != null ? JSON.stringify(fn.input_example, null, 2) : ""
  );
  const run = useRunFunction();
  const invocations = useFunctionInvocations(appId, fn.name, true, environment);

  const onRun = () => {
    // The input box is handed to the function as its `req` body (its params).
    // Empty → no params; otherwise it must parse as JSON.
    let input: unknown;
    const trimmed = inputText.trim();
    if (trimmed) {
      try {
        input = JSON.parse(trimmed);
      } catch {
        toast.error("Input must be valid JSON");
        return;
      }
    }
    run.mutate(
      { id: appId, name: fn.name, input, environment },
      {
        onSuccess: ({ run_id }) => setRunId(run_id),
        onError: (e) => toast.error(e instanceof Error ? e.message : "Couldn't start the run")
      }
    );
  };

  return (
    <div className='flex flex-col gap-3 border-border/60 border-t px-3 py-3'>
      <ConfigChips fn={fn} />
      {fn.route && appUrl && where === PRODUCTION && <RouteUrl appUrl={appUrl} name={fn.name} />}

      <div className='flex flex-col gap-1.5'>
        <span className='font-medium text-muted-foreground text-xs uppercase tracking-wide'>
          Input (JSON, optional)
        </span>
        <textarea
          value={inputText}
          onChange={(e) => setInputText(e.target.value)}
          rows={2}
          spellCheck={false}
          placeholder='e.g. { "store": 1 } — leave empty for no params'
          className='w-full resize-y rounded-md border border-border bg-muted/20 px-2 py-1.5 font-mono text-xs outline-none focus:border-ring'
        />
        {!inputText && (
          <span className='text-[10px] text-muted-foreground'>
            The box is empty — the function runs with no params. Declare <code>inputExample</code>{" "}
            on the function in <code>oxy-app.json</code> to prefill a real, editable value here.
          </span>
        )}
        <div className='flex flex-wrap items-center gap-2'>
          <Button size='sm' variant='outline' disabled={run.isPending} onClick={onRun}>
            {run.isPending ? (
              <Loader2 className='size-3.5 animate-spin' />
            ) : (
              <Play className='size-3.5' />
            )}
            Run now
          </Button>
          {/* Run now is a real run, not a dry one: it executes the build this
              environment serves, against this environment's data. It used to say
              nothing, and always meant production. */}
          <span
            className='text-muted-foreground text-xs'
            data-testid='admin-app-function-run-where'
          >
            Runs in {environmentLabel(where).toLowerCase()} — the build it serves, against its data.
          </span>
        </div>
      </div>

      {/* key={runId} remounts on a new run so RunPanel's queued-hint timer resets
          (otherwise a second Run now reuses the instance and the hint flashes). */}
      {runId && <RunPanel key={runId} appId={appId} runId={runId} />}

      <div className='flex flex-col gap-1'>
        <h4 className='font-medium text-muted-foreground text-xs uppercase tracking-wide'>
          Recent invocations
          {invocations.data && invocations.data.length > 0 ? ` (${invocations.data.length})` : ""}
        </h4>
        {invocations.isLoading ? (
          <p className='text-muted-foreground text-xs'>Loading…</p>
        ) : invocations.data && invocations.data.length > 0 ? (
          <>
            <ul className='flex flex-col'>
              {invocations.data.slice(0, visible).map((iv) => (
                <InvocationRow key={iv.id} appId={appId} iv={iv} fn={fn.name} appRef={appRef} />
              ))}
            </ul>
            {invocations.data.length > visible && (
              <button
                type='button'
                onClick={() => setVisible((v) => v + INVOCATIONS_PAGE)}
                className='self-start pt-1 text-muted-foreground text-xs underline underline-offset-2 hover:text-foreground'
              >
                Show more ({invocations.data.length - visible} older)
              </button>
            )}
          </>
        ) : (
          <p className='text-muted-foreground text-xs'>No invocations recorded yet.</p>
        )}
      </div>
    </div>
  );
};

/**
 * Where a route function is called, as the runtime serves it: the app's own URL
 * plus `fn/<name>`. Shown for production only, where that URL is certain; another
 * environment serves the same path from its own host.
 */
const RouteUrl = ({ appUrl, name }: { appUrl: string; name: string }) => {
  const url = functionUrl(appUrl, name);
  if (!url) return null;
  return (
    <div className='flex min-w-0 items-center gap-1.5 text-xs' data-testid='admin-app-function-url'>
      <span className='shrink-0 text-muted-foreground'>Called at</span>
      <code className='min-w-0 truncate font-mono' title={url}>
        {url}
      </code>
      <CopyButton value={url} label='function URL' />
    </div>
  );
};

/** Manifest config as compact key/value chips. */
const ConfigChips = ({ fn }: { fn: AppFunctionSummary }) => {
  const chips: [string, string][] = [];
  if (fn.schedule)
    chips.push(["schedule", `${fn.schedule}${fn.timezone ? ` · ${fn.timezone}` : ""}`]);
  if (fn.timeout_seconds != null) chips.push(["timeout", `${fn.timeout_seconds}s`]);
  if (fn.retries)
    chips.push([
      "retries",
      `${fn.retries.max_attempts} attempts${
        fn.retries.min_timeout_ms != null
          ? ` · ${fn.retries.min_timeout_ms}–${fn.retries.max_timeout_ms}ms`
          : ""
      }`
    ]);
  if (fn.destinations.length > 0) chips.push(["writes to", fn.destinations.join(", ")]);
  if (chips.length === 0) return null;
  return (
    <div className='flex flex-wrap gap-1.5'>
      {chips.map(([k, v]) => (
        <span
          key={k}
          className='inline-flex items-center gap-1 rounded border border-border bg-muted/40 px-1.5 py-0.5 text-xs'
        >
          <span className='text-muted-foreground'>{k}</span>
          <span className='font-mono'>{v}</span>
        </span>
      ))}
    </div>
  );
};
