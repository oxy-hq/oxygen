import { Bell } from "lucide-react";
import { useCallback, useState } from "react";
import { UnreadBadge } from "@/components/Notifications/UnreadBadge";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { Sheet, SheetContent, SheetTrigger } from "@/components/ui/shadcn/sheet";
import { useAuth } from "@/contexts/AuthContext";
import { useUnreadNotificationCount } from "@/hooks/api/notifications";
import { useMediaQuery } from "@/hooks/useMediaQuery";
import { InboxPanel } from "./components/InboxPanel";
import { POPOVER_MEDIA_QUERY } from "./constants";
import { useInboxOpenSignals } from "./useInboxOpenSignals";
import { badgeLabel } from "./utils";

/**
 * The bell and its inbox. One open state for both shapes, so whatever opens the
 * inbox (`useInboxOpenSignals`) opens whichever one the screen gets: a 320px
 * popover under the bell, or on a phone a sheet that fills the screen.
 */
function InboxBell() {
  const [open, setOpen] = useState(false);
  const openInbox = useCallback(() => setOpen(true), []);
  const close = useCallback(() => setOpen(false), []);
  useInboxOpenSignals(openInbox);
  const popover = useMediaQuery(POPOVER_MEDIA_QUERY);
  const { data: unread = 0 } = useUnreadNotificationCount();
  const badge = badgeLabel(unread);

  const bell = (
    <button
      type='button'
      data-testid='notifications-bell'
      aria-label={badge ? `Notifications, ${unread} unread` : "Notifications"}
      className='relative flex size-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground'
    >
      <Bell className='size-4' />
      <UnreadBadge
        count={unread}
        testId='notifications-badge'
        className='absolute -top-1 -right-1'
      />
    </button>
  );

  if (!popover) {
    return (
      <Sheet open={open} onOpenChange={setOpen}>
        <SheetTrigger asChild>{bell}</SheetTrigger>
        <SheetContent
          side='bottom'
          aria-describedby={undefined}
          // Focus the sheet itself, not its first button: while the list loads that
          // is the close button, and a tap should not light up its focus ring.
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            (event.currentTarget as HTMLElement | null)?.focus();
          }}
          className='inset-0 h-dvh gap-0 border-0 p-0 outline-none'
          data-testid='notifications-sheet'
        >
          <InboxPanel fullScreen onFollow={close} />
        </SheetContent>
      </Sheet>
    );
  }
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>{bell}</PopoverTrigger>
      <PopoverContent align='end' className='w-80 p-0' data-testid='notifications-popover'>
        <InboxPanel onFollow={close} />
      </PopoverContent>
    </Popover>
  );
}

/**
 * The inbox bell in the top bar: an unread badge, and the signed-in person's
 * notifications. Not in legacy local mode, which has no signed-in person to notify.
 */
export function NotificationsBell() {
  const { isLocalMode } = useAuth();
  if (isLocalMode) return null;
  return <InboxBell />;
}
