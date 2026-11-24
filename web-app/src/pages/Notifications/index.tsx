import { PushPrompt } from "@/components/Notifications/PushPrompt";
import {
  useMarkAllNotificationsRead,
  useNotificationCatalog,
  useNotificationFeed
} from "@/hooks/api/notifications";
import { useMediaQuery } from "@/hooks/useMediaQuery";
import type { InboxItem } from "@/types/notifications";
import { CategoryTree } from "./components/CategoryTree";
import { FilterSheet } from "./components/FilterSheet";
import { ListToolbar, ThreadChip } from "./components/ListToolbar";
import { NotificationList } from "./components/NotificationList";
import { PageHeader } from "./components/PageHeader";
import { StreamTabs } from "./components/StreamTabs";
import { WIDE_MEDIA_QUERY } from "./constants";
import { useNotificationsFilter } from "./useNotificationsFilter";
import { usePushClicks } from "./usePushClicks";
import { useStream } from "./useStream";
import { scopeLabel } from "./utils";

/**
 * `/notifications` — every notification the signed-in person has, from Oxygen
 * and every app, across every org they are in. Outside any org or workspace:
 * the inbox is the person's, not an org's.
 *
 * Wide: the category tree on the left, the list on the right. A phone: the list
 * fills the screen and the tree is a filter sheet. Either way the filter lives
 * in the URL (`useNotificationsFilter`).
 *
 * Someone who runs something — `catalog.operator` — gets the two streams as
 * tabs above the list, Activity selected: a failing hourly pipeline then stops
 * crowding out the notices meant for them personally. Everyone else sees the
 * page exactly as before, with no stream in the filter at all, so an operator
 * notice already in their inbox stays visible rather than moving into a tab
 * they do not have.
 */
export default function NotificationsPage() {
  const wide = useMediaQuery(WIDE_MEDIA_QUERY);
  const { filter, unreadOnly, setFilter, setUnreadOnly } = useNotificationsFilter();
  const catalog = useNotificationCatalog();
  const open = useStream(catalog.data, filter);
  const feed = useNotificationFeed(open.filter, unreadOnly);
  const markAll = useMarkAllNotificationsRead();
  const label = scopeLabel(catalog.data, filter);
  usePushClicks();

  const pages = feed.data?.pages ?? [];
  const canMarkAll =
    (pages[0]?.unread ?? 0) > 0 || pages.some((p) => p.items.some((i) => i.read_at === null));
  // A thread is narrower than any node of the tree, so it replaces the rest.
  const openThread = (item: InboxItem) => {
    if (!item.thread) return;
    setFilter({ stream: open.stream, source: item.app?.id ?? "oxygen", thread: item.thread });
  };
  const clearThread = () => setFilter({ ...filter, thread: undefined });

  const tree = {
    catalog: catalog.data,
    isPending: catalog.isPending,
    isError: catalog.isError,
    filter,
    onSelect: setFilter,
    stream: open.stream
  };
  // Switching tab starts over inside that stream: a category of the one being
  // left would narrow the new list to nothing.
  const tabs = open.tabs && open.stream && (
    <StreamTabs
      stream={open.stream}
      onSelect={(next) => setFilter({ stream: next })}
      counts={open.counts}
    />
  );
  const toolbar = (
    <ListToolbar
      unreadOnly={unreadOnly}
      onUnreadOnly={setUnreadOnly}
      canMarkAll={canMarkAll}
      markingAll={markAll.isPending}
      onMarkAll={() => markAll.mutate(open.filter)}
      compact={!wide}
    />
  );
  const thread = filter.thread ? <ThreadChip onClear={clearThread} /> : null;
  const list = (
    <NotificationList
      feed={feed}
      unreadOnly={unreadOnly}
      inThread={!!filter.thread}
      onOpenThread={openThread}
    />
  );

  return (
    <div
      className='flex h-full w-full min-w-0 flex-col bg-background'
      data-testid='notifications-page'
    >
      <PageHeader />
      {wide ? (
        <div className='flex min-h-0 flex-1'>
          <aside
            className='w-64 shrink-0 overflow-y-auto border-r p-2'
            data-testid='notifications-page-sidebar'
          >
            <CategoryTree {...tree} />
          </aside>
          <main className='min-w-0 flex-1 overflow-y-auto'>
            <div className='mx-auto flex w-full max-w-3xl flex-col'>
              {tabs && <div className='flex shrink-0 border-b px-4 py-2'>{tabs}</div>}
              <div className='flex items-center gap-2 border-b px-4 py-2'>
                <h2
                  className='min-w-0 flex-1 truncate font-medium text-sm'
                  data-testid='notifications-page-scope'
                >
                  {label}
                </h2>
                {thread}
                {toolbar}
              </div>
              <PushPrompt />
              {list}
            </div>
          </main>
        </div>
      ) : (
        <main className='flex min-h-0 flex-1 flex-col'>
          {tabs && <div className='flex shrink-0 border-b px-3 py-2'>{tabs}</div>}
          <div className='flex shrink-0 items-center gap-2 border-b px-3 py-2'>
            <FilterSheet {...tree} label={label} />
            {toolbar}
          </div>
          {thread && <div className='flex shrink-0 border-b px-3 py-2'>{thread}</div>}
          <PushPrompt fullScreen />
          <div className='min-h-0 flex-1 overflow-y-auto'>{list}</div>
        </main>
      )}
    </div>
  );
}
