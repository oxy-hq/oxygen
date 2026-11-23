import { Bell } from "@phosphor-icons/react";
import { useCallback, useState } from "react";
import { NoticeList } from "@/components/Notifications/NoticeList";
import { NOTIFICATIONS_DOCK_QUERY } from "@/components/Shell/NotificationsDock";
import { DESIGN_REFERENCE, focusRing, iconControl } from "@/components/ui/reference/tokens";
import { Sheet, SheetContent, SheetTitle } from "@/components/ui/shadcn/sheet";
import { useAuth } from "@/contexts/AuthContext";
import { useUnreadNotificationCount } from "@/hooks/api/notifications";
import { useMediaQuery } from "@/hooks/useMediaQuery";
import { cn } from "@/libs/shadcn/utils";
import useNotificationsDock from "@/stores/useNotificationsDock";
import { useInboxOpenSignals } from "./useInboxOpenSignals";

/**
 * The top bar's bell. No popup: on a desk it opens and closes the
 * notifications panel docked beside the page (`NotificationsDock`); narrower,
 * a sheet that fills the screen. While anything is unread a red dot sits on its
 * shoulder; the number is read out with its name, not drawn.
 *
 * Transcribed from oxy-hq/reference-app `src/views/parts/HomeNotices.tsx`
 * (`HomeBell`, branch design/home-match-oxygen).
 */
function Bell4() {
  const docks = useMediaQuery(NOTIFICATIONS_DOCK_QUERY);
  const dockOpen = useNotificationsDock((s) => s.isOpen);
  const toggleDock = useNotificationsDock((s) => s.toggle);
  const openDock = useNotificationsDock((s) => s.open);
  const [sheetOpen, setSheetOpen] = useState(false);
  const open = docks ? dockOpen : sheetOpen;
  const openInbox = useCallback(() => (docks ? openDock() : setSheetOpen(true)), [docks, openDock]);
  useInboxOpenSignals(openInbox);
  const { data: unread = 0 } = useUnreadNotificationCount();

  return (
    <>
      <button
        type='button'
        aria-expanded={open}
        aria-controls={docks ? "notifications-dock" : undefined}
        aria-label={unread ? `Notifications, ${unread} unread` : "Notifications"}
        onClick={() => (docks ? toggleDock() : setSheetOpen((o) => !o))}
        data-testid='notifications-bell'
        className={cn(
          "relative flex size-8 shrink-0 items-center justify-center text-muted-foreground transition-colors hover:bg-row-hover hover:text-foreground",
          open && "bg-row-selected text-foreground",
          focusRing
        )}
      >
        <Bell className={iconControl} aria-hidden />
        {unread > 0 && (
          <span
            aria-hidden
            data-testid='notifications-dot'
            // On the bell's shoulder, ringed in the bar's ground so it reads on the icon's stroke.
            className='pointer-events-none absolute top-1.5 right-1.5 size-2 rounded-full bg-dot-error ring-2 ring-background'
          />
        )}
      </button>
      {!docks && (
        <Sheet open={sheetOpen} onOpenChange={setSheetOpen}>
          <SheetContent
            side='bottom'
            aria-describedby={undefined}
            onOpenAutoFocus={(event) => {
              event.preventDefault();
              (event.currentTarget as HTMLElement | null)?.focus();
            }}
            className={cn(
              DESIGN_REFERENCE,
              "inset-0 h-dvh gap-0 overflow-y-auto border-0 bg-background p-0 outline-none"
            )}
            data-testid='notifications-sheet'
          >
            <div className='flex h-12 shrink-0 items-center border-border-subtle border-b bg-sidebar pr-12 pl-4'>
              <SheetTitle className='font-semibold text-base'>Notifications</SheetTitle>
            </div>
            <NoticeList onFollow={() => setSheetOpen(false)} />
          </SheetContent>
        </Sheet>
      )}
    </>
  );
}

/** Not in legacy local mode, which has no signed-in person to notify. */
export function DockBell() {
  const { isLocalMode } = useAuth();
  if (isLocalMode) return null;
  return <Bell4 />;
}
