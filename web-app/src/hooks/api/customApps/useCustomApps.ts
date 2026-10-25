import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { CustomAppsService } from "@/services/api/customApps";
import type { AppAvailabilityReading } from "@/types/apps";
import { errMessage } from "../errMessage";
import queryKeys from "../queryKey";

/**
 * Published custom apps for a workspace. Drives the HQ launcher and
 * workspace rail. Empty array (not error) when the workspace has none —
 * callers can render conditionally.
 */
export const useCustomApps = (workspaceId: string) =>
  useQuery({
    queryKey: queryKeys.customApps.list(workspaceId),
    queryFn: () => CustomAppsService.listForWorkspace(workspaceId),
    enabled: !!workspaceId
  });

/**
 * The launcher's list: the same published apps with each one's health `verdict`. A
 * separate query from `useCustomApps` on purpose — the verdict is an observability
 * read, and `useCustomApps` feeds the workspace rail on every page.
 */
export const useCustomAppsWithVerdict = (workspaceId: string) =>
  useQuery({
    queryKey: queryKeys.customApps.listWithVerdict(workspaceId),
    queryFn: () => CustomAppsService.listForWorkspaceWithVerdict(workspaceId),
    enabled: !!workspaceId
  });

export const usePublishApp = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: CustomAppsService.publish,
    onSuccess: (app) => {
      qc.invalidateQueries({ queryKey: queryKeys.customApps.all() });
      // Workspace-scoped list — invalidate the one for this app's workspace
      // so the sidebar picks up the new entry immediately.
      qc.invalidateQueries({ queryKey: queryKeys.customApps.list(app.project_id) });
      toast.success(`${app.name} published`);
    },
    onError: (err) => toast.error(errMessage(err, "Failed to publish"))
  });
};

export const useUnpublishApp = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: CustomAppsService.unpublish,
    onSuccess: (app) => {
      qc.invalidateQueries({ queryKey: queryKeys.customApps.all() });
      qc.invalidateQueries({ queryKey: queryKeys.customApps.list(app.project_id) });
      toast.success(`${app.name} unpublished`);
    },
    onError: (err) => toast.error(errMessage(err, "Failed to unpublish"))
  });
};

/**
 * The page size every admin surface asks for.
 *
 * `pageSize` is part of the query key, so two call sites differing by a digit are two
 * separate caches and two separate walks of the registry. The apps console and the
 * admin palette share this so that the palette is a cache hit rather than a second
 * fetch — which the palette's comment claimed before the number was shared.
 */
export const ADMIN_APP_PAGE_SIZE = 100;

/**
 * Paged admin list of custom apps, ordered by `updated_at` DESC so
 * the first page is the recently-active set — what staff usually
 * want when they open the admin. Additional pages walk back in time
 * via the server-returned `next_offset`.
 *
 * Every caller walks every page in an effect — the fleet list, the access pane and
 * (while it is open) the admin palette all filter or sort across the whole registry, so
 * a first page would give each of them a wrong answer rather than a partial one. The
 * palette was the exception until it was not: it read whatever the shared key happened
 * to hold, which is everything on /admin/apps* and page one elsewhere. (This doc also
 * said "we only fetch more when the user explicitly asks" long after that stopped being
 * true — a claim here is load-bearing, because it is what a new call site copies.)
 * Callers must therefore treat `isLoading` as first-page-only and wait on
 * `useAdminAppRegistry`'s `isWalking` before concluding an app does not exist.
 */
export const useAdminApps = (pageSize = 50, options: { enabled?: boolean } = {}) =>
  useInfiniteQuery({
    queryKey: [...queryKeys.customApps.all(), { pageSize }],
    queryFn: ({ pageParam }) =>
      CustomAppsService.list({ limit: pageSize, offset: pageParam as number }),
    initialPageParam: 0,
    getNextPageParam: (last) => last.next_offset,
    // The admin palette mounts on every admin page but only searches while open, so it
    // gates the registry rather than fetching it behind nine surfaces that never show it.
    // Defaulted on: every existing call site is unchanged.
    enabled: options.enabled ?? true
  });

