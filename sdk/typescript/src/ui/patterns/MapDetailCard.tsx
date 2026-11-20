// Started from oxy-hq/reference-app@81ee8aa `src/components/MapDetailCard.tsx` (INVENTORY section 4:
// 0 copies; Command Center's `src/ui/Panel.tsx:9` cites it). Kept: the detail VIEW of MapSidebar's
// panel, a fixed header and a scrolling body drawn with the same parts as the overview it
// replaces (`mapPanel.tsx`), so the two views are one object; the header's leading slot is a back
// arrow, not a close X, because this view returns the panel to the list the item was picked from;
// sections `spacingLg` apart; the actions at the foot of the body over a rule, sharing its width.
// New, as a layout with slots: the title, the meta line, the sections (MapSection, MapTile, Stat)
// and the actions are the caller's, as are every word and figure; the back arrow is a text
// button (decision 47) named by `backLabel`, which MapSidebar finds to give focus to; `framed`
// draws the panel's frame for a detail shown on its own (the gallery's specimen).
//
// No map library: the map, its pins and what a pin opens are the screen's.

import type { ReactNode } from "react";
import { Button } from "../actions";
import { cx } from "../cx";
import { ArrowLeftIcon } from "./icons";
import { MapPanelBody, MapPanelHeader } from "./mapPanel";

export interface MapDetailCardProps {
  /** The item's name: "Oakland, CA". */
  title: ReactNode;
  /** The line under it: an id, a rank, a code. */
  meta?: ReactNode;
  /** Back to the overview this item was opened from. */
  onBack: () => void;
  /** The back arrow's name. Default "Back". Name the list: "Back to candidates". */
  backLabel?: string;
  /** The sections: MapSection, each holding MapTiles, a StatGrid, a list. */
  children: ReactNode;
  /** The actions at the foot of the body, each taking an equal share of its width. */
  actions?: ReactNode;
  /** The body's name while it scrolls. Default the title, when it is text, else "Details". */
  label?: string;
  /** Draw the panel's own frame (an edge on the card surface). MapSidebar's detail has none:
   *  the sidebar is the frame. */
  framed?: boolean;
  className?: string;
}

export function MapDetailCard({
  title,
  meta,
  onBack,
  backLabel = "Back",
  children,
  actions,
  label,
  framed = false,
  className
}: MapDetailCardProps) {
  return (
    <div
      className={cx("oxy-ui-map-view", framed && "oxy-ui-map-panel", className)}
      data-oxy-ui-map-detail=''
    >
      <MapPanelHeader
        lead={
          <Button
            variant='text'
            iconOnly
            aria-label={backLabel}
            data-oxy-ui-map-back=''
            className='oxy-ui-map-panel__back'
            icon={<ArrowLeftIcon className='oxy-ui-map-panel__back-icon' />}
            onClick={onBack}
          />
        }
        title={title}
        meta={meta}
      />
      <MapPanelBody label={label ?? (typeof title === "string" ? title : "Details")}>
        {children}
        {actions != null && <div className='oxy-ui-map-panel__actions'>{actions}</div>}
      </MapPanelBody>
    </div>
  );
}
