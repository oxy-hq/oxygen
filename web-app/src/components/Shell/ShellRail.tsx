import { Fragment, type ReactNode, type RefObject, useEffect, useRef, useState } from "react";
import { UnreadBadge } from "@/components/Notifications/UnreadBadge";
import { Button } from "@/components/ui/shadcn/button";
import { cn } from "@/libs/shadcn/utils";

/** One rail entry. Provide `imageUrl` (custom app mark, falls back to
 *  `letter` on load error), `icon` (lucide node), or `letter`.
 *  `href` renders a real anchor (custom apps live outside the SPA);
 *  `onSelect` renders a button (SPA nav, wired by the caller).
 *  Router-free on purpose: the SDK will mount this inside custom apps. */
export type RailItem = {
  key: string;
  /** The entry's name: its accessible name, and the text the rail shows beside
   *  the mark once it opens. */
  label: string;
  testId: string;
  active?: boolean;
  icon?: ReactNode;
  letter?: string;
  imageUrl?: string;
  href?: string;
  onSelect?: () => void;
  /** Unread notifications, drawn as a dock badge on the mark. */
  badge?: number;
};

const itemClasses = (active?: boolean) =>
  cn(
    // A row, not a square: 32px while the rail is closed, the rail's width once
    // it opens, so the selected chip and the hover fill take in the label. The
    // transparent border on every row keeps the mark where the selected one's
    // outline would otherwise push it.
    "h-8 w-full justify-start gap-2 border border-transparent p-0",
    // Active: the app's monochrome "selected" recipe — an opaque --muted chip
    // outlined in --border-strong (same pair the settings cards use). Both
    // tokens step away from the rail in either theme (#e4e4e7 on #fafafa,
    // #27272a on #000), and the outline is what separates selected from a
    // plain hover fill. Deliberately no accent bar: an inset box-shadow bows
    // around the tile's rounded corners and reads as a parenthesis, and a
    // colored one would be the only brand hue in otherwise neutral chrome.
    // `hover:` is pinned so the ghost Button's hover fill can't wash it out.
    active
      ? "border-border-strong bg-muted text-sidebar-accent-foreground hover:bg-muted"
      : "opacity-60 hover:opacity-100"
  );

/** Shown once the rail is open: on hover (after the same short wait the rail
 *  itself takes) or while it is tabbed into (`useTabbedIn`). Closed, the text is
 *  at zero width and invisible; the row's `aria-label` names it either way. */
const revealClasses =
  "opacity-0 transition-opacity duration-(--duration-fast) group-hover/rail:opacity-100 group-hover/rail:delay-(--duration-base) group-data-tabbed-in/rail:opacity-100";

/** An item's mark: the app image (falling back to its letter on a load
 *  error), else its icon, else its letter. Shared with `ShellNavSheet`. */
export function RailItemIcon({ item }: { item: RailItem }) {
  const [imageFailed, setImageFailed] = useState(false);
  if (item.imageUrl && !imageFailed) {
    return (
      <img
        src={item.imageUrl}
        alt=''
        onError={() => setImageFailed(true)}
        className='h-5 w-5 rounded-sm object-contain'
      />
    );
  }
  return item.icon ?? <span className='font-semibold text-primary text-xs'>{item.letter}</span>;
}

/** "Name, 3 unread" for a screen reader; the badge itself is drawn, not read. */
function itemLabel(item: RailItem): string {
  return item.badge ? `${item.label}, ${item.badge} unread` : item.label;
}

/** The row plus its dock badge. The badge sits outside the row, so the idle
 *  row's fade does not dim it, and is pinned to the mark rather than the row,
 *  so it stays put while the rail opens. */
function RailEntry({ item }: { item: RailItem }) {
  if (!item.badge) return <RailTile item={item} />;
  return (
    <div className='relative'>
      <RailTile item={item} />
      <span className='pointer-events-none absolute inset-y-0 left-0 w-8'>
        <UnreadBadge
          count={item.badge}
          testId={`${item.testId}-unread`}
          className='absolute -top-1 -right-1'
        />
      </span>
    </div>
  );
}

function RailTile({ item }: { item: RailItem }) {
  const content = (
    <>
      {/* 30px: the row's 32px less its 1px border each side, so the mark sits
          where the closed rail's square tile always put it. */}
      <span className='flex size-7.5 shrink-0 items-center justify-center'>
        <RailItemIcon item={item} />
      </span>
      <span className={cn("min-w-0 flex-1 truncate pr-2 text-left", revealClasses)}>
        {item.label}
      </span>
    </>
  );
  if (item.href) {
    return (
      <Button asChild variant='ghost' className={itemClasses(item.active)}>
        <a href={item.href} data-testid={item.testId} aria-label={itemLabel(item)}>
          {content}
        </a>
      </Button>
    );
  }
  return (
    <Button
      variant='ghost'
      onClick={item.onSelect}
      data-testid={item.testId}
      aria-label={itemLabel(item)}
      className={itemClasses(item.active)}
    >
      {content}
    </Button>
  );
}

/** A hairline divider between conceptual rail groups: 20px under the marks
 *  while closed, the row's width once open. */
