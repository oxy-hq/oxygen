import { useQuery } from "@tanstack/react-query";
import { NotificationsService } from "@/services/api/notifications";
import type { UnreadResponse } from "@/types/notifications";
import queryKeys from "../queryKey";

/** How often the badge re-reads its count while the tab is visible. */
const UNREAD_POLL_MS = 60_000;

/** The bell's polled read. */
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
