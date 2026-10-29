import { useQuery } from "@tanstack/react-query";
import { NotificationsService } from "@/services/api/notifications";
import type { UnreadResponse } from "@/types/notifications";
import queryKeys from "../queryKey";

/** How often the badge re-reads its count while the tab is visible. */
export const UNREAD_POLL_MS = 60_000;

/** The one polled read both the bell and the app badges select from. */
const unreadQuery = {
  queryKey: queryKeys.notifications.unread(),
  queryFn: () => NotificationsService.unread(),
  refetchInterval: UNREAD_POLL_MS,
  refetchOnWindowFocus: true
};

/**
 * The bell's badge count. Polled, and re-read when the window regains focus — a
 * push that arrives while the tab is open also marks it stale (see the inbox bell).
 */
export const useUnreadNotificationCount = () =>
  useQuery({ ...unreadQuery, select: (data: UnreadResponse) => data.unread });

/** App id → its unread count, apps with none left out. */
export type AppUnreadCounts = Readonly<Record<string, number>>;

const NO_COUNTS: AppUnreadCounts = {};

// Module scope, so React Query keeps the selected object while the data is the same.
function byApp(data: UnreadResponse): AppUnreadCounts {
  if (!data.apps?.length) return NO_COUNTS;
  return Object.fromEntries(data.apps.map((a) => [a.app_id, a.unread]));
}

/**
 * Each app's unread count, for the badges on the home's app cards and the rail.
 * The same request as the bell's count, so it costs nothing more. `enabled` is
 * off in legacy local mode, which has no signed-in person to notify.
 */
export function useAppUnreadCounts(enabled = true): AppUnreadCounts {
  const { data } = useQuery({ ...unreadQuery, select: byApp, enabled });
  return data ?? NO_COUNTS;
}
