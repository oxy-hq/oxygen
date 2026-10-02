import { RefreshCw } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { ControlsBar } from "@/components/AppPreview/Controls";
import { Displays } from "@/components/AppPreview/Displays";
import {
  registerSourceFile,
  renderJinja,
  runSqlInDuckDB
} from "@/components/AppPreview/Displays/utils";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useAppData, { useAppDataCached, useAppDisplays } from "@/hooks/api/apps/useApp";
import useRunAppMutation from "@/hooks/api/apps/useRunAppMutation";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { isIdeUnavailableError } from "@/libs/utils/ideHealth";
import type { DataContainer, TableData } from "@/types/app";
import AppDataState from "./AppDataState";

function LoadingStatus({ label }: { label: string }) {
  return <p className='h-5 text-muted-foreground text-sm'>{label}</p>;
}

// Module-level cache: survives component unmount/remount (i.e. navigation away and back).
// Keyed by appPath64; cleared on explicit refresh so stale data is never permanently stuck.
// Capped at MAX_CLIENT_DATA_CACHE_SIZE entries (FIFO) to prevent unbounded memory growth
// during long sessions with many apps.
const MAX_CLIENT_DATA_CACHE_SIZE = 20;
const clientDataCache = new Map<
  string,
  { data: DataContainer; controlValues: Record<string, unknown> }
>();

type Props = {
  appPath64: string;
  runButton?: boolean;
  autoRun?: boolean;
};

