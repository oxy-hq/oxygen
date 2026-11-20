// The forms group's own glyphs, drawn with `Glyph` (../icons): Phosphor Icons, weight regular,
// paths copied unchanged from @phosphor-icons/react 2.1.10 `dist/defs/<Name>.es.js` (MIT,
// (c) Phosphor Icons, https://phosphoricons.com). Decorative: the control carries the name.
// Not exported from the entry; a second group that needs one moves it to ../icons.

import { Glyph, type IconProps } from "../icons";

/** Phosphor MagnifyingGlass: a search field. */
export function MagnifyingGlassIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M229.66,218.34l-50.07-50.06a88.11,88.11,0,1,0-11.31,11.31l50.06,50.07a8,8,0,0,0,11.32-11.32ZM40,112a72,72,0,1,1,72,72A72.08,72.08,0,0,1,40,112Z' />
    </Glyph>
  );
}

/** Phosphor FileText: an attachment with no picture to show. */
export function FileTextIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M213.66,82.34l-56-56A8,8,0,0,0,152,24H56A16,16,0,0,0,40,40V216a16,16,0,0,0,16,16H200a16,16,0,0,0,16-16V88A8,8,0,0,0,213.66,82.34ZM160,51.31,188.69,80H160ZM200,216H56V40h88V88a8,8,0,0,0,8,8h48V216Zm-32-80a8,8,0,0,1-8,8H96a8,8,0,0,1,0-16h64A8,8,0,0,1,168,136Zm0,32a8,8,0,0,1-8,8H96a8,8,0,0,1,0-16h64A8,8,0,0,1,168,168Z' />
    </Glyph>
  );
}

/** Phosphor ImageSquare: where photos and files go. */
export function ImageSquareIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M208,32H48A16,16,0,0,0,32,48V208a16,16,0,0,0,16,16H208a16,16,0,0,0,16-16V48A16,16,0,0,0,208,32ZM48,48H208v77.38l-24.69-24.7a16,16,0,0,0-22.62,0L53.37,208H48ZM208,208H76l96-96,36,36v60ZM96,120A24,24,0,1,0,72,96,24,24,0,0,0,96,120Zm0-32a8,8,0,1,1-8,8A8,8,0,0,1,96,88Z' />
    </Glyph>
  );
}

/** Phosphor ArrowClockwise: try the upload again. */
export function ArrowClockwiseIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M240,56v48a8,8,0,0,1-8,8H184a8,8,0,0,1,0-16H211.4L184.81,71.64l-.25-.24a80,80,0,1,0-1.67,114.78,8,8,0,0,1,11,11.63A95.44,95.44,0,0,1,128,224h-1.32A96,96,0,1,1,195.75,60L224,85.8V56a8,8,0,1,1,16,0Z' />
    </Glyph>
  );
}

/** Phosphor WarningCircle: a file the field refused. */
export function WarningCircleIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M128,24A104,104,0,1,0,232,128,104.11,104.11,0,0,0,128,24Zm0,192a88,88,0,1,1,88-88A88.1,88.1,0,0,1,128,216Zm-8-80V80a8,8,0,0,1,16,0v56a8,8,0,0,1-16,0Zm20,36a12,12,0,1,1-12-12A12,12,0,0,1,140,172Z' />
    </Glyph>
  );
}
