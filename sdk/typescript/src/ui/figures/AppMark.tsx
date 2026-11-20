// Started from oxy-hq/reference-app@81ee8aa `src/components/AppMark.tsx` (the only copy: the
// reference added it to its gallery on 19 Nov as its 25th component, after INVENTORY's count;
// its test is `src/lib/appMark.test.tsx`). Kept whole: one filled glyph faded from full ink at
// its top to 70% at its foot, in the glyph's own 256-unit box (`userSpaceOnUse`), on a square of
// the brand blue; three sizes (32 / 40 / 56px), the glyph 56% of the square at each; a gradient
// id of its own per mark (`useId`, made safe for `url(#…)`); `aria-hidden`, since the app's name
// always stands beside it; `APP_MARK_FADE_FLOOR`; `data-app-mark`. New: the two colours are
// theme tokens (`--app-mark-ground`, `--app-mark-ink`, contract group "App mark"), and the glyph
// is typed by its shape, not by Phosphor's `Icon` (below).
//
// An app's icon: how the platform draws an app wherever it names one, a card on the home, the
// sender of a notification, an app's own header.
//
// **The glyph is a component, typed by shape (`AppMarkGlyph`).** The SDK carries no icon
// library, so it does not import Phosphor's `Icon` type; it asks for what it calls: a component
// that takes `weight`, `color`, `size`, `className` and `children` and draws an SVG whose fill is
// `color`, with `children` inside it. A Phosphor icon is that shape, so `glyph={Package}` passes
// straight in. Without Phosphor, a five-line component does the same: an `<svg viewBox="0 0 256
// 256" fill={color}>` holding `children` and the glyph's path.
//
// **Colour** is two theme tokens, so one mark serves both modes:
//   --app-mark-ground  light: blue-100 (#d7dcff)   dark: blue-600 at 30%, so the page shows
//                      through and the square sits IN the surface rather than on it
//   --app-mark-ink     light: blue-600 (#2a40cc)   dark: blue-300 (#8696ff); the link step
//
// **The fade's floor is 70%.** The platform's preview faded to 55%, which leaves the glyph's foot
// at 2.46:1 on the light ground and 2.67:1 on dark, under the 3:1 a mark needs. At 70% the foot is
// 3.27:1 (light), 3.43:1 (dark, on a card) and 3.74:1 (dark, on the page); the top is 5.43 to
// 6.19:1 (the reference wrote 3.26 / 3.75 and 5.45-6.20, rounding its own way).
// `contrast.cases.ts` measures both ends from this constant, both modes, on a card, the page, a
// dialog and a hovered row. Not in a selected row: on dark the foot is 2.97:1 there.
//
// **The fill weight is the exception, and only here.** Everywhere else an icon is Phosphor
// regular. An app's mark is the app's identity, not a control or a status, and the platform
// draws it filled; that is the whole of the exception.
//
// No phone form of its own: the mark is the same square on a phone, at the size its row asks for.

import { type ReactNode, useId } from "react";
import { cx } from "../cx";

/** The share of the ink left at the glyph's foot. See the header for why 0.7. */
export const APP_MARK_FADE_FLOOR = 0.7;

export type AppMarkSize = "sm" | "md" | "lg";

/** What AppMark hands its glyph. A Phosphor icon takes exactly these. */
export interface AppMarkGlyphProps {
  /** Always "fill": the mark is the one filled glyph in the system. */
  weight?: "fill";
  /** The glyph's fill: the mark's own gradient, as `url(#…)`. */
  color?: string;
  /** "56%", of the square. */
  size?: string | number;
  className?: string;
  "aria-hidden"?: boolean | "true" | "false";
  /** The gradient's `<defs>`, to draw inside the glyph's `<svg>`. */
  children?: ReactNode;
}

/** A glyph component: any function component or forwardRef that draws an `<svg>` filled with
 *  `color` and holding `children` (Phosphor's `Icon` is one). */
export type AppMarkGlyph = (props: AppMarkGlyphProps) => ReactNode;

/** The square's class at each size: 32px a dense row of one line, 40px a card or a
 *  notification's sender, 56px an app's own header or a launcher grid. */
const SIZE: Record<AppMarkSize, string> = {
  sm: "oxy-ui-app-mark--sm",
  md: "oxy-ui-app-mark--md",
  lg: "oxy-ui-app-mark--lg"
};

export interface AppMarkProps {
  /** The icon that says what the app does. Drawn at the fill weight. */
  glyph: AppMarkGlyph;
  size?: AppMarkSize;
  className?: string;
}

export function AppMark({ glyph: Glyph, size = "md", className }: AppMarkProps) {
  // `useId` gives ":r0:", "«r0»" or "_r_0_" by React version; only the safe characters belong
  // inside `url(#…)`.
  const id = `app-mark-${useId().replace(/[^A-Za-z0-9_-]/g, "")}`;
  return (
    // Decorative: an app's name is always beside its mark, and says it.
    <span
      aria-hidden='true'
      data-app-mark=''
      className={cx("oxy-ui-app-mark", SIZE[size], className)}
    >
      <Glyph
        weight='fill'
        color={`url(#${id})`}
        size='56%'
        className='oxy-ui-app-mark__glyph'
        aria-hidden='true'
      >
        <defs>
          <linearGradient id={id} gradientUnits='userSpaceOnUse' x1='0' y1='0' x2='0' y2='256'>
            <stop offset='0' style={{ stopColor: "var(--app-mark-ink)" }} />
            <stop
              offset='1'
              style={{ stopColor: "var(--app-mark-ink)", stopOpacity: APP_MARK_FADE_FLOOR }}
            />
          </linearGradient>
        </defs>
      </Glyph>
    </span>
  );
}
