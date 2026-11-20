// Inline glyphs for `@oxy-hq/sdk/ui`, so the SDK carries no icon library: Phosphor Icons, weight
// regular, the design system's set (never the fill weight). Paths copied unchanged from
// @phosphor-icons/react 2.1.10 (MIT, (c) Phosphor Icons, https://phosphoricons.com), as the phone
// kit's are (`src/shell/phone/icons.tsx`).
//
// Two sizes, set by what the icon sits in (`ui.iconInline` 14px, `ui.iconControl` 16px, the
// default). Decorative: every control that shows one also has a name, so each glyph is
// `aria-hidden`. A group that needs another glyph adds it in its own folder with `Glyph`.

import type { ReactNode } from "react";
import { cx } from "./cx";

export interface IconProps {
  /** Replaces the size class: `ui.iconInline` inside text, a tag or a chip. Default: 16px. */
  className?: string;
}

/** The frame every glyph draws in: Phosphor's 256 grid, filled with the text colour. */
export function Glyph({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <svg
      className={cx("oxy-ui-icon", className ?? "oxy-ui-icon-control")}
      viewBox='0 0 256 256'
      fill='currentColor'
      aria-hidden='true'
      focusable='false'
    >
      {children}
    </svg>
  );
}

/** Phosphor CaretRight: a caret on a callout, a row that opens. */
export function CaretRightIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M181.66,133.66l-80,80a8,8,0,0,1-11.32-11.32L164.69,128,90.34,53.66a8,8,0,0,1,11.32-11.32l80,80A8,8,0,0,1,181.66,133.66Z' />
    </Glyph>
  );
}

/** Phosphor CaretDown: something that opens below. */
export function CaretDownIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M213.66,101.66l-80,80a8,8,0,0,1-11.32,0l-80-80A8,8,0,0,1,53.66,90.34L128,164.69l74.34-74.35a8,8,0,0,1,11.32,11.32Z' />
    </Glyph>
  );
}

/** Phosphor ArrowSquareOut: a link that leaves the app. */
export function ArrowSquareOutIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M224,104a8,8,0,0,1-16,0V59.32l-66.33,66.34a8,8,0,0,1-11.32-11.32L196.68,48H152a8,8,0,0,1,0-16h64a8,8,0,0,1,8,8Zm-40,24a8,8,0,0,0-8,8v72H48V80h72a8,8,0,0,0,0-16H48A16,16,0,0,0,32,80V208a16,16,0,0,0,16,16H176a16,16,0,0,0,16-16V136A8,8,0,0,0,184,128Z' />
    </Glyph>
  );
}

/** Phosphor Plus: add. */
export function PlusIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M224,128a8,8,0,0,1-8,8H136v80a8,8,0,0,1-16,0V136H40a8,8,0,0,1,0-16h80V40a8,8,0,0,1,16,0v80h80A8,8,0,0,1,224,128Z' />
    </Glyph>
  );
}

/** Phosphor Check: done, chosen. */
export function CheckIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M229.66,77.66l-128,128a8,8,0,0,1-11.32,0l-56-56a8,8,0,0,1,11.32-11.32L96,188.69,218.34,66.34a8,8,0,0,1,11.32,11.32Z' />
    </Glyph>
  );
}

/** Phosphor X: dismiss, clear. */
export function XIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M205.66,194.34a8,8,0,0,1-11.32,11.32L128,139.31,61.66,205.66a8,8,0,0,1-11.32-11.32L116.69,128,50.34,61.66A8,8,0,0,1,61.66,50.34L128,116.69l66.34-66.35a8,8,0,0,1,11.32,11.32L139.31,128Z' />
    </Glyph>
  );
}

/** Phosphor CircleNotch: work in progress (a busy button). base.css turns it only where motion is
 *  welcome. */
export function CircleNotchIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M232,128a104,104,0,0,1-208,0c0-41,23.81-78.36,60.66-95.27a8,8,0,0,1,6.68,14.54C60.15,61.59,40,93.27,40,128a88,88,0,0,0,176,0c0-34.73-20.15-66.41-51.34-80.73a8,8,0,0,1,6.68-14.54C208.19,49.64,232,87,232,128Z' />
    </Glyph>
  );
}
