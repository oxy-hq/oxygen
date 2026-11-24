// Where a card's name meets its access gear. Copied from oxy-hq/reference-app
// `src/views/parts/useCutLines.ts` (branch design/home-match-oxygen), itself the
// design preview's cut lines.
//
// The name keeps its box and its lines at all times, so a hover never moves,
// narrows or rewraps it. With a mouse the gear is laid over the row's right end
// on hover; each line of the name that runs under it is cut after its last whole
// character, and a "…" follows. This reads where those cuts fall, from the text's
// own glyph boxes, and reads them again whenever the card's width, the name or
// the kind of pointer changes. A card with no gear (someone who cannot manage
// access) has no cuts.

import { useLayoutEffect, useRef, useState } from "react";

/** The "…"'s room when it cannot be measured, and the gap kept before the gear. */
const ELLIPSIS_FALLBACK_PX = 13;
const GAP_PX = 4;

/** One cut: a line's top and height, and where the cut starts, all from the card's corner. */
export type CutLine = { top: number; height: number; left: number };

/** A visual line of the name, in viewport coordinates. */
type Line = { top: number; bottom: number; end: number; keep: number };

/** The width of a "…" in the element's own font; the fallback where there is no canvas. */
function ellipsisWidth(el: Element): number {
  try {
    const ctx = document.createElement("canvas").getContext("2d");
    if (!ctx) return ELLIPSIS_FALLBACK_PX;
    ctx.font = getComputedStyle(el).font;
    const width = ctx.measureText("…").width;
    return width > 0 ? Math.ceil(width) : ELLIPSIS_FALLBACK_PX;
  } catch {
    return ELLIPSIS_FALLBACK_PX;
  }
}

/** `withGear`: whether the card draws the gear; the cuts are read again when it appears. */
/**
 * Adds one glyph's box to the line it sits on (matched by its midline: glyphs from a
 * fallback font sit a pixel or two off), widening the line, and records how far its
 * text may run before the "…" would reach `limit`. A space is never the last thing
 * before the "…".
 */
function addGlyph(lines: Line[], box: DOMRect, char: string, keepUntil: number) {
  const mid = (box.top + box.bottom) / 2;
  let line = lines.find((l) => mid >= l.top && mid <= l.bottom);
  if (!line) {
    line = { top: box.top, bottom: box.bottom, end: 0, keep: box.left };
    lines.push(line);
  }
  line.top = Math.min(line.top, box.top);
  line.bottom = Math.max(line.bottom, box.bottom);
  line.end = Math.max(line.end, box.right);
  if (char.trim() && box.right <= keepUntil) line.keep = Math.max(line.keep, box.right);
}

/**
 * The text's lines as drawn, glyph by glyph, by code point so a character outside the
 * BMP (an emoji) is measured whole. Glyphs under `clipBottom` (hidden by a clamp,
 * though they still have boxes) are left out.
 */
function glyphLines(node: Text, clipBottom: number, keepUntil: number): Line[] {
  const range = document.createRange();
  const lines: Line[] = [];
  let i = 0;
  for (const char of node.data) {
    const start = i;
    i += char.length;
    range.setStart(node, start);
    range.setEnd(node, i);
    const box = range.getBoundingClientRect();
    if (box.width === 0 || (box.top + box.bottom) / 2 > clipBottom) continue;
    addGlyph(lines, box, char, keepUntil);
  }
  return lines;
}

export function useCutLines(name: string, withGear = true) {
  const cardRef = useRef<HTMLDivElement>(null);
  const textRef = useRef<HTMLSpanElement>(null);
  const gearRef = useRef<HTMLButtonElement>(null);
  const [cuts, setCuts] = useState<CutLine[]>([]);
  // `name` is not read here, but it is the text this measures: a new name is new glyphs.
  // biome-ignore lint/correctness/useExhaustiveDependencies: re-measure when the name changes
  useLayoutEffect(() => {
    const card = cardRef.current;
    const gear = gearRef.current;
    const text = textRef.current;
    if (!card || !gear || !text || !withGear) {
      setCuts([]);
      return;
    }
    const measure = () => {
      const node = text.firstChild;
      if (!(node instanceof Text)) return setCuts([]);
      const cardBox = card.getBoundingClientRect();
      const gearBox = gear.getBoundingClientRect();
      // On touch the gear sits in the row beside the name, so no line reaches it.
      const limit = gearBox.left - GAP_PX;
      const ellipsis = ellipsisWidth(text);
      // A clamp hides any later line, but its glyphs still have boxes.
      const clipBottom = (text.parentElement ?? text).getBoundingClientRect().bottom;
      // Offsets inside the card's border, which its absolute children are placed from.
      const originX = cardBox.left + card.clientLeft;
      const originY = cardBox.top + card.clientTop;
      const lines = glyphLines(node, clipBottom, limit - ellipsis);
      setCuts(
        lines
          // Cut only a line the gear actually lies over: across, and down.
          .filter((l) => l.end > limit && l.bottom > gearBox.top && l.top < gearBox.bottom)
          .map((l) => ({
            top: Math.round(l.top - originY),
            height: l.bottom - l.top,
            left: l.keep - originX
          }))
      );
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(card);
    // Plugging in a mouse moves the gear out of the row without resizing the card.
    const pointer = window.matchMedia?.("(pointer: fine)");
    pointer?.addEventListener?.("change", measure);
    return () => {
      observer.disconnect();
      pointer?.removeEventListener?.("change", measure);
    };
  }, [name, withGear]);
  return { cardRef, textRef, gearRef, cuts };
}
