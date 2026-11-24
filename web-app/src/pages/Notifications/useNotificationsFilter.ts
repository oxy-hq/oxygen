import { useCallback, useMemo } from "react";
import { useSearchParams } from "react-router-dom";
import type { InboxFilter } from "@/types/notifications";

const FILTER_KEYS = ["source", "category", "kind", "thread", "stream"] as const;
const UNREAD_PARAM = "unread";

/**
 * The page's filter, kept in the URL so a link (or a refresh) lands on the same
 * list:
 * `?source=<oxygen|app id>&category=<path>&kind=<kind>&thread=<id>&stream=<activity|operations>&unread=1`.
 * Every change replaces the entry, so Back leaves the page rather than stepping
 * through filters.
 */
export function useNotificationsFilter() {
  const [params, setParams] = useSearchParams();

  const filter = useMemo(() => {
    const next: InboxFilter = {};
    for (const key of FILTER_KEYS) {
      const value = params.get(key);
      if (!value) continue;
      // A stream the server would refuse is dropped here rather than sent: a
      // hand-edited URL should not 400 the whole page.
      if (key === "stream") {
        if (value === "activity" || value === "operations") next.stream = value;
        continue;
      }
      next[key] = value;
    }
    return next;
  }, [params]);
  const unreadOnly = params.get(UNREAD_PARAM) === "1";

  const setFilter = useCallback(
    (next: InboxFilter) =>
      setParams(
        (prev) => {
          const out = new URLSearchParams(prev);
          for (const key of FILTER_KEYS) {
            const value = next[key];
            if (value) out.set(key, value);
            else out.delete(key);
          }
          return out;
        },
        { replace: true }
      ),
    [setParams]
  );

  const setUnreadOnly = useCallback(
    (on: boolean) =>
      setParams(
        (prev) => {
          const out = new URLSearchParams(prev);
          if (on) out.set(UNREAD_PARAM, "1");
          else out.delete(UNREAD_PARAM);
          return out;
        },
        { replace: true }
      ),
    [setParams]
  );

  return { filter, unreadOnly, setFilter, setUnreadOnly };
}
