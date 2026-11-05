import { AppMark } from "@/components/apps/AppMark";
import { UnreadBadge } from "@/components/Notifications/UnreadBadge";
import { useAppUnreadCounts } from "@/hooks/api/notifications";
import { prefetchApp } from "@/libs/utils/prefetchApp";
import type { CustomAppSummary } from "@/types/apps";

/**
 * Apps as a phone's home screen draws them, at every width: the icon, the name
 * under it, the unread count in the icon's corner, and nothing else. The simple
 * home's whole page is these.
 *
 * `testId` names each app's link, so the recent row and the full grid can both
 * hold an app without two elements answering to one id.
 */
export function HomeAppIcons({
  apps,
  testId
}: {
  apps: readonly CustomAppSummary[];
  testId: (app: CustomAppSummary) => string;
}) {
  const unread = useAppUnreadCounts();
  return (
    <ul className='grid grid-cols-4 gap-x-1 gap-y-6 sm:grid-cols-5 sm:gap-x-3 md:grid-cols-6'>
      {apps.map((app) => (
        <li key={app.id} className='min-w-0'>
          <HomeAppIcon app={app} unread={unread[app.id] ?? 0} testId={testId(app)} />
        </li>
      ))}
    </ul>
  );
}

function HomeAppIcon({
  app,
  unread,
  testId
}: {
  app: CustomAppSummary;
  unread: number;
  testId: string;
}) {
  const warm = () => prefetchApp(app.url);
  return (
    <div
      // Gives slightly under a press, like a button; the name's link covers the whole icon.
      className='group relative flex flex-col items-center gap-2.5 rounded-xl py-1 text-center transition-[scale] duration-(--duration-fast) focus-within:ring-2 focus-within:ring-ring has-[a:active]:scale-95 has-[a:active]:duration-(--duration-instant) motion-reduce:has-[a:active]:scale-100 sm:px-1'
      // What grows into the app's launch screen when it is pressed.
      data-app-launch-origin
    >
      <div className='relative'>
        <AppMark
          iconUrl={app.icon_url}
          name={app.name}
          size='xl'
          className='size-16 rounded-2xl text-lg transition-[border-color,box-shadow] duration-(--duration-fast) group-hover:border-primary/50 group-hover:shadow-sm'
        />
        <UnreadBadge
          count={unread}
          className='absolute -top-1.5 -right-1.5 h-5 min-w-5 px-1.5 shadow-sm'
        />
      </div>
      <a
        href={app.url}
        data-testid={testId}
        title={app.name}
        // Two lines at most, hyphenated where the browser can.
        className='line-clamp-2 w-full hyphens-auto break-words font-medium text-foreground text-xs leading-snug after:absolute after:inset-0 after:content-[""] focus-visible:outline-none sm:text-sm'
        onPointerEnter={warm}
        onFocus={warm}
      >
        {app.name}
        {unread > 0 && <span className='sr-only'>, {unread} unread</span>}
      </a>
    </div>
  );
}
