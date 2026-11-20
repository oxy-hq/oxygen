// The navigation group's own glyph, drawn with the ui `Glyph` frame: Phosphor regular, the path
// copied unchanged from @phosphor-icons/react 2.1.10 `dist/defs/DotsThree.es.js` (MIT, (c)
// Phosphor Icons). Decorative: the More button carries the name.

import { Glyph, type IconProps } from "../icons";

/** Phosphor DotsThree: more of the same, behind a menu. */
export function DotsThreeIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M140,128a12,12,0,1,1-12-12A12,12,0,0,1,140,128Zm56-12a12,12,0,1,0,12,12A12,12,0,0,0,196,116ZM60,116a12,12,0,1,0,12,12A12,12,0,0,0,60,116Z' />
    </Glyph>
  );
}
