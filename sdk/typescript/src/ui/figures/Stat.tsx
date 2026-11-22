// Started from oxy-hq/reference-app@3caad1f `src/components/Stat.tsx` (INVENTORY section 2: the
// reference is the fullest of six copies, 171 lines against 20-100; Watchlist's is 20). Kept
// whole: the props (`label`, `value`, `sub`, `icon`, `valueColor`, `subColor`, `tip`, `boxed`,
// `className`), `StatGrid`'s auto-fit row, `useTippedMin`, and the reference's own phone seam:
// a phone layout that provides `TapHintFrame` (containers' HoverHint, drawing the kit's
// `PhoneHint`) gets an ⓘ after the label. Not `usePhoneLayout()` (CONVENTIONS section 3): the
// frame is what the reference app's phone layout already provides, so its move is an import
// swap. `KpiTile` in other apps is this piece (BEYOND-GALLERY row 17).
//
// A labelled figure: the one for every tile that is a number with a name over it.
//
// **One recipe, every variant.** An `eyebrow` label, a 20px medium figure (`ui.figure`), a
// `note` caption and, boxed, the `tile` at `paddingSm`. The variants differ only in what they
// ADD: a box, a trend icon in front of the label, a judged colour on the figure or the caption,
// a source tip.
//
// **`valueColor` / `subColor`, not a tone union.** Staffing colours a figure, a competitor count
// colours another, a delta that is good news colours the caption: three judgements that end in a
// colour, so the caller keeps the judgement and hands over the result (a theme token is the
// usual answer: `var(--status-error)`).
//
// **The tip.** Where a figure's source needs saying: a hover hint over the whole tile on a desk,
// with a focusable ⓘ on the figure's line so a keyboard reaches it too; on a phone, an ⓘ after
// the label (a hover would leave the source out of reach by touch).
//
// **Plain `<div>`/`<span>`, not `<dt>`/`<dd>`**: the tipped tile sits inside the hint's anchor,
// and an element between a `<dl>` and its `<dt>` is invalid.
//
// 2.31.0 (the gap round):
//   - `tipMark`: a focusable ⓘ on the figure's line on a desk, so a keyboard reaches the tip the
//     hover shows (pokehouse-oxy Command Center's and Site Scout's `HintMark`, polish audit CC2:
//     8px after the figure, not at the tile's far edge). Named "About <label>"; the bubble it
//     opens describes it, as the HoverHint ⓘ form does. ON by default for every tile with a
//     `tip` (decision 89: a source only a pointer can reach is out of reach for a keyboard);
//     `tipMark={false}` leaves the hover alone. A tile without a tip, and the phone, unchanged.
//   The rest opt-in:
//   - `valueAt`: `foot` takes the figure down with its caption (Command Center's 400px rail,
//     whose caps labels wrap differently), `top` keeps it under the label (Quadrogen's phone,
//     whose two-to-a-row tiles are of unequal height).
//   - the tile takes the div's own props (`title`, `aria-*`, `id`): Supplier Ordering's tiles
//     carry a hint on the whole tile. `StatGrid` the same (Amazon P&L names its grid).
//
// 2.31.1: `OxyShell` provides the frame on a phone layout (shell `PhoneTapHintFrame`, which says
// why there and not a coarse-pointer test here). Before, an app whose phone layout did not provide
// it (every one but Poke House Bookkeeping) drew the desk ⓘ on a phone, and a tap on that ⓘ is a
// press on a control, which the desk HoverHint answers by closing: the source was out of reach by
// touch. An app's own frame still wins; the desk is unchanged.
//
// 2.32.0: `StatGrid`'s `phonePairs`, opt-in: on a phone layout two tiles to a row whatever `min`
// is, and an odd last tile across the row rather than beside a hole (Quadrogen's phone, whose
// `min="12rem"` grids fit one tile across a 358px panel). Off, the grid is the auto-fit row at
// every width, as before.

import type { ComponentPropsWithRef, CSSProperties, ReactNode } from "react";
import { usePhoneLayout } from "../../shell/phone/usePhoneLayout";
import { HoverHint, useTapHint } from "../containers";
import { cx } from "../cx";
import { InfoIcon } from "../icons";
import { ui } from "../recipes";

export interface StatProps extends Omit<ComponentPropsWithRef<"div">, "children"> {
  label: string;
  value: ReactNode;
  /** The line under the figure: "14/36 slots", "2 stores over". */
  sub?: ReactNode;
  /** In front of the label: a trend arrow. Decorative; the figure says it. */
  icon?: ReactNode;
  /** The figure's colour, when it carries a verdict. The caller owns the judgement. */
  valueColor?: string;
  /** Colours the caption instead, when the judged part is the change, not the figure. */
  subColor?: string;
  /** The figure's source: a hover hint over the tile on a desk, an ⓘ on a phone. */
  tip?: string;
  /** Draw the tile's box: a figure in a grid of them. Unboxed is a figure in running layout. */
  boxed?: boolean;
  /** On a desk, a focusable ⓘ on the figure's line opens the tip, so a keyboard reaches it.
   *  Default true wherever there is a `tip` (decision 89); `false` leaves the hover alone. */
  tipMark?: boolean;
  /** Where the figure sits in a tile taller than it: `foot` (with its caption) or `top` (under
   *  the label). Default: at the foot when there is no caption, else under the label. */
  valueAt?: "foot" | "top";
  /** Grid placement, where a tile is not one cell wide. */
  className?: string;
}

