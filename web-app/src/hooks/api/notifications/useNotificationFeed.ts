import { useInfiniteQuery } from "@tanstack/react-query";
import { NotificationsService } from "@/services/api/notifications";
import type { InboxFilter } from "@/types/notifications";
import queryKeys from "../queryKey";

/** How many notifications one page of the notifications page holds. */
const FEED_PAGE_SIZE = 50;

/**
 * The notifications page's list: newest first, narrowed by `filter`, and paged
 * back through history with the `before` cursor. A server too old to send
 * `next_before` has no second page.
 */
export const useNotificationFeed = (filter: InboxFilter, unreadOnly: boolean) =>
  useInfiniteQuery({
    queryKey: queryKeys.notifications.feed(filter, unreadOnly),
    queryFn: ({ pageParam }) =>
      NotificationsService.inbox({
        ...filter,
        unreadOnly,
        limit: FEED_PAGE_SIZE,
        before: pageParam
      }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next_before ?? undefined
  });
