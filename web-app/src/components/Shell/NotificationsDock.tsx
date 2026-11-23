import { useEffect } from "react";
import { NoticeList } from "@/components/Notifications/NoticeList";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { useMediaQuery } from "@/hooks/useMediaQuery";
import { cn } from "@/libs/shadcn/utils";
import useAskDock from "@/stores/useAskDock";
import useNotificationsDock from "@/stores/useNotificationsDock";

/** The panel's width when open. */
const PANEL_PX = 440;

/** From here the panel docks beside the page; narrower, the bell opens a full-screen sheet. */
export const NOTIFICATIONS_DOCK_QUERY = "(min-width: 768px)";

/**
 * The notifications panel, opened by the top bar's bell: a
 * right-hand sibling of the page that narrows it rather than lying over it, as
 * the Ask dock does, full height under the top bar. Its width eases open and
 * shut; it stays mounted, `inert` while shut, so the list keeps its place, and
 * reads the inbox only while open. One panel on that edge at a time: Ask
 * opening closes it.
 *
 * Transcribed from oxy-hq/reference-app `src/views/Home.tsx` (branch
 * design/home-match-oxygen), where the panel is the home's; here it is the
 * shell's, so the bell opens it from any page.
 */
export function NotificationsDock() {
  const open = useNotificationsDock((s) => s.isOpen);
  const close = useNotificationsDock((s) => s.close);
  const docks = useMediaQuery(NOTIFICATIONS_DOCK_QUERY);
  const shown = open && docks;

  // Ask opening takes the edge.
  useEffect(
    () =>
      useAskDock.subscribe((s, was) => {
        if (s.isOpen && !was.isOpen) close();
      }),
    [close]
  );

  if (!docks) return null;
  return (
    <aside
      id='notifications-dock'
      aria-label='Notifications'
      data-testid='notifications-dock'
      inert={!shown}
      style={{ width: shown ? PANEL_PX : 0 }}
      className={cn(
        DESIGN_REFERENCE,
        "flex h-full shrink-0 flex-col overflow-hidden bg-background transition-[width] duration-300 ease-out motion-reduce:transition-none",
        shown ? "border-border-subtle border-l" : "pointer-events-none"
      )}
    >
      {/* At its open width always, so the list never rewraps while the panel eases. */}
      <div className='min-h-0 flex-1 overflow-y-auto' style={{ width: PANEL_PX }}>
        <NoticeList open={shown} />
      </div>
    </aside>
  );
}
