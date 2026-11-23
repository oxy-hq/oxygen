import { useQuery } from "@tanstack/react-query";
import { NotificationsService } from "@/services/api/notifications";
import queryKeys from "../queryKey";

/** How many rows the inbox shows. Read and unread alike, newest first. */
export const INBOX_LIMIT = 50;

/**
 * The inbox list. Read it only while the inbox is open: it reads the store of
 * every org on the page, which the badge (`useUnreadNotificationCount`) never does.
 * A list that stays mounted while shut (the docked panel) passes `enabled`.
 */
export const useNotificationInbox = (enabled = true) =>
  useQuery({
    queryKey: queryKeys.notifications.inbox(),
    queryFn: () => NotificationsService.inbox({ unreadOnly: false, limit: INBOX_LIMIT }),
    enabled
  });
