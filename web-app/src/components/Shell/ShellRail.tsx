import { Fragment, type ReactNode, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { cn } from "@/libs/shadcn/utils";

/** One rail entry. Provide `imageUrl` (custom app mark, falls back to
 *  `letter` on load error), `icon` (lucide node), or `letter`.
 *  `href` renders a real anchor (custom apps live outside the SPA);
 *  `onSelect` renders a button (SPA nav, wired by the caller).
 *  Router-free on purpose: the SDK will mount this inside custom apps. */
export type RailItem = {
  key: string;
  label: string;
  /** Hover-tooltip text; defaults to `label`. Use for a richer description
   *  while keeping `label` short for the aria-label. */
  tooltip?: string;
  testId: string;
  active?: boolean;
  icon?: ReactNode;
  letter?: string;
  imageUrl?: string;
  href?: string;
  onSelect?: () => void;
};

const itemClasses = (active?: boolean) =>
  cn(
    "flex h-8 w-8 items-center justify-center rounded-md",
    // Active: the app's monochrome "selected" recipe — an opaque --muted chip
    // outlined in --border-strong (same pair the settings cards use). Both
    // tokens step away from the rail in either theme (#e4e4e7 on #fafafa,
    // #27272a on #000), and the outline is what separates selected from a
    // plain hover fill. Deliberately no accent bar: an inset box-shadow bows
    // around the tile's rounded corners and reads as a parenthesis, and a
    // colored one would be the only brand hue in otherwise neutral chrome.
    // `hover:` is pinned so the ghost Button's hover fill can't wash it out.
    active
      ? "border border-border-strong bg-muted text-sidebar-accent-foreground hover:bg-muted"
      : "opacity-60 hover:opacity-100"
  );

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

function RailEntry({ item }: { item: RailItem }) {
  const content = <RailItemIcon item={item} />;
  if (item.href) {
    return (
      <Button
        asChild
        variant='ghost'
        size='icon'
        tooltip={{ content: item.tooltip ?? item.label, side: "right" }}
        className={itemClasses(item.active)}
      >
        <a href={item.href} data-testid={item.testId} aria-label={item.label}>
          {content}
        </a>
      </Button>
    );
  }
  return (
    <Button
      variant='ghost'
      size='icon'
      onClick={item.onSelect}
      data-testid={item.testId}
      aria-label={item.label}
      tooltip={{ content: item.label, side: "right" }}
      className={itemClasses(item.active)}
    >
      {content}
    </Button>
  );
}

/** A hairline divider between conceptual rail groups. */
function RailDivider() {
  return <div className='my-1 h-px w-5 bg-sidebar-border' />;
}

/** The 48px icon rail — sole chrome of the workspace shell. Mirrors the
 *  IDE rail's visual vocabulary so the two read as one system.
 *
 *  `groups` are the conceptual sections (HQ · Apps · Intelligence), rendered
 *  top-down with a hairline divider between them. `footerItems` pin to the
 *  bottom of the scroll area as a distinct "system" zone (Oxygen Factory),
 *  above the `bottom` account slot (workspace switch · user menu). */
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
  return (
    <div
      data-testid='shell-rail'
      className='flex h-full w-12 shrink-0 flex-col border-r bg-sidebar-background'
    >
      {/* Fixed h-12 so the logo cell matches the top bar's height exactly —
          the two bottom borders then line up into one continuous divider and
          the logo anchors the top-left corner. */}
      {top && (
        <div className='flex h-12 shrink-0 flex-col items-center justify-center border-b px-1'>
          {top}
        </div>
      )}
      <div className='flex min-h-0 flex-1 flex-col items-center gap-1 overflow-y-auto px-1 py-2'>
        {groups.map((group, i) => (
          <Fragment key={group[0]?.key ?? i}>
            {i > 0 && <RailDivider />}
            {group.map((item) => (
              <RailEntry key={item.key} item={item} />
            ))}
          </Fragment>
        ))}
        {footerItems && footerItems.length > 0 && (
          <div className='mt-auto flex flex-col items-center gap-1 pt-2'>
            <RailDivider />
            {footerItems.map((item) => (
              <RailEntry key={item.key} item={item} />
            ))}
          </div>
        )}
      </div>
      {bottom && (
        <div className='flex shrink-0 flex-col items-center gap-1 border-t px-1 py-2'>{bottom}</div>
      )}
    </div>
  );
}
