import { PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { Fragment, type ReactNode, useState } from "react";
import { useLocalStorage } from "usehooks-ts";
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

/** Shown once the rail is open. Closed, the text is at zero width and
 *  invisible; the row's `aria-label` names it either way. */
const revealClasses =
  "opacity-0 transition-opacity duration-(--duration-fast) group-data-expanded/rail:opacity-100";

/** Collapsed, a row is only its mark, so its name is a hint to its right on
 *  hover or focus. Open, the name is on the row and needs no hint. */
const railHint = (label: string, expanded: boolean) =>
  expanded ? undefined : { content: label, side: "right" as const, sideOffset: 8 };

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
function RailEntry({ item, expanded }: { item: RailItem; expanded: boolean }) {
  if (!item.badge) return <RailTile item={item} expanded={expanded} />;
  return (
    <div className='relative'>
      <RailTile item={item} expanded={expanded} />
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

function RailTile({ item, expanded }: { item: RailItem; expanded: boolean }) {
  const hint = railHint(item.label, expanded);
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
      <Button asChild variant='ghost' className={itemClasses(item.active)} tooltip={hint}>
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
      tooltip={hint}
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

/** Where the rail remembers how it was left — per browser, as ChatGPT and
 *  Gemini do, so it opens the way the person last had it. */
export const RAIL_EXPANDED_KEY = "oxy.shell-rail.expanded";

/** The expand / collapse control, in the head's 32px cell. Collapsed, the cell
 *  shows `mark` (the workspace's) and turns into the toggle's icon while the
 *  pointer is on the rail or the toggle has keyboard focus — the cue that the
 *  mark is the way back in. The swap only changes the icon; nothing opens until
 *  the toggle is pressed. One element in both states, so keyboard focus stays
 *  on it as the rail opens and closes. */
function RailToggle({
  expanded,
  onToggle,
  mark
}: {
  expanded: boolean;
  onToggle: () => void;
  mark?: ReactNode;
}) {
  const label = expanded ? "Collapse sidebar" : "Expand sidebar";
  const Icon = expanded ? PanelLeftClose : PanelLeftOpen;
  return (
    <Button
      variant='ghost'
      onClick={onToggle}
      aria-label={label}
      aria-expanded={expanded}
      data-testid='shell-rail-toggle'
      tooltip={{ content: label, side: "right", sideOffset: 8 }}
      className='group/toggle size-8 shrink-0 p-0'
    >
      {mark ? (
        <>
          <span className='flex group-hover/rail:hidden group-focus-visible/toggle:hidden'>
            {mark}
          </span>
          <Icon className='hidden size-4 group-hover/rail:block group-focus-visible/toggle:block' />
        </>
      ) : (
        <Icon className='size-4' />
      )}
    </Button>
  );
}

/** The icon rail — sole chrome of the workspace shell. Mirrors the IDE rail's
 *  visual vocabulary so the two read as one system.
 *
 *  It opens and closes like ChatGPT's and Gemini's sidebars, and only from its
 *  toggle: closed it is 48px of marks, each named in a hint; open it is 224px
 *  with each entry's name beside its mark. It sits in the page's flow, so the
 *  page beside it narrows as it opens and widens as it closes — nothing is drawn
 *  over the page, and hovering or tabbing into it opens nothing. The marks, the
 *  badges and the account menu stay where they were through the change. The
 *  state is remembered per browser (`RAIL_EXPANDED_KEY`).
 *
 *  Head: closed, one cell — the workspace mark, which is the toggle. Open, the
 *  mark, the workspace's `title` and the toggle at the far end. Below `md` the
 *  rail gives way to the nav sheet, which is labelled already.
 *
 *  The SDK's port (`sdk/typescript/src/shell/ShellRail.tsx`, the rail inside
 *  custom apps, from `@oxy-hq/sdk` 2.33.0) behaves the same and shares the key,
 *  so on Oxygen's own origin the rail stays as it was left between HQ and apps.
 *
 *  `groups` are the conceptual sections (HQ · Apps · Intelligence), rendered
 *  top-down with a hairline divider between them. `footerItems` pin to the
 *  bottom of the scroll area as a distinct "system" zone (Oxygen Nexus), above the
 *  `bottom` account slot (workspace switch · user menu). */
export function ShellRail({
  top,
  title,
  groups,
  footerItems,
  bottom
}: {
  top?: ReactNode;
  /** The workspace's name, beside `top` in the head once the rail is open. */
  title?: string;
  groups: RailItem[][];
  footerItems?: RailItem[];
  bottom?: ReactNode;
}) {
  const [expanded, setExpanded] = useLocalStorage(RAIL_EXPANDED_KEY, false);
  const toggle = () => setExpanded((open) => !open);
  return (
    <nav
      aria-label='Oxygen'
      data-testid='shell-rail'
      data-expanded={expanded || undefined}
      className={cn(
        "group/rail flex h-full shrink-0 flex-col overflow-hidden border-r bg-sidebar-background",
        // The width eases in the page's flow, so the page beside it moves with
        // it. Opening arrives (ease-out); closing departs, faster (ease-in).
        "transition-[width] motion-reduce:transition-none",
        expanded
          ? "w-56 duration-(--duration-base) ease-out"
          : "w-12 duration-(--duration-fast) ease-in"
      )}
    >
      {/* Fixed h-12 so the head matches the top bar's height exactly — the two
          bottom borders then line up into one continuous divider and the mark
          anchors the top-left corner. */}
      {/* The toggle keeps its place among the head's children in both states (the
          `false` holes hold it), so it is one element and keeps keyboard focus. */}
      <div className='flex h-12 shrink-0 items-center gap-2 border-b px-2'>
        {expanded && top}
        {expanded && (
          <span
            data-testid='shell-rail-title'
            className='min-w-0 flex-1 truncate font-medium text-sm'
          >
            {title}
          </span>
        )}
        <RailToggle expanded={expanded} onToggle={toggle} mark={expanded ? undefined : top} />
      </div>
      <div className='flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto overflow-x-hidden p-2'>
        {groups.map((group, i) => (
          <Fragment key={group[0]?.key ?? i}>
            {i > 0 && <RailDivider />}
            {group.map((item) => (
              <RailEntry key={item.key} item={item} expanded={expanded} />
            ))}
          </Fragment>
        ))}
        {footerItems && footerItems.length > 0 && (
          <div className='mt-auto flex flex-col gap-1 pt-2'>
            <RailDivider />
            {footerItems.map((item) => (
              <RailEntry key={item.key} item={item} expanded={expanded} />
            ))}
          </div>
        )}
      </div>
      {bottom && (
        <div className='flex shrink-0 flex-col items-start gap-1 border-t p-2'>{bottom}</div>
      )}
    </nav>
  );
}
