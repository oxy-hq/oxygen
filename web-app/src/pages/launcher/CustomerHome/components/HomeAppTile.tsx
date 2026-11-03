import { UserCog } from "lucide-react";
import { AppAccessBadge } from "@/components/appAccess/AppAccessBadge";
import { AppMark } from "@/components/apps/AppMark";
import { UnreadBadge } from "@/components/Notifications/UnreadBadge";
import { Button } from "@/components/ui/shadcn/button";
import { prefetchApp } from "@/libs/utils/prefetchApp";
import type { CustomAppSummary } from "@/types/apps";
import type { InboxItem } from "@/types/notifications";
import { appFreshnessLine } from "../../components/appFreshness";

/**
 * One app on the home, sized so the whole set fits without scrolling.
 *
 * On a phone it is an icon with the name under it, four to a row, like a phone's
 * home screen: the icon carries the unread count in its corner and is the tap
 * target. Once the grid has 24rem (`@sm`, a container query) it becomes a row:
 * icon, name, and one line under the name, the newest unread notification when
 * there is one, else how fresh the app is.
 *
 * It opens the app the way the launcher's card does (a plain anchor, this tab,
 * warmed on hover and focus). Owners and admins get the access control beside
 * the name in the row form (people, not a shield: it manages who may open the app);
 * on a phone the grid has no room for it, and access is managed from the launcher.
 */
export function HomeAppTile({
  app,
  unread = 0,
  latest,
  onManageAccess
}: {
  app: CustomAppSummary;
  /** How many of this app's notifications the viewer has not read. */
  unread?: number;
  /** The newest of them, when the inbox's first page holds it. */
  latest?: InboxItem;
  /** Open the access dialog for this app. Passed only for org owners and admins. */
  onManageAccess?: (app: CustomAppSummary) => void;
}) {
  const warm = () => prefetchApp(app.url);
  const freshness = appFreshnessLine(app);
  // Null when the org's store did not answer for its text; then the badge alone.
  const banner = unread > 0 ? (latest?.title ?? latest?.body ?? null) : null;

  return (
    // A div, not an anchor, because the tile holds a second action (access). The
    // name's link stretches over the whole tile with `after:inset-0`; the access
    // button is its sibling, lifted above the overlay with `z-10`.
    <div
      // Gives slightly under a press (the name's link covers the tile), like a button.
      className='group relative flex min-w-0 @sm:flex-row flex-col items-center @sm:gap-3 gap-1.5 rounded-lg @sm:border @sm:bg-card @sm:p-3 @sm:text-left text-center transition-[border-color,scale] duration-(--duration-fast) focus-within:ring-2 focus-within:ring-ring @sm:hover:border-primary/50 has-[a:active]:scale-98 has-[a:active]:duration-(--duration-instant) motion-reduce:has-[a:active]:scale-100'
      data-testid={`home-app-card-${app.slug}`}
      // What grows into the app's launch screen when the tile is pressed.
      data-app-launch-origin
      // A name of its own, so a layout change (Today arriving) moves the tile, not a cross-fade.
      style={{ viewTransitionName: `home-app-${app.slug}` }}
    >
      <div className='relative shrink-0'>
        <AppMark
          iconUrl={app.icon_url}
          name={app.name}
          size='xl'
          className='@sm:size-9 @sm:text-sm'
          testId={`launcher-app-card-mark-${app.slug}`}
        />
        <UnreadBadge
          count={unread}
          testId={`home-app-card-unread-${app.slug}`}
          className='absolute -top-1.5 -right-1.5 h-5 min-w-5 px-1.5 shadow-sm'
        />
      </div>
      <div className='flex @sm:w-auto w-full min-w-0 @sm:flex-1 flex-col'>
        <h3
          // The full name on hover, wherever it is cut.
          title={app.name}
          // Hyphenated where the browser can, never broken mid-word without one.
          className='line-clamp-2 @sm:truncate hyphens-auto font-medium @sm:text-sm text-card-foreground text-xs'
        >
          {/* Opens in this tab, as the launcher's card does (§ 4.1). */}
          <a
            href={app.url}
            data-testid={`launcher-app-card-${app.slug}`}
            className='after:absolute after:inset-0 after:content-[""] focus-visible:outline-none'
            onPointerEnter={warm}
            onFocus={warm}
          >
            {app.name}
            {unread > 0 && <span className='sr-only'>, {unread} unread</span>}
          </a>
        </h3>
        {banner ? (
          <p
            title={banner}
            className='@sm:flex hidden min-w-0 items-center gap-1.5 text-foreground text-xs'
            data-testid={`home-app-card-banner-${app.slug}`}
          >
            {/* "New", not "wrong": a routine notice is not an alarm. */}
            <span aria-hidden className='size-1.5 shrink-0 rounded-full bg-primary' />
            <span className='truncate'>{banner}</span>
          </p>
        ) : (
          freshness && (
            <p
              className='@sm:block hidden truncate text-muted-foreground text-xs'
              data-testid={`launcher-app-card-freshness-${app.slug}`}
            >
              {freshness}
            </p>
          )
        )}
      </div>
      {onManageAccess && (
        // `z-10` lifts these above the name's link, which would swallow the click.
        // With a mouse it shows on hover or focus, so seven of them are not the
        // grid's loudest repeat, and it floats over the tile's end rather than
        // holding a gap that cuts every name short; on touch, where nothing
        // hovers, it always shows, in the row.
        <div className='pointer-fine:absolute relative pointer-fine:inset-y-0 pointer-fine:right-2 z-10 @sm:flex hidden shrink-0 items-center gap-1 pointer-fine:bg-card pointer-fine:pl-2 pointer-fine:opacity-0 transition-opacity pointer-fine:group-focus-within:opacity-100 pointer-fine:group-hover:opacity-100'>
          {/* Only the restricted state gets a badge; "whole org" is the default. */}
          {app.visibility === "members" && <AppAccessBadge visibility={app.visibility} />}
          <Button
            variant='ghost'
            size='icon'
            className='size-8 text-muted-foreground'
            aria-label={`Manage access to ${app.name}`}
            title='Access'
            onClick={() => onManageAccess(app)}
            data-testid={`home-app-card-access-${app.slug}`}
          >
            <UserCog className='size-4' aria-hidden />
          </Button>
        </div>
      )}
    </div>
  );
}