/**
 * One app's admin detail. The registry list omits `staging_url` (a query per row),
 * so a surface that needs the staging host — the preview stage's Draft view —
 * reads this instead of the registry row.
 */
export const useAdminApp = (id: string | undefined) =>
  useQuery({
    queryKey: queryKeys.customApps.detail(id ?? ""),
    queryFn: () => CustomAppsService.get(id as string),
    enabled: !!id
  });

/** How often the console re-reads the one registry row it shows. */
const ROW_REFRESH_MS = 60_000;

/**
 * One registry row, read on its own (`GET /customer-apps?id=`) and polled.
 *
 * The row carries the app's `verdict` and `availability`; the registry walk that first
 * supplies it is not polled, and re-walking every page to refresh one app would cost the
 * whole fleet's evaluation each tick. This is the same list, narrowed to the app on screen —
 * the server keeps what it computed for 30 s, so the tick right after a registry read is
 * a cache hit.
 *
 * `retry: false` and a null for a row the caller cannot see: the caller keeps the row it
 * already holds rather than blanking the panel.
 */
export const useAdminAppRow = (id: string | undefined) =>
  useQuery({
    queryKey: queryKeys.customApps.row(id ?? ""),
    queryFn: async () => (await CustomAppsService.list({ id, limit: 1 })).items[0] ?? null,
    enabled: !!id,
    refetchInterval: ROW_REFRESH_MS,
    retry: false
  });

/**
 * The availability reading the console shows for an app: the one its registry row carries,
 * kept live by re-reading that one row (`?id=`). There is no per-app availability request.
 *
 * `reading` is `undefined` when the row carries none (a draft, a deployment that does not
 * measure availability, a store that did not answer); say so, never read it as healthy.
 * `isLoading` is true only while a caller with no reading of its own waits for the row.
 */
export function useListedAvailability(app: { id: string; availability?: AppAvailabilityReading }): {
  reading: AppAvailabilityReading | undefined;
  isLoading: boolean;
} {
  const row = useAdminAppRow(app.id);
  // A re-read row replaces the caller's reading, including by carrying none.
  if (row.data) {
    return { reading: row.data.availability, isLoading: false };
  }
  return { reading: app.availability, isLoading: !app.availability && row.isLoading };
}

/**
 * Diagnostic snapshot. Only fires when both slugs are present so the
 * hook is cheap to mount even on the list view. The 30s staleTime
 * keeps tab switches snappy without hiding fresh edits for long.
 */
export const useAppDebug = (orgSlug: string | undefined, appSlug: string | undefined) =>
  useQuery({
    queryKey: queryKeys.customApps.debug(orgSlug ?? "", appSlug ?? ""),
    queryFn: () =>
      CustomAppsService.debug({
        org_slug: orgSlug as string,
        slug: appSlug as string
      }),
    enabled: !!orgSlug && !!appSlug,
    staleTime: 30_000
  });

// ── Activity (usage tracking) ───────────────────────────────────────────

/**
 * Headline numbers (last viewed, 7d uniques, 7d total views/events)
 * for the AppDetail Activity tab. 30s staleTime so navigating away
 * and back doesn't refetch; the data isn't real-time-critical.
 */
/**
 * Persisted Oxy Function output and browser errors.
 *
 * `retry: false`: a 501 (no observability backend) is a state to render, not a
 * transient failure to retry behind a spinner.
 */
export const useAppLogs = (
  orgSlug: string | undefined,
  appSlug: string | undefined,
  query: { hours: number; limit: number; requestId?: string }
) =>
  useQuery({
    queryKey: queryKeys.customApps.logs(
      orgSlug ?? "",
      appSlug ?? "",
      query.hours,
      query.limit,
      query.requestId
    ),
    queryFn: () => CustomAppsService.logs(orgSlug as string, appSlug as string, query),
    enabled: !!orgSlug && !!appSlug,
    staleTime: 15_000,
    retry: false
  });

export const useAppClientErrors = (
  orgSlug: string | undefined,
  appSlug: string | undefined,
  hours = 24
) =>
  useQuery({
    queryKey: queryKeys.customApps.clientErrors(orgSlug ?? "", appSlug ?? "", hours),
    queryFn: () => CustomAppsService.clientErrors(orgSlug as string, appSlug as string, hours),
    enabled: !!orgSlug && !!appSlug,
    staleTime: 15_000,
    retry: false
  });

