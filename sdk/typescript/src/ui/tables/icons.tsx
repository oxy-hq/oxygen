// The tables group's own glyph: Phosphor CaretUp, regular weight, the path copied unchanged from
// @phosphor-icons/react 2.1.10 (`dist/defs/CaretUp.es.js`, the "regular" entry), drawn with the
// shared `Glyph` frame like every ui icon. CaretDown and CaretRight are the shared ones (../icons).

import { Glyph, type IconProps } from "../icons";

/** Phosphor CaretUp: a column sorted smallest first. */
export function CaretUpIcon({ className }: IconProps) {
  return (
    <Glyph className={className}>
      <path d='M213.66,165.66a8,8,0,0,1-11.32,0L128,91.31,53.66,165.66a8,8,0,0,1-11.32-11.32l80-80a8,8,0,0,1,11.32,0l80,80A8,8,0,0,1,213.66,165.66Z' />
    </Glyph>
  );
}
