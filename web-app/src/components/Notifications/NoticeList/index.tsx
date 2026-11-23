import { Checks } from "@phosphor-icons/react";
import { type ReactNode, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { PushPrompt } from "@/components/Notifications/PushPrompt";
import {
  btnText,
  iconInline,
  note,
  segment,
  segmentOff,
  segmentOn,
  segmentRow
} from "@/components/ui/reference/tokens";
import useOrgCustomApps from "@/hooks/api/customApps/useOrgCustomApps";
import {
  useMarkAllNotificationsRead,
  useMarkNotificationRead,
  useNotificationInbox
} from "@/hooks/api/notifications";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import type { InboxItem } from "@/types/notifications";
import { NoticeRow } from "./NoticeRow";

type Filter = "unread" | "all";

/** Unread or every notification: the segmented pair. */
function FilterSwitch({ value, onChange }: { value: Filter; onChange: (v: Filter) => void }) {
  return (
    <fieldset className={cn(segmentRow, "m-0 min-w-0 p-0")} aria-label='Show'>
      {(["unread", "all"] as const).map((v) => (
        <button
          key={v}
          type='button'
          aria-pressed={value === v}
          onClick={() => onChange(v)}
          data-testid={`notifications-filter-${v}`}
          className={cn(segment, "min-h-8", value === v ? segmentOn : segmentOff)}
        >
          {v === "unread" ? "Unread" : "All"}
        </button>
      ))}
    </fieldset>
  );
}

const unreadIds = (items: InboxItem[]) => items.filter((i) => i.read_at === null).map((i) => i.id);

/**
 * The Unread view's rows: what was unread when it was chosen, or when the panel
 * last opened, so a row read there turns read in place instead of leaving from
 * under the pointer. Adjusted while rendering, not in an effect, so a reopened
 * panel never shows a frame of last time's rows.
 */
function useHeld(items: InboxItem[], open: boolean, loaded: boolean) {
  const [held, setHeld] = useState<string[] | null>(null);
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) setHeld(null);
  }
  // Taken the first time the rows are there after opening.
  if (held === null && loaded && open) setHeld(unreadIds(items));
  return { held: held ?? [], hold: () => setHeld(unreadIds(items)), drop: () => setHeld([]) };
}

/**
 * The notifications panel's contents: the Unread / All band across its top,
 * the offer to turn on push here (when it applies), then one stacked list, edge
 * to edge, newest first, and the way to the notifications page. "Mark all read"
 * empties the Unread view.
 *
 * `open` is the panel's: on a desk the list stays mounted while the panel is
 * shut and reads nothing then; opening it again is what lets the rows read last
 * time go. A list mounted only while it shows leaves `open` at its default.
 *
 * Transcribed from oxy-hq/reference-app `src/views/parts/HomeNotices.tsx`
 * (`HomeNoticeList`, branch design/home-match-oxygen) onto the inbox.
 */
export function NoticeList({ open = true, onFollow }: { open?: boolean; onFollow?: () => void }) {
  const inbox = useNotificationInbox(open);
  const markOne = useMarkNotificationRead();
  const markAll = useMarkAllNotificationsRead();
  const { workspace } = useCurrentWorkspace();
  const { apps } = useOrgCustomApps(workspace?.id ?? "");
  const icons = useMemo(() => new Map(apps.map((a) => [a.id, a.icon_url])), [apps]);
  const [filter, setFilter] = useState<Filter>("unread");
  const items = inbox.data?.items ?? [];
  const { held, hold, drop } = useHeld(items, open, inbox.data !== undefined);
  const unread = items.filter((i) => i.read_at === null).length;
  const shown =
    filter === "unread" ? items.filter((i) => i.read_at === null || held.includes(i.id)) : items;

  const choose = (next: Filter) => {
    setFilter(next);
    if (next === "unread") hold();
  };

  let body: ReactNode;
  if (inbox.isPending) {
    body = <State testId='notifications-loading'>Loading…</State>;
  } else if (inbox.isError) {
    body = <State testId='notifications-error'>Couldn't load notifications</State>;
  } else if (shown.length === 0) {
    body = (
      <State testId='notifications-empty'>
        {filter === "unread" ? "No unread notifications." : "You're all caught up"}
      </State>
    );
  } else {
    body = (
      <ul
        className='m-0 flex list-none flex-col divide-y divide-border-subtle border-border-subtle border-b p-0'
        data-testid='notifications-list'
      >
        {shown.map((item) => (
          <NoticeRow
            key={item.id}
            item={item}
            iconUrl={item.app ? icons.get(item.app.id) : undefined}
            onRead={() => markOne.mutate(item)}
            onFollow={onFollow}
          />
        ))}
      </ul>
    );
  }

  return (
    <section className='flex flex-col' aria-label='Notifications' data-testid='notifications-panel'>
      <div className='flex items-center justify-between gap-2 border-border-subtle border-b bg-card px-4 py-3'>
        <FilterSwitch value={filter} onChange={choose} />
        <button
          type='button'
          onClick={() => {
            markAll.mutate({});
            drop();
          }}
          disabled={unread === 0 || markAll.isPending}
          data-testid='notifications-mark-all-read'
          // The segments' hover fill, with room either side of the label.
          className={cn(btnText, "-mr-[3px] px-2 enabled:hover:bg-row-hover")}
        >
          <Checks aria-hidden className={iconInline} />
          Mark all read
        </button>
      </div>
      <PushPrompt />
      {body}
      <Link
        to={ROUTES.NOTIFICATIONS}
        onClick={onFollow}
        data-testid='notifications-see-all'
        className={cn(note, "px-4 py-3 text-center hover:bg-row-hover hover:text-foreground")}
      >
        See all notifications
      </Link>
    </section>
  );
}

function State({ testId, children }: { testId: string; children: ReactNode }) {
  return (
    <p className={cn(note, "m-0 py-6 text-center")} data-testid={testId}>
      {children}
    </p>
  );
}
