import { Tabs, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import type { NotificationStream, StreamUnread } from "@/types/notifications";

interface StreamTabsProps {
  stream: NotificationStream;
  onSelect: (stream: NotificationStream) => void;
  /** Unread per stream, from the badge's poll. Absent until it answers. */
  counts: StreamUnread | undefined;
}

const TABS: { value: NotificationStream; label: string }[] = [
  { value: "activity", label: "Activity" },
  { value: "operations", label: "Operations" }
];

/**
 * The two streams, for someone who runs something: **Activity** is what
 * happened to them — their apps' notices, an answer they asked for, an alarm
 * they were named in — and **Operations** is what happened to the workspace: a
 * failed run, an unpaid invoice.
 *
 * Only an operator sees this (`catalog.operator`); everyone else sees the page
 * exactly as before, because everything they can receive is Activity.
 */
export function StreamTabs({ stream, onSelect, counts }: StreamTabsProps) {
  return (
    <Tabs
      value={stream}
      onValueChange={(v) => onSelect(v as NotificationStream)}
      data-testid='notifications-streams'
    >
      <TabsList>
        {TABS.map(({ value, label }) => {
          const unread = counts?.[value] ?? 0;
          return (
            <TabsTrigger key={value} value={value} data-testid={`notifications-stream-${value}`}>
              {label}
              {unread > 0 && (
                <span className='ml-1.5 text-muted-foreground text-xs tabular-nums'>{unread}</span>
              )}
            </TabsTrigger>
          );
        })}
      </TabsList>
    </Tabs>
  );
}
