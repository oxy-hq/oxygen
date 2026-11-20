// Started from oxy-hq/reference-app@81ee8aa `src/components/MapSidebar.tsx` (INVENTORY section 4:
// 0 copies; Site Scout's side panel, which `apps/site-scout/src/NetworkOverview.tsx:33` cites).
// Kept: one panel, two views. The overview (a header with the title and its meta line, then its
// sections) and, when an item is picked, that item in the same place (`detail`, a MapDetailCard).
// The header sits in the same place in both, so switching views changes what the panel is about
// without changing what the panel is. The overview stays mounted while the detail is open,
// hidden rather than unmounted, so going back lands on the same scroll position in the list the
// reader picked from. The panel draws itself; the caller places it (`className`). The `<aside>`
// is named for the view it shows.
//
// New: focus follows the view (the reference left it on a hidden row, then on <body>). Opening a
// detail from inside the panel puts focus on its back arrow; going back returns it to the row it
// was opened from, or to the overview's title when that row is gone. Opened from outside the
// panel (a pin on the map), focus stays where the reader is. A detail replaced by another while
// focus was in it puts focus on the new one's back arrow. And the parts are the system's tokens,
// not Site Scout's palette; the sections, rows and words are the caller's (layouts with slots).
//
// No map library: the map, its pins and the re-ranking a sort control causes are the screen's.
// No phone form of its own: a phone screen shows the map and this panel as the phone kit's sheet.

import { type ReactNode, useLayoutEffect, useRef } from "react";
import { cx } from "../cx";
import { MapPanelBody, MapPanelHeader } from "./mapPanel";

export interface MapSidebarProps {
  /** The overview's title: the market, the region. */
  title: ReactNode;
  /** The line under it: "Site Scout · 41 stores · 18 cities". */
  meta?: ReactNode;
  /** The overview's sections (MapSection). */
  children: ReactNode;
  /** The open item's view (a MapDetailCard, keyed by the item), or nothing for the overview. */
  detail?: ReactNode;
  /** The panel's name on the overview: "Bay Area overview". */
  label: string;
  /** Its name while the detail is shown: "Oakland hex detail". Default `label`. */
  detailLabel?: string;
  /** Placement: the caller positions and sizes the panel; the panel draws itself. */
  className?: string;
}

const BACK = "[data-oxy-ui-map-back]";

export function MapSidebar({
  title,
  meta,
  children,
  detail,
  label,
  detailLabel,
  className
}: MapSidebarProps) {
  const open = detail != null && detail !== false;
  const panel = useRef<HTMLElement>(null);
  const overview = useRef<HTMLDivElement>(null);
  const overviewTitle = useRef<HTMLHeadingElement>(null);
  const opener = useRef<HTMLElement | null>(null);
  const wasOpen = useRef(open);
  // Focus is in the panel, or was until the element holding it went away (a removed element
  // fires no focusout); cleared only when focus moves to something outside.
  const focusIn = useRef(false);

  useLayoutEffect(() => {
    const active = document.activeElement as HTMLElement | null;
    const lost = !active || active === document.body;
    const inside = !lost && active !== null && (panel.current?.contains(active) ?? false);
    const ours = inside || (lost && focusIn.current);
    const toBack = () => panel.current?.querySelector<HTMLElement>(BACK)?.focus();
    if (open && !wasOpen.current) {
      if (inside && overview.current?.contains(active)) opener.current = active;
      if (ours) toBack();
    } else if (!open && wasOpen.current) {
      if (ours) {
        const row = opener.current;
        const back = row?.isConnected && overview.current?.contains(row) ? row : null;
        (back ?? overviewTitle.current)?.focus();
      }
      opener.current = null;
    } else if (open && lost && focusIn.current) {
      // The detail was replaced by another one's while focus was in it.
      toBack();
    }
    wasOpen.current = open;
  });

  return (
    <aside
      ref={panel}
      aria-label={open ? (detailLabel ?? label) : label}
      className={cx("oxy-ui-map-panel", className)}
      onFocus={() => {
        focusIn.current = true;
      }}
      onBlur={(event) => {
        const next = event.relatedTarget as Node | null;
        if (next && !event.currentTarget.contains(next)) focusIn.current = false;
      }}
    >
      {/* Hidden, not unmounted: going back lands on the same scroll position. */}
      <div ref={overview} className='oxy-ui-map-view' hidden={open}>
        <MapPanelHeader title={title} meta={meta} titleRef={overviewTitle} />
        <MapPanelBody label={label}>{children}</MapPanelBody>
      </div>
      {open && detail}
    </aside>
  );
}
