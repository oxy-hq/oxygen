// The actions group's own glyph, drawn with the ui `Glyph` frame: Phosphor regular, the path
// copied unchanged from @phosphor-icons/react 2.1.10 `dist/defs/Copy.es.js` (MIT, (c) Phosphor
// Icons). Decorative: the button carries the name.

import { Glyph, type IconProps } from "../icons";

/** Phosphor Copy: put this on the clipboard. */
export function CopyIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M216,32H88a8,8,0,0,0-8,8V80H40a8,8,0,0,0-8,8V216a8,8,0,0,0,8,8H168a8,8,0,0,0,8-8V176h40a8,8,0,0,0,8-8V40A8,8,0,0,0,216,32ZM160,208H48V96H160Zm48-48H176V88a8,8,0,0,0-8-8H96V48H208Z' />
    </Glyph>
  );
}
