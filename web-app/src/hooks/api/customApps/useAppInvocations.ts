import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { isAxiosError } from "axios";

import { CustomAppsService } from "@/services/api/customApps";
import { PRODUCTION } from "@/types/appEnvironments";
import type { AppInvocationQuery } from "@/types/apps";
import { errMessage } from "../errMessage";
import { isForbidden } from "../httpStatus";
import queryKeys from "../queryKey";

/** A 403 (no non-production reach) or a 404 (no such environment) is an answer
 *  to show, not a fault to retry. */
const retryUnlessRefused = (failures: number, error: unknown) =>
  !isForbidden(error) && !isNotFound(error) && failures < 3;

const isNotFound = (error: unknown) => isAxiosError(error) && error.response?.status === 404;

/** The app-wide invocation listing (`GET …/invocations`), for one environment,
 *  window and filter set. `poll` refetches every few seconds while set — after a
 *  run is queued, so it shows up without a reload. */
export function useAppInvocations(
  appId: string | undefined,
  query: AppInvocationQuery,
  { enabled = true, poll = false }: { enabled?: boolean; poll?: boolean } = {}
) {
  return useQuery({
    queryKey: queryKeys.customApps.invocations(appId ?? "", query),
    queryFn: () => CustomAppsService.listInvocations(appId as string, query),
    enabled: !!appId && enabled,
    retry: retryUnlessRefused,
    refetchInterval: poll ? 3_000 : false
  });
}

/** Several listings at once — one per function, or per function and
 *  environment — keyed like `useAppInvocations`, so a read the explorer already
 *  made is shared. */
export function useAppInvocationsMany(
  appId: string | undefined,
  queries: AppInvocationQuery[],
  { poll = false }: { poll?: boolean } = {}
) {
  return useQueries({
    queries: queries.map((query) => ({
      queryKey: queryKeys.customApps.invocations(appId ?? "", query),
      queryFn: () => CustomAppsService.listInvocations(appId as string, query),
      enabled: !!appId,
      retry: retryUnlessRefused,
      refetchInterval: poll ? 3_000 : (false as const)
    }))
  });
}

/** A non-production environment's schedules switch. Production has none, so it
 *  is never asked for. */
export function useEnvironmentSchedules(appId: string | undefined, environment: string) {
  return useQuery({
    queryKey: queryKeys.customApps.environmentSchedules(appId ?? "", environment),
    queryFn: () => CustomAppsService.getEnvironmentSchedules(appId as string, environment),
    enabled: !!appId && environment !== PRODUCTION,
    retry: retryUnlessRefused,
    staleTime: 15_000
  });
}

/** Run a scheduled function once as its schedule would (`?as=schedule`), then
 *  refetch the app's invocation reads so the run shows up. */
export function useRunFunctionAsSchedule() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, name, environment }: { id: string; name: string; environment: string }) =>
      CustomAppsService.runFunctionAsSchedule(id, name, environment),
    onSuccess: (_data, { id }) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.customApps.appInvocations(id) });
    }
  });
}

/** What queueing one check answered: its run, or why it was refused. */
export type CheckQueued = { name: string; run_id: string } | { name: string; error: string };

/** Queue every named check in `environment` through the plain run endpoint, one
 *  after another, and report each — a refusal for one does not stop the rest. */
export function useRunChecks() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({
      id,
      names,
      environment
    }: {
      id: string;
      names: string[];
      environment: string;
    }): Promise<CheckQueued[]> => {
      const out: CheckQueued[] = [];
      for (const name of names) {
        try {
          const { run_id } = await CustomAppsService.runFunction(id, name, undefined, environment);
          out.push({ name, run_id });
        } catch (error) {
          out.push({ name, error: errMessage(error, "could not be queued") });
        }
      }
      return out;
    },
    onSuccess: (_data, { id }) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.customApps.appInvocations(id) });
    }
  });
}
