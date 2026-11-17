import { Loader2, Play } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { useAppFunctions } from "@/hooks/api/customApps/useAppFunctions";
import {
  type CheckQueued,
  useAppInvocationsMany,
  useRunChecks
} from "@/hooks/api/customApps/useAppInvocations";
import { isForbidden } from "@/hooks/api/httpStatus";
import { cn } from "@/libs/shadcn/utils";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { AppInvocationQuery, CustomApp } from "@/types/apps";
import { environmentLabel } from "../../AppDetail/components/Secrets/environmentLabel";
import { ConsolePanel } from "../../ConsolePanel";
import { CheckRow } from "./CheckRow";
import { checkEnvironments, checkMatrix } from "./checks";

/** How long the cells refetch after "Run checks" queued runs: long enough for a
 *  worker to pick them up and a check to finish. */
const WATCH_MS = 90_000;

/**
 * The functions marked `"check": true` in the build the console's environment
 * serves, each with its latest run in production and staging (and the console's
 * sandbox, when it is on one), and "Run checks", which runs every one of them in
 * the console's environment through the plain Run now. Outside production a
 * manual run is admitted only for a check, so this is the one run there that
 * needs no flag.
 */
export const ChecksPanel = ({
  app,
  environment,
  scope
}: {
  app: CustomApp;
  environment: string;
  scope: { label: string; warn?: boolean };
}) => {
  const functions = useAppFunctions(app.id, environment);
  const checks = (functions.data ?? []).filter((f) => f.check).map((f) => f.name);
  const environments = checkEnvironments(environment);
  const run = useRunChecks();
  const [queued, setQueued] = useState<CheckQueued[] | null>(null);
  const [watching, setWatching] = useState(0);
  useEffect(() => {
    if (!watching) return;
    const stop = setTimeout(() => setWatching(0), WATCH_MS);
    return () => clearTimeout(stop);
  }, [watching]);

  const queries: AppInvocationQuery[] = checks.flatMap((name) =>
    environments.map((env) => ({ environment: env, function: name, limit: 1 }))
  );
  const reads = useAppInvocationsMany(app.id, queries, { poll: watching > 0 });
  const matrix = checkMatrix(checks, environments, (name, env) => {
    const r = reads[checks.indexOf(name) * environments.length + environments.indexOf(env)];
    return {
      rows: r?.data?.invocations,
      pending: !!r?.isPending,
      failed: !!r?.isError,
      forbidden: isForbidden(r?.error)
    };
  });
  const where = environmentLabel(environment).toLowerCase();

  const runChecks = () =>
    run.mutate(
      { id: app.id, names: checks, environment },
      {
        onSuccess: (result) => {
          setQueued(result);
          setWatching(Date.now());
        }
      }
    );

  return (
    <ConsolePanel
      id='checks'
      title='Checks'
      question='Q1 — do its checks pass'
      scope={scope}
      actions={
        <Button
          variant='outline'
          size='sm'
          className='h-6 gap-1 px-2 text-xs'
          disabled={checks.length === 0 || run.isPending}
          onClick={runChecks}
          title={`Run every check in ${where}: the build it serves, against its data.`}
          data-testid='admin-app-checks-run'
        >
          {run.isPending ? (
            <Loader2 className='size-3 animate-spin' />
          ) : (
            <Play className='size-3' />
          )}
          Run checks
        </Button>
      }
    >
      <AdminAsync
        query={functions}
        noun='this app&rsquo;s functions'
        rows={2}
        isEmpty={() => checks.length === 0}
        empty={
          <p className='text-muted-foreground text-xs' data-testid='admin-app-checks-empty'>
            No function is marked <code>"check": true</code> in the build {where} serves.
          </p>
        }
      >
        {() => (
          <div className='flex flex-col gap-2'>
            {queued && <Queued queued={queued} where={where} />}
            <div
              className='grid gap-x-3'
              style={{
                gridTemplateColumns: `minmax(6rem, 1fr) repeat(${environments.length}, minmax(0, 1.2fr))`
              }}
              data-testid='admin-app-checks'
            >
              <div className='col-span-full grid grid-cols-subgrid border-border/60 border-b pb-1 text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
                <span>Check</span>
                {environments.map((env) => (
                  <span key={env}>Latest · {environmentLabel(env)}</span>
                ))}
              </div>
              {matrix.map((row) => (
                <CheckRow key={row.name} app={app} row={row} />
              ))}
            </div>
          </div>
        )}
      </AdminAsync>
    </ConsolePanel>
  );
};

/** What "Run checks" queued, and what it was refused, each by name. */
const Queued = ({ queued, where }: { queued: CheckQueued[]; where: string }) => {
  const refused = queued.filter((q): q is { name: string; error: string } => "error" in q);
  const ran = queued.length - refused.length;
  return (
    <div className='flex flex-col gap-0.5 text-xs' data-testid='admin-app-checks-queued'>
      <span>
        Queued {ran} of {queued.length} check{queued.length === 1 ? "" : "s"} in {where}. The cells
        below refresh as they finish.
      </span>
      {refused.map((r) => (
        <span key={r.name} className={cn("break-words", ADMIN_TONE.danger.text)}>
          <span className='font-mono'>{r.name}</span>: {r.error}
        </span>
      ))}
    </div>
  );
};