export const useAppActivitySummary = (appId: string | undefined) =>
  useQuery({
    queryKey: queryKeys.customApps.activitySummary(appId ?? ""),
    queryFn: () => CustomAppsService.activitySummary(appId as string),
    enabled: !!appId,
    staleTime: 30_000
  });

export const useAppActivityVisitors = (appId: string | undefined, days = 7) =>
  useQuery({
    queryKey: queryKeys.customApps.activityVisitors(appId ?? "", days),
    queryFn: () => CustomAppsService.activityVisitors(appId as string, days),
    enabled: !!appId,
    staleTime: 30_000
  });

/**
 * Per-event-name counts + last-fired timestamps for the Activity tab's
 * Events table. Split from the occurrences hook because they return
 * different row shapes — folding both into one `useQuery` widened
 * the `queryFn` return into a union TanStack Query's overloads
 * can't satisfy (TS2769 in CI). One hook per shape keeps the
 * generics clean.
 */
export const useAppActivityEventGroups = (appId: string | undefined, days = 7) =>
  useQuery({
    queryKey: queryKeys.customApps.activityEvents(appId ?? "", days, null),
    queryFn: () => CustomAppsService.activityEventGroups(appId as string, days),
    enabled: !!appId,
    staleTime: 30_000
  });

/**
 * Recent occurrences for a single event_name — the drill-down view
 * the Activity tab opens when an operator clicks a row in the
 * Events table. Disabled when `eventName` is null so the call site
 * can `useState<string | null>` and pass it through without a
 * conditional hook.
 */
export const useAppActivityEventOccurrences = (
  appId: string | undefined,
  eventName: string | null,
  days = 7
) =>
  useQuery({
    queryKey: queryKeys.customApps.activityEvents(appId ?? "", days, eventName),
    queryFn: () =>
      CustomAppsService.activityEventOccurrences(appId as string, eventName as string, days),
    enabled: !!appId && !!eventName,
    staleTime: 30_000
  });

// ── Staging (held writes) ───────────────────────────────────────────────

/**
 * The caller's own held-write rows for `appId`'s staging, newest first.
 *
 * `enabled` is the console's own ruling, not a convenience default: the
 * banner that reads this list must render only while the console is
 * actually framing staging (channel `draft` AND the target resolved to the
 * staging host), so the caller passes that exact condition through rather
 * than this hook re-deriving it. Polling (`refetchInterval`) follows the
 * same flag — once `enabled` goes false the interval stops instead of
 * ticking in the background for a frame nobody is looking at — and stops
 * for good on a 404, which no later poll can turn into a list.
 *
 * `retry: false`: a 404 (the
 * caller may not open this app's staging) is a state the banner renders,
 * not a transient failure to retry behind a spinner.
 *
 * `poll: false` is for a reader that wants the count once — the lifecycle
 * card states "N writes held" beside the draft build and nobody is watching
 * it tick. Same key, so it shares the banner's cache entry when both mount.
 */
export const useStagingHeld = (
  appId: string,
  enabled: boolean,
  { poll = true }: { poll?: boolean } = {}
) =>
  useQuery({
    queryKey: queryKeys.customApps.stagingHeld(appId),
    queryFn: () => CustomAppsService.listStagingHeld(appId, STAGING_HELD_LIMIT),
    enabled,
    refetchInterval: (query) =>
      poll && enabled && !isNotFound(query.state.error) ? STAGING_HELD_POLL_MS : false,
    retry: false
  });

/**
 * How many held rows the console asks for. The banner reads a list this
 * long as possibly truncated ("N+ writes held"), so it must be the limit
 * actually sent, not the server's default.
 */
export const STAGING_HELD_LIMIT = 100;

const STAGING_HELD_POLL_MS = 10_000;

/**
 * A 404 is terminal for this list: the caller may not open the app's
 * staging, and polling will not change that — so polling stops on it.
 */
export const isNotFound = (error: unknown): boolean =>
  (error as { response?: { status?: number } } | null)?.response?.status === 404;