function RailDivider() {
  return <div className='mx-1.5 my-1 h-px shrink-0 bg-sidebar-border' />;
}

/** Whether Tab has brought keyboard focus into `ref`'s element: true from a Tab
 *  (or Shift+Tab) that lands inside it until focus leaves it.
 *
 *  Not `:focus-visible` inside the rail, which also matches focus a menu or
 *  dialog hands back to its trigger when it closes: a mouse user who opened the
 *  account menu and pressed Escape got the visible focus back on the avatar, so
 *  the rail stayed open over the page after the pointer had left. A key's
 *  `keyup` fires where focus landed, so Tab into the rail is seen here and the
 *  Escape that closed a menu is not. Native listeners, not React's: React
 *  bubbles a portalled menu's events through the rail, the DOM does not. */
function useTabbedIn<T extends HTMLElement>(ref: RefObject<T | null>): boolean {
  const [tabbedIn, setTabbedIn] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === "Tab") setTabbedIn(true);
    };
    const onFocusOut = (e: FocusEvent) => {
      if (!(e.relatedTarget instanceof Node && el.contains(e.relatedTarget))) setTabbedIn(false);
    };
    el.addEventListener("keyup", onKeyUp);
    el.addEventListener("focusout", onFocusOut);
    return () => {
      el.removeEventListener("keyup", onKeyUp);
      el.removeEventListener("focusout", onFocusOut);
    };
  }, [ref]);
  return tabbedIn;
}

/** The icon rail — sole chrome of the workspace shell. Mirrors the IDE rail's
 *  visual vocabulary so the two read as one system.
 *
 *  Closed it is 48px of marks. Hovered (after a short wait, so crossing it on
 *  the way to the page does not open it) or tabbed into, it opens
 *  to 224px with each entry's name beside its mark, OVER the page: the 48px
 *  slot it holds in the layout never changes, so nothing beside it reflows.
 *  The marks, the workspace tile and the account menu stay exactly where they
 *  were, so a menu opened from the rail keeps its anchor as the rail closes.
 *  Hover-capable pointers only (Tailwind's `hover:` is `(hover: hover)`): a
 *  touch screen never sees it open, and below `md` the rail gives way to the
 *  nav sheet, which is labelled already.
 *
 *  The SDK's port (`sdk/typescript/src/shell/ShellRail.tsx`, the rail inside
 *  custom apps) does not open yet: it still names its entries in tooltips.
 *
 *  `groups` are the conceptual sections (HQ · Apps · Intelligence), rendered
 *  top-down with a hairline divider between them. `footerItems` pin to the
 *  bottom of the scroll area as a distinct "system" zone (Oxygen Nexus), above the
 *  `bottom` account slot (workspace switch · user menu). */
export function ShellRail({
  top,
  groups,
  footerItems,
  bottom
}: {
  top?: ReactNode;
  groups: RailItem[][];
  footerItems?: RailItem[];
  bottom?: ReactNode;
}) {
  const panelRef = useRef<HTMLDivElement>(null);
  const tabbedIn = useTabbedIn(panelRef);
  return (
    <div data-testid='shell-rail' className='relative h-full w-12 shrink-0'>
      <div
        ref={panelRef}
        data-testid='shell-rail-panel'
        data-tabbed-in={tabbedIn || undefined}
        className={cn(
          "group/rail absolute inset-y-0 left-0 z-40 flex w-12 flex-col overflow-hidden border-r bg-sidebar-background",
          // Closing is a departure: faster, easing in. Opening arrives easing out,
          // after a wait long enough that a pointer passing through does nothing.
          "transition-[width,box-shadow] duration-(--duration-fast) ease-in motion-reduce:duration-0",
          // `motion-reduce:hover:` too: the hovered duration below outranks a bare
          // `motion-reduce:duration-0` (`:hover` adds specificity), and the rail
          // then slid open under reduced motion. The wait stays; a wait is not motion.
          "hover:w-56 hover:shadow-lg hover:delay-(--duration-base) hover:duration-(--duration-base) hover:ease-out motion-reduce:hover:duration-0",
          "data-tabbed-in:w-56 data-tabbed-in:shadow-lg"
        )}
      >
        {/* Fixed h-12 so the logo cell matches the top bar's height exactly —
            the two bottom borders then line up into one continuous divider and
            the logo anchors the top-left corner. */}
        {top && <div className='flex h-12 shrink-0 items-center border-b px-2'>{top}</div>}
        <div className='flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto overflow-x-hidden p-2'>
          {groups.map((group, i) => (
            <Fragment key={group[0]?.key ?? i}>
              {i > 0 && <RailDivider />}
              {group.map((item) => (
                <RailEntry key={item.key} item={item} />
              ))}
            </Fragment>
          ))}
          {footerItems && footerItems.length > 0 && (
            <div className='mt-auto flex flex-col gap-1 pt-2'>
              <RailDivider />
              {footerItems.map((item) => (
                <RailEntry key={item.key} item={item} />
              ))}
            </div>
          )}
        </div>
        {bottom && (
          <div className='flex shrink-0 flex-col items-start gap-1 border-t p-2'>{bottom}</div>
        )}
      </div>
    </div>
  );
}
