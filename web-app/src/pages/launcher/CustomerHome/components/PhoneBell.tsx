import { Bell, X } from "@phosphor-icons/react";
import { focusRing, iconControl } from "@/components/ui/reference/tokens";
import { useUnreadNotificationCount } from "@/hooks/api/notifications";
import { cn } from "@/libs/shadcn/utils";

/**
 * The phone home's bell, at its greeting's right end (the top bar's hides on
 * this page). It shows the notifications in place of the apps, and while they
 * show it turns into an X that closes them. A red dot while anything is unread.
 *
 * Transcribed from oxy-hq/reference-app `src/views/Home.tsx` (`PhoneHome`,
 * branch design/home-match-oxygen).
 */
export function PhoneBell({ open, onToggle }: { open: boolean; onToggle: () => void }) {
  const { data: unread = 0 } = useUnreadNotificationCount();
  return (
    <button
      type='button'
      aria-expanded={open}
      aria-controls='home-notifications'
      aria-label={
        open ? "Close notifications" : unread ? `Notifications, ${unread} unread` : "Notifications"
      }
      onClick={onToggle}
      data-testid='home-phone-bell'
      className={cn(
        "relative -mr-2 flex size-11 shrink-0 items-center justify-center text-muted-foreground hover:text-foreground",
        focusRing
      )}
    >
      {open ? (
        <X className={iconControl} aria-hidden />
      ) : (
        <>
          <Bell className={iconControl} aria-hidden />
          {unread > 0 && (
            <span
              aria-hidden
              className='pointer-events-none absolute top-3 right-3 size-2 rounded-full bg-dot-error ring-2 ring-background'
            />
          )}
        </>
      )}
    </button>
  );
}
