import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { NotificationsService } from "@/services/api/notifications";
import type { InboxItem, InboxResponse } from "@/types/notifications";
import queryKeys from "../queryKey";
import { INBOX_LIMIT } from "./useNotificationInbox";
import { useUnreadNotificationCount } from "./useUnreadNotificationCount";

/** App id → its newest unread notification. */
export type LatestUnreadByApp = Readonly<Record<string, InboxItem>>;

const NONE: LatestUnreadByApp = {};

// Newest first from the server, so the first item seen per app is its latest.
function latestPerApp(data: InboxResponse): LatestUnreadByApp {
  const latest: Record<string, InboxItem> = {};
  for (const item of data.items) {
    if (item.app && !latest[item.app.id]) latest[item.app.id] = item;
  }
  return latest;
}

/**
 * Each app's newest unread notification, for the banner on its home card.
 *
 * This reads org stores (what a notification says lives there), which the polled
 * badge never does. So it reads only while something is unread, never on focus,
 * and again only when the badge's total changes (`latestUnread(total)`) or a read
 * re-reads the inbox. An app whose newest unread is past the first page gets its
 * badge without a banner.
 */
export function useLatestUnreadByApp(enabled = true): LatestUnreadByApp {
  const { data: unread = 0 } = useUnreadNotificationCount();
  const { data } = useQuery({
    queryKey: queryKeys.notifications.latestUnread(unread),
    queryFn: () => NotificationsService.inbox({ unreadOnly: true, limit: INBOX_LIMIT }),
    select: latestPerApp,
    enabled: enabled && unread > 0,
    refetchOnWindowFocus: false,
    // Keep the banners up while the next total's read is in flight.
    placeholderData: keepPreviousData
  });
  return enabled && unread > 0 ? (data ?? NONE) : NONE;
}
