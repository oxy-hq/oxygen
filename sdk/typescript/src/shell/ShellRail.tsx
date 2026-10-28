// The 48px icon rail — sole chrome of the workspace shell. Ported from
// web-app/src/components/Shell/ShellRail.tsx, which was written router-free
// for exactly this move; the web-app now consumes this component. Keep the
// two visually identical: this file is the source of truth.

import { Fragment, type ReactNode, useState } from "react";
import { cx } from "./cx";
import { ShellPortalContext, useShellPortalContainer } from "./portal";
import { ShellTooltip } from "./Tooltip";

/** One rail entry. Provide `imageUrl` (custom app mark, falls back to
 *  `letter` on load error), `icon` (any SVG node), or `letter`.
 *  `href` renders a real anchor (full-page nav — custom apps live outside
 *  the SPA); `onSelect` renders a button (SPA nav, wired by the caller). */
export type RailItem = {
  key: string;
  label: string;
  /** Hover-tooltip text; defaults to `label`. Use for a richer description
   *  while keeping `label` short for the aria-label. */
  tooltip?: string;
  testId?: string;
  active?: boolean;
  icon?: ReactNode;
  letter?: string;
  imageUrl?: string;
  href?: string;
  onSelect?: () => void;
};

/** An item's mark: the app image (falling back to its letter on a load
 *  error), else its icon, else its letter. Shared with `NavSheet`. */
export function RailItemMark({ item }: { item: RailItem }) {
  // Track the URL that failed (not a boolean) so a new imageUrl retries
  // instead of latching the letter fallback forever.
  const [failedUrl, setFailedUrl] = useState<string | null>(null);
  if (item.imageUrl && failedUrl !== item.imageUrl) {
    return (
      <img
        src={item.imageUrl}
        alt=''
        onError={() => setFailedUrl(item.imageUrl ?? null)}
        className='oxy-rail__item-img'
      />
    );
  }
  return item.icon ?? <span className='oxy-rail__item-letter'>{item.letter}</span>;
}

function RailEntry({ item }: { item: RailItem }) {
  const content = <RailItemMark item={item} />;
  const className = cx("oxy-rail__item", item.active && "oxy-rail__item--active");
  const ariaCurrent = item.active ? ("page" as const) : undefined;
  const entry = item.href ? (
    <a
      href={item.href}
      data-testid={item.testId}
      aria-label={item.label}
      aria-current={ariaCurrent}
      className={className}
    >
      {content}
    </a>
  ) : (
    <button
      type='button'
      onClick={item.onSelect}
      data-testid={item.testId}
      aria-label={item.label}
      aria-current={ariaCurrent}
      className={className}
    >
      {content}
    </button>
  );
  return <ShellTooltip content={item.tooltip ?? item.label}>{entry}</ShellTooltip>;
}

/** A hairline divider between conceptual rail groups. */
function RailDivider() {
  return <div className='oxy-rail__divider' />;
}

/** The icon rail. `groups` are the conceptual sections (HQ · Apps ·
 *  Intelligence), rendered top-down with a hairline divider between them.
 *  `footerItems` pin to the bottom of the scroll area as a distinct
 *  "system" zone (Oxygen Factory), above the `bottom` account slot
 *  (workspace switch · user menu — injected by the host app). */
export function ShellRail({
  top,
  groups,
  footerItems,
  bottom,
  className
}: {
  top?: ReactNode;
  groups: RailItem[][];
  footerItems?: RailItem[];
  bottom?: ReactNode;
  className?: string;
}) {
  const { container, setContainer } = useShellPortalContainer();
  return (
    <ShellPortalContext.Provider value={container}>
      <div
        ref={setContainer}
        data-testid='shell-rail'
        className={cx("oxy-shell-scope oxy-rail", className)}
      >
        {top && <div className='oxy-rail__top'>{top}</div>}
        <div className='oxy-rail__nav'>
          {groups.map((group, i) => (
            <Fragment key={group[0]?.key ?? i}>
              {i > 0 && <RailDivider />}
              {group.map((item) => (
                <RailEntry key={item.key} item={item} />
              ))}
            </Fragment>
          ))}
          {footerItems && footerItems.length > 0 && (
            <div className='oxy-rail__footer'>
              <RailDivider />
              {footerItems.map((item) => (
                <RailEntry key={item.key} item={item} />
              ))}
            </div>
          )}
        </div>
        {bottom && <div className='oxy-rail__bottom'>{bottom}</div>}
      </div>
    </ShellPortalContext.Provider>
  );
}
