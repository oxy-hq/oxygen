import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { AppDataService } from "@/services/api/appData";
import type { AppDataPage } from "@/types/appData";
import queryKeys from "../queryKey";

/**
 * Every read here is audited server-side and may wake a suspended database, so none
 * happens on its own: `enabled` is the operator's "Browse data", and nothing refetches
 * on focus or reconnect. A retry would write a second audit row for a refusal that
 * will not change (a lockdown, a missing role), so there is none.
 */
const deliberate = {
  refetchOnWindowFocus: false,
  refetchOnReconnect: false,
  retry: false,
  staleTime: 30_000
} as const;

/** The app's OLTP tables in `environment`, or why it has none. */
export function useAppDataTables(id: string, environment: string, enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.customApps.dataTables(id, environment),
    queryFn: () => AppDataService.listTables(id, environment),
    enabled,
    ...deliberate
  });
}

/** One page of one table; the previous page stays on screen while the next loads. */
export function useAppDataRows(id: string, environment: string, page: AppDataPage | null) {
  return useQuery({
    queryKey: queryKeys.customApps.dataRows(id, environment, page),
    queryFn: () => AppDataService.listRows(id, environment, page as AppDataPage),
    enabled: page !== null,
    placeholderData: keepPreviousData,
    ...deliberate
  });
}
