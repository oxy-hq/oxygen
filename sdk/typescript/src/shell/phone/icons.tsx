// Inline glyphs for the phone kit, so the SDK carries no icon library: Phosphor
// Icons, weight regular, the design system's set. Paths copied unchanged from
// @phosphor-icons/react (MIT, (c) Phosphor Icons, https://phosphoricons.com).
// All decorative: every control that shows one also has a name.

import type { ReactNode } from "react";

function Glyph({ size, children }: { size: number; children: ReactNode }) {
  return (
    <svg width={size} height={size} viewBox='0 0 256 256' fill='currentColor' aria-hidden='true'>
      {children}
    </svg>
  );
}

/** Phosphor CaretRight. */
export function ChevronRightIcon({ size = 18 }: { size?: number }) {
  return (
    <Glyph size={size}>
      <path d='M181.66,133.66l-80,80a8,8,0,0,1-11.32-11.32L164.69,128,90.34,53.66a8,8,0,0,1,11.32-11.32l80,80A8,8,0,0,1,181.66,133.66Z' />
    </Glyph>
  );
}

/** Phosphor CaretLeft. */
export function ChevronLeftIcon({ size = 22 }: { size?: number }) {
  return (
    <Glyph size={size}>
      <path d='M165.66,202.34a8,8,0,0,1-11.32,11.32l-80-80a8,8,0,0,1,0-11.32l80-80a8,8,0,0,1,11.32,11.32L91.31,128Z' />
    </Glyph>
  );
}

/** Phosphor CaretDown. */
export function ChevronDownIcon({ size = 16 }: { size?: number }) {
  return (
    <Glyph size={size}>
      <path d='M213.66,101.66l-80,80a8,8,0,0,1-11.32,0l-80-80A8,8,0,0,1,53.66,90.34L128,164.69l74.34-74.35a8,8,0,0,1,11.32,11.32Z' />
    </Glyph>
  );
}

/** Phosphor Info. */
export function InfoIcon({ size = 18 }: { size?: number }) {
  return (
    <Glyph size={size}>
      <path d='M128,24A104,104,0,1,0,232,128,104.11,104.11,0,0,0,128,24Zm0,192a88,88,0,1,1,88-88A88.1,88.1,0,0,1,128,216Zm16-40a8,8,0,0,1-8,8,16,16,0,0,1-16-16V128a8,8,0,0,1,0-16,16,16,0,0,1,16,16v40A8,8,0,0,1,144,176ZM112,84a12,12,0,1,1,12,12A12,12,0,0,1,112,84Z' />
    </Glyph>
  );
}

/** Phosphor DotsThree. */
export function MoreIcon({ size = 22 }: { size?: number }) {
  return (
    <Glyph size={size}>
      <path d='M140,128a12,12,0,1,1-12-12A12,12,0,0,1,140,128Zm56-12a12,12,0,1,0,12,12A12,12,0,0,0,196,116ZM60,116a12,12,0,1,0,12,12A12,12,0,0,0,60,116Z' />
    </Glyph>
  );
}

/** Phosphor FunnelSimple. */
export function FilterIcon({ size = 16 }: { size?: number }) {
  return (
    <Glyph size={size}>
      <path d='M200,136a8,8,0,0,1-8,8H64a8,8,0,0,1,0-16H192A8,8,0,0,1,200,136Zm32-56H24a8,8,0,0,0,0,16H232a8,8,0,0,0,0-16Zm-80,96H104a8,8,0,0,0,0,16h48a8,8,0,0,0,0-16Z' />
    </Glyph>
  );
}