export function Stat({
  label,
  value,
  sub,
  icon,
  valueColor,
  subColor,
  tip,
  boxed = false,
  tipMark = true,
  valueAt,
  className,
  ...rest
}: StatProps) {
  // On a phone layout the tip is an ⓘ after the label, and the tile is not wrapped: the source
  // is the load-bearing part of a tipped tile, and a hover would leave it out of reach by touch.
  const tapHint = useTapHint();
  const tapTip = tip && tapHint ? tapHint(`About ${label}`, tip) : null;
  const hovered = Boolean(tip) && tapTip === null;
  const atFoot = valueAt === "foot" || (valueAt === undefined && sub == null);
  const figure = (
    <span
      className={cx(
        ui.figure,
        "oxy-ui-stat__value",
        atFoot && !(hovered && tipMark) && "oxy-ui-stat__value--end"
      )}
      style={valueColor ? { color: valueColor } : undefined}
    >
      {value}
    </span>
  );
  const body = (
    // Every tile in a row is as tall as the tallest (the grid stretches it), which is what lets
    // the figure's auto margin line the figures up. Under a desk hint the grid item is the
    // hint's anchor, so there the tile fills it (`--fill`); the reference's Safari 17 note.
    <div
      {...rest}
      className={cx(
        "oxy-ui-stat",
        hovered && "oxy-ui-stat--fill",
        boxed && cx(ui.tile, ui.paddingSm),
        className
      )}
    >
      <span
        className={cx(ui.eyebrow, (icon != null || tapTip !== null) && "oxy-ui-stat__label-row")}
      >
        {icon}
        {label}
        {tapTip}
      </span>
      {/* The figure sits at the bottom of a tile with no caption, so a row whose labels wrap
          differently still lines its figures up; with a caption the caption holds the bottom. */}
      {hovered && tipMark ? (
        <span className={cx("oxy-ui-stat__value-row", atFoot && "oxy-ui-stat__value--end")}>
          {figure}
          {/* Inside the hint's anchor, so focusing it opens the bubble, which describes it. */}
          <button
            type='button'
            aria-label={`About ${label}`}
            className={cx("oxy-ui-hover-hint__mark", ui.focusRing, "oxy-ui-stat__mark")}
          >
            <InfoIcon className={ui.iconInline} />
          </button>
        </span>
      ) : (
        figure
      )}
      {sub != null && (
        <span className={ui.note} style={subColor ? { color: subColor } : undefined}>
          {sub}
        </span>
      )}
    </div>
  );
  return hovered && tip ? <HoverHint text={tip}>{body}</HoverHint> : body;
}

export interface StatGridProps extends Omit<ComponentPropsWithRef<"div">, "children"> {
  children: ReactNode;
  /** The narrowest a tile may get before the row wraps. Default 9rem. */
  min?: string;
  className?: string;
  style?: CSSProperties;
  /** On a phone layout: two tiles to a row whatever `min` is, an odd last tile across the row.
   *  Default off (the auto-fit row at every width). */
  phonePairs?: boolean;
}

/**
 * A row of boxed figures: the one grid every set of Stats sits in. Auto-fit, not a column count:
 * as many columns as fit at `min` each, so one grid gives six across a full page, three across a
 * side panel and two on a phone, with no breakpoints chosen per caller.
 */
export function StatGrid({
  children,
  min = "9rem",
  className,
  style,
  phonePairs = false,
  ...rest
}: StatGridProps) {
  const phone = usePhoneLayout();
  const pairs = phonePairs && phone;
  return (
    <div
      {...rest}
      className={cx("oxy-ui-stat-grid", pairs && "oxy-ui-stat-grid--pairs", className)}
      style={{
        ...style,
        gridTemplateColumns: pairs
          ? "repeat(2, minmax(0, 1fr))"
          : `repeat(auto-fit, minmax(${min}, 1fr))`
      }}
    >
      {children}
    </div>
  );
}

/** What an ⓘ takes from a tile's label line: the kit's 24px hint and a 4px gap. */
const TAP_TIP_WIDTH = "1.75rem";

/**
 * `StatGrid`'s `min` for tiles that carry a `tip`: on a phone the ⓘ takes 28px of the label's
 * line, so a tile must be that much wider before the row adds a column. On a desk (no tap form),
 * `min` as given.
 */
export function useTippedMin(min: string): string {
  return useTapHint() ? `calc(${min} + ${TAP_TIP_WIDTH})` : min;
}
