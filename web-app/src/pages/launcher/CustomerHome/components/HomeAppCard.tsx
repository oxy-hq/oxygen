import { ArrowUpRight, ShieldCheck } from "lucide-react";
import { AppAccessBadge } from "@/components/appAccess/AppAccessBadge";
import { AppMark } from "@/components/apps/AppMark";
import { UnreadBadge } from "@/components/Notifications/UnreadBadge";
import { Button } from "@/components/ui/shadcn/button";
import { buttonVariants } from "@/components/ui/shadcn/utils/button-variants";
import { cn } from "@/libs/shadcn/utils";
import { prefetchApp } from "@/libs/utils/prefetchApp";
import type { CustomAppSummary } from "@/types/apps";
import type { InboxItem } from "@/types/notifications";
import { appFreshnessLine } from "../../components/appFreshness";
import { HOME_GAP } from "../layout";
import { HomeAppArt } from "./HomeAppArt";

/**
 * How big a card is, set by how many apps share the grid:
 * - `wide`: the only app. The art takes two thirds of the row and the text the last
 *   third, the same thirds the cards under it are cut on.
 * - `regular`: two to five apps. Art at 16:9 over the name.
 * - `compact`: six or more, four to a row. Art at 2:1, smaller type, and a name
 *   that wraps to two lines rather than losing its end.
 */
export type HomeAppCardSize = "wide" | "regular" | "compact";

/**
 * One app on the apps-first home: its art edge to edge across the top of the card,
 * then its mark, name and how fresh it is. Like a dock icon, it carries the app's
 * unread count in its corner, and its newest unread notification as a banner line.
 *
 * It opens the app the way the HQ launcher's `AppCard` does (a plain anchor that
 * opens the app in this tab, warmed on hover and focus) and carries the same access
 * control for owners and admins. A separate card because the home's layout and
 * the launcher's differ, and the launcher's card stays as it is.
 */
export function HomeAppCard({
  app,
  size = "regular",
  eagerArt = false,
  unread = 0,
  latest,
  onManageAccess
}: {
  app: CustomAppSummary;
  size?: HomeAppCardSize;
  /** Load the art immediately: set for the cards on screen at first paint. */
  eagerArt?: boolean;
  /** How many of this app's notifications the viewer has not read. */
  unread?: number;
  /** The newest of them, when the inbox's first page holds it. */
  latest?: InboxItem;
  /** Open the access dialog for this app. Passed only for org owners and admins. */
  onManageAccess?: (app: CustomAppSummary) => void;
}) {
  const warm = () => prefetchApp(app.url);
  const freshness = appFreshnessLine(app);
  const wide = size === "wide";
  const compact = size === "compact";
  // Null when the org's store did not answer for its text; then the badge alone.
  const banner = unread > 0 ? (latest?.title ?? latest?.body ?? null) : null;

  return (
    // A div, not an anchor, because the card holds a second action (access). The
    // name's link stretches over the whole card with `after:inset-0`; the access
    // button is its sibling, lifted above the overlay with `z-10`.
    <div
      className={cn(
        "group relative flex flex-col overflow-hidden rounded-lg border bg-card transition-colors focus-within:ring-2 focus-within:ring-ring hover:border-primary/50",
        wide && cn("sm:grid sm:grid-cols-3", HOME_GAP)
      )}
      data-testid={`home-app-card-${app.slug}`}
      data-size={size}
    >
      <UnreadBadge
        count={unread}
        testId={`home-app-card-unread-${app.slug}`}
        className='absolute top-2 right-2 z-10 h-5 min-w-5 px-1.5 shadow-sm'
      />
      <HomeAppArt
        app={app}
        eager={eagerArt}
        compact={compact}
        className={cn(
          "h-auto rounded-none border-0 border-b",
          compact ? "aspect-2/1" : "aspect-video",
          wide && "sm:col-span-2 sm:border-r sm:border-b-0"
        )}
      />
      <div
        className={cn(
          "flex flex-1 flex-col",
          compact ? "gap-2 p-3" : "gap-4 p-4",
          wide && "sm:py-6 sm:pr-6 sm:pl-2"
        )}
      >
        <div className={cn("flex flex-col gap-1", compact && "flex-1")}>
          <div className={cn("flex gap-2", compact ? "items-start" : "items-center")}>
            {/* The same glyph the rail shows for this app. */}
            <AppMark
              iconUrl={app.icon_url}
              name={app.name}
              size={wide ? "lg" : compact ? "sm" : "md"}
              // Two compact cards to a phone's row leave no room beside the name;
              // the art above already carries the app's picture.
              className={cn(compact && "max-sm:hidden")}
              testId={`launcher-app-card-mark-${app.slug}`}
            />
            <h3
              // The full name on hover, wherever it is cut.
              title={app.name}
              className={cn(
                "min-w-0 font-medium text-card-foreground",
                wide && "truncate text-lg",
                size === "regular" && "truncate text-base",
                compact && "line-clamp-2 break-words text-sm"
              )}
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
          </div>
          {banner && (
            <p
              title={banner}
              className={cn(
                "flex min-w-0 items-center gap-1.5 text-foreground",
                compact ? "text-xs" : "text-sm"
              )}
              data-testid={`home-app-card-banner-${app.slug}`}
            >
              <span aria-hidden className='size-1.5 shrink-0 rounded-full bg-destructive' />
              <span className='truncate'>{banner}</span>
            </p>
          )}
          {freshness && (
            <p
              className={cn(
                "text-muted-foreground",
                // Pinned to the bottom, so the lines in a row of compact cards line up
                // whether the names above them take one line or two.
                compact ? "mt-auto text-xs" : "text-sm"
              )}
              data-testid={`launcher-app-card-freshness-${app.slug}`}
            >
              {freshness}
            </p>
          )}
        </div>
        {(wide || onManageAccess) && (
          <div className='mt-auto flex items-center gap-2'>
            {wide && (
              // What a click on the card does, drawn as a button. Not a second link:
              // the name's link already covers the card, and this sits under it.
              <span
                aria-hidden
                className={cn(buttonVariants({ variant: "outline", size: "sm" }), "bg-card")}
              >
                Open
                <ArrowUpRight />
              </span>
            )}
            {onManageAccess && (
              // `z-10` lifts these above the name's link, which would swallow the click.
              <div className='relative z-10 ml-auto flex items-center gap-2'>
                {/* Only the restricted state gets a badge; "whole org" is the default. */}
                {app.visibility === "members" && <AppAccessBadge visibility={app.visibility} />}
                <Button
                  variant='ghost'
                  size='sm'
                  className='h-7 gap-1 px-2 text-muted-foreground text-xs'
                  onClick={() => onManageAccess(app)}
                >
                  <ShieldCheck className='size-3.5' aria-hidden />
                  Access
                </Button>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
