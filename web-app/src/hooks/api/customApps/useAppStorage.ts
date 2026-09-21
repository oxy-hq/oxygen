import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { isAxiosError } from "axios";
import { toast } from "sonner";
import { CustomAppStorageService } from "@/services/api/customAppStorage";
import queryKeys from "../queryKey";

/**
 * Fleet-wide storage usage, ranked. Reads the sweeper's rollup, so it is up to
 * one sweep interval stale — the UI shows `measuredAt` rather than implying
 * these are live numbers.
 */
export const useFleetStorage = (
  sort: "bytes" | "growth" | "untagged" = "bytes",
  options: { enabled?: boolean } = {}
) =>
  useQuery({
    queryKey: queryKeys.customApps.storageFleet(sort),
    queryFn: () => CustomAppStorageService.fleet(sort),
    // Gated because the fleet list wants this and a single app's console does not —
    // an ungated fleet rollup fires a full sweeper read on every app page view.
    enabled: options.enabled ?? true
  });

/**
 * One app's objects, straight from S3. Paginated by the store's own cursor
 * rather than an offset — a silo with 100k objects must never turn one call
 * into an unbounded walk.
 */
export const useAppStorageObjects = (appId: string | null, prefix: string) =>
  useInfiniteQuery({
    queryKey: queryKeys.customApps.storageObjects(appId ?? "", prefix),
    enabled: !!appId,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      CustomAppStorageService.browse(appId as string, {
        prefix: prefix || undefined,
        cursor: pageParam,
        limit: 100
      }),
    getNextPageParam: (last) => (last.hasMore ? (last.cursor ?? undefined) : undefined)
  });

/**
 * Daily usage history. Fleet-wide unless `appId` is given.
 *
 * Kept separate from the fleet query so changing the chart's time range doesn't
 * refetch the table (or vice-versa) — they move for different reasons.
 */
export const useStorageHistory = (days: number, appId?: string) =>
  useQuery({
    queryKey: queryKeys.customApps.storageHistory(days, appId),
    queryFn: () => CustomAppStorageService.history(days, appId),
    // The sweeper runs every 15 minutes; refetching faster just burns queries.
    staleTime: 5 * 60 * 1000
  });

export const useDeleteStorageObjects = (appId: string | null) => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (keys: string[]) => CustomAppStorageService.deleteObjects(appId as string, keys),
    onSuccess: ({ deleted }) => {
      // Both surfaces move: the listing loses rows, and the rollup's totals are
      // now wrong until the next sweep.
      qc.invalidateQueries({ queryKey: ["customApps", "storage"] });
      toast.success(`Deleted ${deleted} object${deleted === 1 ? "" : "s"}`);
    },
    onError: () => toast.error("Could not delete the selected objects")
  });
};

export const useSweepStorage = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: CustomAppStorageService.sweep,
    onSuccess: () => {
      // The sweep only STARTED — claiming "measured N apps" here would be a
      // number we never received. Refetch shortly so the rows update on their
      // own; each carries its own `measuredAt` so progress is visible.
      toast.success("Measuring in the background…");
      setTimeout(
        () => qc.invalidateQueries({ queryKey: ["customApps", "storage"] }),
        SWEEP_REFETCH_DELAY_MS
      );
    },
    onError: (err) => {
      if (!isAxiosError(err) || err.response?.status !== 409) {
        toast.error("Could not start the sweep");
        return;
      }
      // Two different 409s, and they mean opposite things to whoever clicked.
      // A sweep bounded to another operator's orgs will never touch this
      // caller's rows, so the old flat "A sweep is already running" sent them
      // off to watch a table that cannot change. The server distinguishes the
      // cases; pass its message through rather than re-deciding here.
      const message = err.response?.data?.message;
      toast.error(typeof message === "string" ? message : "A sweep is already running");
    }
  });
};

/** Long enough for a small fleet's sweep to land before the first refetch. */
const SWEEP_REFETCH_DELAY_MS = 5000;
