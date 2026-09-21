import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { CustomAppsService } from "@/services/api/customApps";
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
 * Derived availability + burn verdict for one app.
 *
 * Polled on a 60s interval rather than fetched once: this is the panel an
 * operator leaves open during an incident, and a stale verdict there is worse
 * than no panel. `retry: false` because the two interesting failures — 501 when
 * observability is unconfigured, 502 when the query itself fails — are both
 * states to render, not transient errors to retry behind a spinner.
 */
export const useAppAvailability = (orgSlug: string | undefined, appSlug: string | undefined) =>
  useQuery({
    queryKey: queryKeys.customApps.availability(orgSlug ?? "", appSlug ?? ""),
    queryFn: () => CustomAppsService.availability(orgSlug as string, appSlug as string),
    enabled: !!orgSlug && !!appSlug,
    staleTime: 30_000,
    refetchInterval: 60_000,
    retry: false
  });

/**
 * Persisted Oxy Function output and browser errors.
 *
 * `retry: false` for the same reason as `useAppAvailability`: a 501 (no
 * observability backend) is a state to render, not a transient failure to
 * retry behind a spinner.
 */
export const useAppLogs = (orgSlug: string | undefined, appSlug: string | undefined, hours = 24) =>
  useQuery({
    queryKey: queryKeys.customApps.logs(orgSlug ?? "", appSlug ?? "", hours),
    queryFn: () => CustomAppsService.logs(orgSlug as string, appSlug as string, hours),
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
