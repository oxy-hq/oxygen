// The parts MapSidebar and MapDetailCard share, so the two views are one object: the panel's
// header (an optional leading control, a 24px record title over a tabular meta line), its section
// (a caps eyebrow with an optional count on the right, the rows, an optional footnote over a
// hairline; or a disclosure that opens on demand), and its tile (a boxed item: a reason, a ranked
// row, a figure). Started from oxy-hq/reference-app@81ee8aa `src/lib/siteScoutColors.ts:216-249`
// (`ssPanel`, `ssPanelHeader`, `ssPanelBody`, `ssSection*`, `ssTile`) and the disclosure in
// `src/components/MapDetailCard.tsx` (`SignalBreakdown`). New: they read the system's tokens, not
// Site Scout's `--ss-*` palette (which stays in that app, CONVENTIONS section 4).

import { type ComponentPropsWithRef, type ReactNode, useId, useState } from "react";
import { cx } from "../cx";
import { useScrollFocusable } from "../focus";
import { CaretDownIcon, CaretRightIcon } from "../icons";
import { ui } from "../recipes";

export interface MapPanelHeaderProps {
  /** The leading slot: MapDetailCard's back arrow. */
  lead?: ReactNode;
  title: ReactNode;
  meta?: ReactNode;
  /** Focus lands on the title when the view it heads is shown and nothing else should take it. */
  titleRef?: ComponentPropsWithRef<"h2">["ref"];
}

/** The header both views open with. It does not scroll: it is how a reader knows which view
 *  they are in while the body moves under it. */
export function MapPanelHeader({ lead, title, meta, titleRef }: MapPanelHeaderProps) {
  return (
    <header className='oxy-ui-map-panel__header'>
      {lead}
      <div className='oxy-ui-map-panel__heading'>
        <h2 ref={titleRef} tabIndex={-1} className={cx(ui.text.xl, "oxy-ui-map-panel__title")}>
          {title}
        </h2>
        {meta != null && (
          <p className={cx(ui.note, ui.tabularNums, "oxy-ui-map-panel__meta")}>{meta}</p>
        )}
      </div>
    </header>
  );
}

export interface MapPanelBodyProps {
  /** The body's name while it scrolls (a named region, as DataSheet's box is). */
  label: string;
  children: ReactNode;
}

/** The scrolling body, sections `spacingLg` apart. A Tab stop and a named region only while it
 *  overflows (`useScrollFocusable`), so a body of text alone can still be scrolled by keyboard. */
export function MapPanelBody({ label, children }: MapPanelBodyProps) {
  const box = useScrollFocusable<HTMLDivElement>();
  const region = box.scrolls ? { role: "region", "aria-label": label } : {};
  return (
    <div
      {...region}
      ref={box.ref}
      tabIndex={box.tabIndex}
      className={cx("oxy-ui-map-panel__body", ui.focusRingInset)}
    >
      {children}
    </div>
  );
}

export interface MapSectionProps {
  /** The caps eyebrow: "Top candidates". */
  title: ReactNode;
  /** A count or a hint on the eyebrow's right: "25 ranked". */
  meta?: ReactNode;
  /** The footnote closing the section, over a hairline: "sorted by Composite · high first". */
  foot?: ReactNode;
  /** Folded until asked for: the eyebrow is a button, `meta` is replaced by the show / hide word. */
  collapsible?: boolean;
  /** A collapsible section's state at first. Default shut. */
  defaultOpen?: boolean;
  /** The words on a shut and an open disclosure. Default "show all" / "hide". */
  showLabel?: ReactNode;
  hideLabel?: ReactNode;
  children?: ReactNode;
  className?: string;
}

/** One section of a map panel: an eyebrow, then its rows, `spacingSm` apart. Sections sit
 *  `spacingLg` apart in the body, the read-one-record rhythm. */
export function MapSection({
  title,
  meta,
  foot,
  collapsible = false,
  defaultOpen = false,
  showLabel = "show all",
  hideLabel = "hide",
  children,
  className
}: MapSectionProps) {
  const [open, setOpen] = useState(defaultOpen);
  const body = useId();
  const shown = !collapsible || open;
  return (
    <section className={cx("oxy-ui-map-section", className)}>
      {collapsible ? (
        // The disclosure pattern: the heading holds the button, so the section keeps its place in
        // the outline and the button keeps phrasing content only.
        <h3 className='oxy-ui-map-section__heading'>
          <button
            type='button'
            aria-expanded={open}
            aria-controls={open ? body : undefined}
            onClick={() => setOpen((o) => !o)}
            className={cx("oxy-ui-map-section__toggle", ui.focusRing)}
          >
            {open ? (
              <CaretDownIcon className={cx(ui.iconInline, "oxy-ui-map-section__caret")} />
            ) : (
              <CaretRightIcon className={cx(ui.iconInline, "oxy-ui-map-section__caret")} />
            )}
            <span className={cx(ui.eyebrow, "oxy-ui-map-section__title")}>{title}</span>
            <span className={cx(ui.note, ui.tabularNums)}>{open ? hideLabel : showLabel}</span>
          </button>
        </h3>
      ) : (
        <div className='oxy-ui-map-section__head'>
          <h3 className={cx(ui.eyebrow, "oxy-ui-map-section__name")}>{title}</h3>
          {meta != null && <span className={cx(ui.note, ui.tabularNums)}>{meta}</span>}
        </div>
      )}
      {shown && (
        <div id={body} className='oxy-ui-map-section__body'>
          {children}
        </div>
      )}
      {shown && foot != null && (
        <p className={cx(ui.note, ui.tabularNums, "oxy-ui-map-section__foot")}>{foot}</p>
      )}
    </section>
  );
}

export interface MapTileProps {
  children: ReactNode;
  /** Makes the tile a button: a ranked row that opens its item. */
  onClick?: () => void;
  /** A pressed tile (the open item, the chosen sort) takes the selected segment's fill. */
  pressed?: boolean;
  /** `row`: items side by side (a glyph, then lines); `stack` (default): lines under each other. */
  layout?: "row" | "stack";
  className?: string;
}

/** A boxed item inside a section: the field surface on a subtle edge, lighter than the panel, so
 *  it reads as a thing on it. A ranked row here and a reason in the detail are one object at two
 *  zoom levels. */
export function MapTile({ children, onClick, pressed, layout = "stack", className }: MapTileProps) {
  const cls = cx(ui.tile, "oxy-ui-map-tile", layout === "row" && "oxy-ui-map-tile--row", className);
  if (!onClick) return <div className={cls}>{children}</div>;
  return (
    <button
      type='button'
      onClick={onClick}
      aria-pressed={pressed ?? false}
      className={cx(cls, "oxy-ui-map-tile--action", ui.focusRing)}
    >
      {children}
    </button>
  );
}
