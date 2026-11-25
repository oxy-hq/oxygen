import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { isAxiosError } from "axios";

import { CustomAppsService } from "@/services/api/customApps";
import type { DataFlowQuery } from "@/types/dataFlow";
import queryKeys from "../queryKey";

/** Declarations change on a publish or a compile, not by the second. */
const DATA_FLOW_STALE_MS = 60_000;

/** A failed read carrying the server's own sentence and its status. */
export class DataFlowError extends Error {
  constructor(
    message: string,
    readonly status: number | undefined
  ) {
    super(message);
    this.name = "DataFlowError";
  }
}

/** The route answers `{ error, message }`; "no such organization" is worth showing,
 *  "Request failed with status code 404" is not. */
function toDataFlowError(error: unknown): DataFlowError {
  if (isAxiosError(error)) {
    const body = error.response?.data as { message?: unknown } | undefined;
    const message = typeof body?.message === "string" ? body.message : error.message;
    return new DataFlowError(message, error.response?.status);
  }
  return new DataFlowError(error instanceof Error ? error.message : String(error), undefined);
}

/**
 * One org's declared data flow for the admin Data flow page. Held until an org is
 * chosen; the previous graph stays on screen while a new focus or depth loads, so
 * walking the graph does not flash an empty canvas between clicks. A 4xx is the
 * server's answer (no such org or focus, out of range), so it is not retried.
 */
export function useDataFlow(query: DataFlowQuery | null) {
  return useQuery({
    queryKey: queryKeys.customApps.dataFlow(query ?? { org_id: "" }),
    queryFn: () =>
      CustomAppsService.dataFlow(query as DataFlowQuery).catch((error: unknown) => {
        throw toDataFlowError(error);
      }),
    enabled: !!query?.org_id,
    staleTime: DATA_FLOW_STALE_MS,
    placeholderData: keepPreviousData,
    retry: (failures, error) => {
      const status = error instanceof DataFlowError ? error.status : undefined;
      const answered = status !== undefined && status >= 400 && status < 500;
      return !answered && failures < 3;
    }
  });
}
