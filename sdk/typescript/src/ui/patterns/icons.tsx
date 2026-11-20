// The patterns group's own glyphs, drawn with the ui `Glyph` frame: Phosphor regular, the paths
// copied unchanged from @phosphor-icons/react 2.1.10 `dist/defs/<Name>.es.js` (MIT, (c) Phosphor
// Icons). Decorative: the control beside or around each carries the name.

import { Glyph, type IconProps } from "../icons";

/** Phosphor ArrowLeft: back to the view this one was opened from (MapDetailCard). */
export function ArrowLeftIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M224,128a8,8,0,0,1-8,8H59.31l58.35,58.34a8,8,0,0,1-11.32,11.32l-72-72a8,8,0,0,1,0-11.32l72-72a8,8,0,0,1,11.32,11.32L59.31,120H216A8,8,0,0,1,224,128Z' />
    </Glyph>
  );
}

/** Phosphor ArrowRight: a connector, from one place to another (CoverageRebalanceList). */
export function ArrowRightIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M221.66,133.66l-72,72a8,8,0,0,1-11.32-11.32L196.69,136H40a8,8,0,0,1,0-16H196.69L138.34,61.66a8,8,0,0,1,11.32-11.32l72,72A8,8,0,0,1,221.66,133.66Z' />
    </Glyph>
  );
}

/** Phosphor ArrowCounterClockwise: undo every applied move (CoverageRebalanceList's reset). */
export function ArrowCounterClockwiseIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M224,128a96,96,0,0,1-94.71,96H128A95.38,95.38,0,0,1,62.1,197.8a8,8,0,0,1,11-11.63A80,80,0,1,0,71.43,71.39a3.07,3.07,0,0,1-.26.25L44.59,96H72a8,8,0,0,1,0,16H24a8,8,0,0,1-8-8V56a8,8,0,0,1,16,0V85.8L60.25,60A96,96,0,0,1,224,128Z' />
    </Glyph>
  );
}
