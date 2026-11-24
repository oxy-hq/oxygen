import { useQuery } from "@tanstack/react-query";
import { NotificationsService } from "@/services/api/notifications";
import type { StreamUnread, UnreadResponse } from "@/types/notifications";
import queryKeys from "../queryKey";

/** How often the stream counts re-read, matching the bell's poll. */
const UNREAD_POLL_MS = 60_000;

/**
 * Unread per stream, for the notifications page's tabs. The same polled read as
 * the bell's badge (one query key, so one request), selected differently —
 * `undefined` from a server that predates streams, which is also when the tabs
 * are not drawn.
 */
export const useUnreadByStream = () =>
  useQuery({
    queryKey: queryKeys.notifications.unread(),
    queryFn: () => NotificationsService.unread(),
    refetchInterval: UNREAD_POLL_MS,
    refetchOnWindowFocus: true,
    select: (data: UnreadResponse): StreamUnread | undefined => data.streams
  });