export default function AppPreview({ appPath64, runButton = true, autoRun = true }: Props) {
  const { project, branchName } = useCurrentProjectBranch();
  const { data: appDisplay } = useAppDisplays(appPath64);
  const controls = appDisplay?.controls ?? [];

  const taskMap = useMemo(() => appDisplay?.tasks ?? {}, [appDisplay?.tasks]);
  // All tasks are client-mode only when:
  //   1. Every task in taskMap declares mode: client, AND
  //   2. Every display that references data has its source present in taskMap.
  //
  // Condition 2 catches apps that mix inline-SQL tasks (in taskMap) with
  // sql_file tasks (filtered out of taskMap by the server). Without it,
  // allClientMode would be true and the server fetch would be disabled,
  // leaving sql_file-backed displays with no data.
  const allClientMode =
    Object.keys(taskMap).length > 0 &&
    Object.values(taskMap).every((t) => t.mode === "client") &&
    (appDisplay?.displays ?? []).every((d) => {
      const dataRef = (d as { data?: string }).data;
      if (!dataRef) return true; // layout-only block (markdown, row, etc.)
      return taskMap[dataRef] !== undefined;
    });

  const [controlValues, setControlValues] = useState<Record<string, unknown>>({});
  const serverDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const clientDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Incremented on each control change; client tasks check this to discard stale results.
  const clientGenRef = useRef(0);
  const [paramData, setParamData] = useState<DataContainer | undefined>();
  const [isClientRunning, setIsClientRunning] = useState(false);
  // Once DuckDB fails for any reason, all subsequent requests go to the server.
  const [forcedServerMode, setForcedServerMode] = useState(false);

  const {
    mutate: runApp,
    isPending: isServerRunning,
    isError
  } = useRunAppMutation((data) => {
    setParamData(data.data);
  });

  const isRunning = isServerRunning || isClientRunning;

  // Server-side data fetch — disabled for all-client apps.
  const appDataQueryResult = useAppData(appPath64, !allClientMode);
  // When the live fetch 502s because the ide is down, fall back to the last
  // cached data (compile boundary + S3 mirror) so the dashboard still renders.
  const ideDown = appDataQueryResult.isError && isIdeUnavailableError(appDataQueryResult.error);
  const cachedQuery = useAppDataCached(appPath64, ideDown);

  useEffect(() => {
    if (isError) toast.error("Error refreshing app. Check configuration and try again.");
  }, [isError]);

  // Run all client-mode tasks in DuckDB WASM with the given control values.
  // Source files (e.g. oxymart.csv) are downloaded once as Parquet from the server
  // and registered in DuckDB; SQL references are rewritten to use the .parquet name.
  // Falls back to the server silently if DuckDB execution fails.
  const runClientTasks = useCallback(
    async (values: Record<string, unknown>) => {
      // Capture the current generation; discard results if a newer call has started.
      clientGenRef.current += 1;
      const gen = clientGenRef.current;
      setIsClientRunning(true);
      try {
        // Collect unique source files across all client tasks.
        const allSourceFiles = [
          ...new Set(Object.values(taskMap).flatMap((t) => t.source_files ?? []))
        ];
        // Download each source file as Parquet and get the original→registered name mapping.
        const fileMappings = await Promise.all(
          allSourceFiles.map((f) => registerSourceFile(f, project.id, branchName))
        );
        const sourceFileMap = Object.fromEntries(
          fileMappings.map(({ original, registered }) => [original, registered])
        );

        const entries = await Promise.all(
          Object.entries(taskMap).map(async ([taskName, taskInfo]) => {
            // Render Jinja controls, then rewrite 'file.csv' → 'file.parquet' in SQL.
            let sql = renderJinja(taskInfo.sql, values);
            for (const [orig, registered] of Object.entries(sourceFileMap)) {
              sql = sql.replaceAll(`'${orig}'`, `'${registered}'`);
            }
            const json = await runSqlInDuckDB(sql);
            return [taskName, { file_path: `client_tasks/${taskName}.parquet`, json }] as [
              string,
              TableData
            ];
          })
        );
        // Only apply results from the most recent invocation.
        if (gen === clientGenRef.current) {
          const result = Object.fromEntries(entries) as DataContainer;
          if (clientDataCache.size >= MAX_CLIENT_DATA_CACHE_SIZE) {
            const oldest = clientDataCache.keys().next().value;
            if (oldest !== undefined) clientDataCache.delete(oldest);
          }
          clientDataCache.set(appPath64, {
            data: result,
            controlValues: values
          });
          setParamData(result);
        }
      } catch {
        if (gen === clientGenRef.current) {
          setForcedServerMode(true);
          runApp({
            pathb64: appPath64,
            params: values as Record<string, unknown>
          });
        }
      } finally {
        if (gen === clientGenRef.current) {
          setIsClientRunning(false);
        }
      }
    },
    [taskMap, appPath64, project.id, branchName, runApp]
  );

  // When the displays metadata loads, initialize control defaults and — for
  // all-client apps — immediately run the initial SQL in DuckDB WASM (unless autoRun is disabled).
  const appDisplayControls = appDisplay?.controls;
  useEffect(() => {
    if (!appDisplayControls) return;
    const defaults = Object.fromEntries(appDisplayControls.map((c) => [c.name, c.default ?? null]));
    setControlValues(defaults);

    if (allClientMode) {
      const cached = clientDataCache.get(appPath64);
      if (cached && JSON.stringify(cached.controlValues) === JSON.stringify(defaults)) {
        setParamData(cached.data);
      } else if (autoRun) {
        // runClientTasks cannot reject: its catch falls back to the server run.
        void runClientTasks(defaults);
      }
    }
  }, [appDisplayControls, allClientMode, runClientTasks, appPath64, autoRun]);

  const handleRun = () => {
    setParamData(undefined);
    clientDataCache.delete(appPath64);
    // Reset the DuckDB fallback flag on explicit refresh so the user can retry
    // client-mode execution after a transient failure.
    setForcedServerMode(false);
    // forcedServerMode was just reset above; read allClientMode directly so this
    // render's stale closure value doesn't send us to the server unnecessarily.
    if (allClientMode) {
      void runClientTasks(controlValues);
    } else {
      runApp({
        pathb64: appPath64,
        params: controlValues as Record<string, unknown>
      });
    }
  };

  useEffect(() => {
    return () => {
      if (clientDebounceRef.current) clearTimeout(clientDebounceRef.current);
      if (serverDebounceRef.current) clearTimeout(serverDebounceRef.current);
    };
  }, []);

  const handleControlChange = async (name: string, value: unknown) => {
    const next = { ...controlValues, [name]: value };
    setControlValues(next);

    if (allClientMode && !forcedServerMode) {
      if (clientDebounceRef.current) clearTimeout(clientDebounceRef.current);
      clientDebounceRef.current = setTimeout(() => {
        void runClientTasks(next);
      }, 300);
    } else {
      if (serverDebounceRef.current) clearTimeout(serverDebounceRef.current);
      serverDebounceRef.current = setTimeout(() => {
        runApp({ pathb64: appPath64, params: next as Record<string, unknown> });
      }, 300);
    }
  };

  const displayData = paramData ?? appDataQueryResult.data?.data ?? cachedQuery.data?.data;

  // True on first load (including newly-created apps) before any data has
  // arrived and before any error surfaces. Prevents displays from briefly
  // rendering "No data found" against undefined data while the initial
  // server fetch or client-mode DuckDB tasks are still in flight.
  const isInitialLoading =
    (displayData === undefined && !appDataQueryResult.isError && !appDataQueryResult.data?.error) ||
    // While falling back to cached data, hold the spinner so the displays don't
    // flash "No data found" before the cache arrives.
    (ideDown && cachedQuery.isFetching && displayData === undefined);

  return (
    <div className='relative h-full w-full overflow-hidden px-2' data-testid='app-preview'>
      {runButton && controls.length === 0 && (
        <Button
          className='absolute right-6 bottom-6 z-1'
          onClick={handleRun}
          disabled={isRunning || appDataQueryResult.isPending}
          variant='default'
          content='icon'
          size='sm'
        >
          {isRunning ? <Spinner /> : <RefreshCw />}
        </Button>
      )}

      <div className='h-full w-full overflow-auto'>
        {controls.length > 0 && (
          <div className='sticky top-0 z-10 border-border border-b bg-background/95 backdrop-blur-sm'>
            <div className='mx-auto w-full max-w-200 px-2'>
              <ControlsBar
                controls={controls}
                values={controlValues}
                data={displayData}
                onChange={handleControlChange}
                onRun={runButton ? handleRun : undefined}
                isRunning={isRunning || appDataQueryResult.isPending}
              />
            </div>
          </div>
        )}
        <div className='mx-auto w-full max-w-200 p-2'>
          <AppDataState
            appDataQueryResult={appDataQueryResult}
            cachedAvailable={!!cachedQuery.data?.data}
            cachedPending={cachedQuery.isFetching}
          />
          {isInitialLoading ? (
            <div
              className='flex min-h-100 flex-col items-center justify-center gap-3'
              data-testid='app-preview-loading'
            >
              <Spinner className='size-8' />
              <LoadingStatus label='Loading app…' />
            </div>
          ) : (
            <div
              className={`relative transition-opacity duration-150 ${isRunning ? "pointer-events-none opacity-40" : "opacity-100"}`}
            >
              {isRunning && (
                <div className='absolute inset-0 z-10 flex flex-col items-center justify-center gap-3'>
                  <Spinner className='size-8' />
                  <LoadingStatus label='Loading app…' />
                </div>
              )}
              <Displays displays={appDisplay?.displays || []} data={displayData} />
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
