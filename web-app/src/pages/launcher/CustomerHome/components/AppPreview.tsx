import { Gear } from "@phosphor-icons/react";
import { AppArt } from "@/components/apps/AppArt";
import { AppMark } from "@/components/apps/AppMark";
import { focusRing, textBase, textSm, tile } from "@/components/ui/reference/tokens";
import { cn } from "@/libs/shadcn/utils";
import { prefetchApp } from "@/libs/utils/prefetchApp";
import type { CustomAppSummary } from "@/types/apps";
import { appFreshnessLine } from "../../components/appFreshness";
import { useCutLines } from "./useCutLines";

/** A press answers in place: the card shrinks a hair while its link is held. */
const PRESS =
  "transition-[border-color,background-color,scale] has-[a:active]:scale-98 motion-reduce:has-[a:active]:scale-100";
/** The art rests slightly zoomed in and eases out to the whole frame on hover. */
const ZOOM_OUT =
  "origin-top-left scale-104 transition-transform duration-500 ease-out group-hover:scale-100 group-focus-within:scale-100 motion-reduce:scale-100 motion-reduce:transition-none";
/** The name's link stretches over the whole card, so the card is one target. */
const STRETCH = 'after:absolute after:inset-0 after:content-[""] focus-visible:outline-none';

/**
 * One app on the home: its window preview (the manifest's `art`, `AppArt`'s
 * letter tile without one), then its mark, its name and when it was last
 * published, and, for org owners and admins, the gear that opens who can open
 * it. The card lifts its edge on hover, its art eases out from a slight zoom,
 * and it shrinks a hair while pressed. It opens the app in this tab, as every
 * other way into an app does, warmed on hover and focus.
 *
 * Transcribed from oxy-hq/reference-app `src/views/parts/HomeApps.tsx`
 * (`AppPreview`, branch design/home-match-oxygen) onto the org's apps.
 */
export function AppPreview({
  app,
  onManage
}: {
  app: CustomAppSummary;
  /** Opens the access dialog. Passed only for org owners and admins. */
  onManage?: (app: CustomAppSummary) => void;
}) {
  const { cardRef, textRef, gearRef, cuts } = useCutLines(app.name, !!onManage);
  const restricted = app.visibility === "members";
  const updated = appFreshnessLine(app);
  const warm = () => prefetchApp(app.url);
  return (
    <div
      ref={cardRef}
      className={cn(
        tile,
        "group relative flex min-w-0 flex-col overflow-hidden bg-card hover:border-border-strong",
        // The one focus ring, on the card while something in it has keyboard focus.
        "has-[:focus-visible]:ring-(--focus-ring-color) has-[:focus-visible]:ring-[1.5px]",
        PRESS
      )}
      data-testid={`home-app-card-${app.slug}`}
      // What grows into the app's launch screen when the card is pressed.
      data-app-launch-origin
      style={{ viewTransitionName: `home-app-${app.slug}` }}
    >
      {/* Anchored top-left, where an app's header and title are; the crop takes the rest. */}
      <span className='block overflow-hidden border-border-subtle border-b'>
        <AppArt
          artUrl={app.art_url}
          name={app.name}
          eager
          className={cn("aspect-video h-auto border-0 object-left-top", ZOOM_OUT)}
        />
      </span>
      <span className='flex min-w-0 items-center gap-3 p-3'>
        <AppMark
          iconUrl={app.icon_url}
          name={app.name}
          size='xl'
          className='size-10 border-0 text-sm'
          testId={`launcher-app-card-mark-${app.slug}`}
        />
        {/* The name has the row's whole width and keeps its line at all times: with a mouse
            the gear is laid over the card and takes no room, so a hover never moves the
            name. Where it runs under the gear it is cut with a "…" (below). */}
        <span className='flex min-w-0 flex-1 flex-col'>
          <a
            href={app.url}
            title={app.name}
            data-testid={`launcher-app-card-${app.slug}`}
            onPointerEnter={warm}
            onFocus={warm}
            className={cn("block w-full min-w-0 text-left text-foreground", textBase, STRETCH)}
          >
            {/* One line, a name too long for it ending in "…", so every card in a row is
                the same height (Mars, 23 Nov: "Warehouse Management" wrapped and stood
                taller than its row). The whole name is the link's title. */}
            <span className='block truncate'>
              <span ref={textRef}>{app.name}</span>
            </span>
          </a>
          {updated && (
            <span
              className={cn("truncate text-disabled-foreground", textSm)}
              data-testid={`launcher-app-card-freshness-${app.slug}`}
            >
              {updated}
            </span>
          )}
        </span>
        {/* The access gear. With a mouse it shows on hover or focus, laid over the card's
            corner so while hidden it takes no room from the name; on touch it always shows,
            in the row. Above the name's stretched link, which would otherwise take the press. */}
        {onManage && (
          <button
            ref={gearRef}
            type='button'
            onClick={() => onManage(app)}
            aria-label={`Manage access to ${app.name}${restricted ? ", restricted" : ""}`}
            title={restricted ? "Access · restricted" : "Access"}
            data-testid={`home-app-card-access-${app.slug}`}
            className={cn(
              "relative z-10 -mr-1 flex size-9 shrink-0 items-center justify-center text-muted-foreground transition-opacity hover:bg-row-selected hover:text-foreground",
              "pointer-fine:absolute pointer-fine:right-2 pointer-fine:bottom-3.5 pointer-fine:mr-0",
              "pointer-fine:opacity-0 pointer-fine:group-focus-within:opacity-100 pointer-fine:group-hover:opacity-100",
              focusRing
            )}
          >
            {/* 20px, a step above the 16px control icon: it floats over a card and has to be found. */}
            <Gear className='size-5 shrink-0' aria-hidden />
          </button>
        )}
      </span>
      {/* The cut layer: per line of the name that runs under the gear, the card's own ground
          from the line's last whole character to the card's edge, led by a "…". It shows only
          with the gear, under it, and lets every press through. */}
      {cuts.map((cut) => (
        <span
          key={cut.top}
          aria-hidden
          className={cn(
            "pointer-events-none absolute right-0 hidden bg-card text-foreground",
            "pointer-fine:group-focus-within:block pointer-fine:group-hover:block",
            textBase
          )}
          style={{ top: cut.top, height: cut.height, left: cut.left }}
        >
          …
        </span>
      ))}
    </div>
  );
}
