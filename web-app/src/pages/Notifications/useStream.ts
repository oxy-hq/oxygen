import { useUnreadByStream } from "@/hooks/api/notifications";
import type {
  InboxFilter,
  NotificationCatalog,
  NotificationStream,
  StreamUnread
} from "@/types/notifications";
import { kindStream } from "./utils";

/** The default tab: what happened to you, rather than to the workspace. */
const FIRST: NotificationStream = "activity";

export interface Stream {
  /** The open tab, or `undefined` for someone who runs nothing. */
  stream: NotificationStream | undefined;
  /** The page's filter as every read and write should send it. */
  filter: InboxFilter;
  /** Whether to draw the tabs at all. */
  tabs: boolean;
  /** Unread per stream, for the tab labels; `undefined` until the badge answers. */
  counts: StreamUnread | undefined;
}

/**
 * Which stream the notifications page is showing.
 *
 * Someone who runs something always has a tab open, so a tab is always the
 * truth about what the list holds. For everyone else there is **no stream in
 * the filter at all** — not "activity" — which is what keeps an operator notice
 * already in their inbox visible rather than moving it into a tab they do not
 * have.
 *
 * **A link naming one item is narrower than any tab, so it is not narrowed
 * again.** A clicked push lands on `?source=oxygen&kind=oxy.run_failed`
 * (`web_push_payload::href`, which carries a kind or a thread and never a
 * stream), and defaulting that to Activity asks the server for a kind Activity
 * can never hold — an empty list, and no Automations branch in the tree either,
 * for a link that worked before this existed. So when the URL names a `kind` or
 * a `thread`, the request carries no stream; the tab is only what is drawn as
 * open, picked from the kind's own stream when the catalogue knows it.
 *
 * `catalog` decides who gets tabs (`operator`), so they and the server's own
 * segmentation cannot disagree; a server that predates streams says nothing and
 * the page behaves exactly as it did.
 */
export function useStream(catalog: NotificationCatalog | undefined, filter: InboxFilter): Stream {
  const operator = catalog?.operator ?? false;
  // One item is named: whichever tab is drawn, it must not narrow the list.
  const pinned = !!(filter.kind || filter.thread);
  const open = filter.stream ?? kindStream(catalog, filter.kind) ?? FIRST;
  const stream = operator ? open : undefined;
  const counts = useUnreadByStream();
  // `stream` is stripped rather than merely left unset: the URL keeps it so
  // clearing the thread returns to the tab you came from, and a thread opened
  // from the Operations tab must not be asked for within Activity either.
  const { stream: _inUrl, ...unnarrowed } = filter;
  return {
    stream,
    filter: pinned ? unnarrowed : stream ? { ...filter, stream } : filter,
    tabs: operator,
    counts: counts.data
  };
}
