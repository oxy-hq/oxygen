// Started from oxy-hq/reference-app@3caad1f `src/chrome/Subnav.tsx` (BEYOND-GALLERY row 9: its
// header records the decisions; 0 tests anywhere, so these are new). Kept: one level of
// sections, each a labelled link with its icon, `role="group"` named by the section's heading
// with no printed heading, the selected-row fill + weight + `aria-current="page"`, the column
// that stretches while its list sticks, the divider above the footer group, the sideways row
// below 768px with the current link brought into the row's view (never the page's). Changed:
// the sections, the footer and the current screen are props (the reference's table, its
// ThemeToggle and its `Link` are the app's); links are router-free (`href` + `onNavigate`). The
// column is 220px (13.75rem), the width the reference's code draws (`md:w-[13.75rem]`, whose
// comment says 220px holds "Sales Tax & Bookkeeping" on one line); its header's "208px" is stale.
//
// The app's own section nav, down the left. Independent of the platform rail: that rail
// switches APPS and disappears when the shell bootstrap is unavailable, this switches PAGES
// inside one app. Labelled, so a column of words beside the rail's column of glyphs reads as
// another job, not one sidebar drawn twice. A section with several screens opens onto a
// `ParentTabs` strip; there is no third level.

import { type MouseEvent, type ReactNode, useEffect, useRef } from "react";
import { cx } from "../cx";
import { ui } from "../recipes";
import { isPlainClick } from "./parentTab";

export interface SubnavItem {
  /** The value of `current` that lights this item. */
  id: string;
  /** Further values that also light it: the section's other screens and their modes. */
  also?: readonly string[];
  label: string;
  href: string;
  /** Decorative (`aria-hidden`): the label names the link. */
  icon?: ReactNode;
}

export interface SubnavSection {
  /** The group's accessible name; not printed (a section of one entry would say it twice). */
  heading: string;
  items: readonly SubnavItem[];
}

export interface SubnavProps {
  sections: readonly SubnavSection[];
  current: string;
  /** Below the divider, at the column's foot: what is not a section (a gallery, a settings
   *  page), same type and states, separated rather than demoted. */
  footer?: readonly SubnavItem[];
  /** After the footer's links, in the same group (a mode switch). */
  footerExtra?: ReactNode;
  /** A plain left click on an item, for the app's router. Absent: the links navigate natively. */
  onNavigate?: (id: string, item: SubnavItem) => void;
  /** The nav's name (default "Sections"). */
  label?: string;
  className?: string;
}

const isActive = (item: SubnavItem, current: string) =>
  item.id === current || (item.also?.includes(current) ?? false);

/**
 * Bring the current link into a sideways-scrolling row's view. Only the row moves: a no-op when
 * the row does not overflow (the desk column), and never `scrollIntoView`, which would scroll
 * the page as well.
 */
function centreActive(row: HTMLElement | null) {
  const el = row?.querySelector<HTMLElement>('[aria-current="page"]');
  if (!row || !el || row.scrollWidth <= row.clientWidth) return;
  const r = row.getBoundingClientRect();
  const e = el.getBoundingClientRect();
  row.scrollLeft += e.left - r.left - (r.width - e.width) / 2;
}

function SubnavLink({
  item,
  current,
  onNavigate
}: {
  item: SubnavItem;
  current: string;
  onNavigate: SubnavProps["onNavigate"];
}) {
  const on = isActive(item, current);
  const click = (e: MouseEvent<HTMLAnchorElement>) => {
    if (!onNavigate || !isPlainClick(e)) return;
    e.preventDefault();
    onNavigate(item.id, item);
  };
  return (
    <a
      href={item.href}
      aria-current={on ? "page" : undefined}
      className={cx(ui.sectionLink, on ? ui.sectionLinkOn : ui.sectionLinkOff)}
      onClick={click}
    >
      {item.icon}
      {item.label}
    </a>
  );
}

export function Subnav({
  sections,
  current,
  footer,
  footerExtra,
  onNavigate,
  label = "Sections",
  className
}: SubnavProps) {
  const row = useRef<HTMLDivElement>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new current screen re-centres the row.
  useEffect(() => centreActive(row.current), [current]);

  return (
    // The column STRETCHES and the list inside it sticks: a sticky nav is only as tall as its
    // items, so its edge would stop partway down a long page.
    <nav aria-label={label} className={cx("oxy-ui-subnav", className)}>
      <div ref={row} className='oxy-ui-subnav__list'>
        {sections.map((section) => (
          // biome-ignore lint/a11y/useSemanticElements: a section's links, named for a screen reader; a fieldset is a group of fields.
          <div
            key={section.heading}
            role='group'
            aria-label={section.heading}
            className='oxy-ui-subnav__group'
          >
            {section.items.map((item) => (
              <SubnavLink key={item.id} item={item} current={current} onNavigate={onNavigate} />
            ))}
          </div>
        ))}
        {(footer?.length || footerExtra) && (
          <div className='oxy-ui-subnav__group oxy-ui-subnav__footer'>
            {footer?.map((item) => (
              <SubnavLink key={item.id} item={item} current={current} onNavigate={onNavigate} />
            ))}
            {footerExtra}
          </div>
        )}
      </div>
    </nav>
  );
}
