// The feedback group's own glyph, drawn with the shared `Glyph` frame: Phosphor regular, the path
// copied unchanged from @phosphor-icons/react 2.1.10 (MIT, (c) Phosphor Icons), as `../icons.tsx`'s
// are. Decorative: the control that shows it carries the name.

import { Glyph, type IconProps } from "../icons";

/** Phosphor ArrowClockwise: read again (Try again). */
export function ArrowClockwiseIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M240,56v48a8,8,0,0,1-8,8H184a8,8,0,0,1,0-16H211.4L184.81,71.64l-.25-.24a80,80,0,1,0-1.67,114.78,8,8,0,0,1,11,11.63A95.44,95.44,0,0,1,128,224h-1.32A96,96,0,1,1,195.75,60L224,85.8V56a8,8,0,1,1,16,0Z' />
    </Glyph>
  );
}
