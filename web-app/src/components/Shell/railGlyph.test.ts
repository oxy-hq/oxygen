import { describe, expect, it } from "vitest";
import { cutGlyph, groundOf } from "./railGlyph";

/**
 * The rail shows an app as its mark alone, cut out of the app's icon: an icon on a
 * tile loses the tile (and any frame round it), an icon on nothing keeps every pixel
 * it draws. The mark is drawn in a square that keeps a glyph's own margin: on the
 * design's tile, the glyph's box (the middle 56%); else the painted extent with a
 * Phosphor glyph's margin round it.
 */

const SIZE = 40;
type Rgba = [number, number, number, number];

/** A SIZE × SIZE picture: `paint(x, y)` gives each pixel. */
function picture(paint: (x: number, y: number) => Rgba): Uint8ClampedArray {
  const px = new Uint8ClampedArray(SIZE * SIZE * 4);
  for (let y = 0; y < SIZE; y++) {
    for (let x = 0; x < SIZE; x++) px.set(paint(x, y), (y * SIZE + x) * 4);
  }
  return px;
}

const CLEAR: Rgba = [0, 0, 0, 0];
const TILE: Rgba = [215, 220, 255, 255];
const BLUE: Rgba = [42, 64, 204, 255];
const inSquare = (x: number, y: number, from: number, to: number) =>
  x >= from && x < to && y >= from && y < to;

describe("cutting an app's mark out of its icon", () => {
  it("keys out a tile, keeping the glyph on it, cropped to the glyph", () => {
    const px = picture((x, y) => (inSquare(x, y, 14, 26) ? BLUE : TILE));
    expect(groundOf(px, SIZE)).toEqual([215, 220, 255, 255]);
    const cut = cutGlyph(px, SIZE);
    expect(cut?.painted).toEqual({ x: 14, y: 14, w: 12, h: 12 });
    // The design's glyph box: the middle 56% of the 40px square, margin and all.
    expect(cut).toMatchObject({ x: 9, y: 9, side: 22 });
    expect(cut?.alpha[20 * SIZE + 20]).toBe(255);
    expect(cut?.alpha[5 * SIZE + 20]).toBe(0);
  });

  it("keys out a translucent tile, the design's own mark in the dark, and its faint edge", () => {
    const GROUND: Rgba = [42, 64, 204, 77];
    const INK: Rgba = [134, 150, 255, 255];
    const px = picture((x, y) => {
      if (inSquare(x, y, 14, 26)) return INK;
      // A rounded corner's soft edge: the ground's colour, fainter.
      if (x + y < 3) return [42, 64, 204, 30];
      return GROUND;
    });
    expect(groundOf(px, SIZE)).toEqual([42, 64, 204, 77]);
    expect(cutGlyph(px, SIZE)?.painted).toEqual({ x: 14, y: 14, w: 12, h: 12 });
  });

  it("drops a frame drawn round the tile", () => {
    const px = picture((x, y) => {
      if (!inSquare(x, y, 1, SIZE - 1)) return BLUE;
      return inSquare(x, y, 14, 26) ? BLUE : TILE;
    });
    expect(cutGlyph(px, SIZE)?.painted).toEqual({ x: 14, y: 14, w: 12, h: 12 });
  });

  it("keeps everything an icon on nothing draws, with its own alpha", () => {
    const px = picture((x, y) => {
      if (inSquare(x, y, 2, 38) && y > 30) return [148, 163, 184, 107];
      return inSquare(x, y, 16, 24) ? BLUE : CLEAR;
    });
    expect(groundOf(px, SIZE)).toBeNull();
    const cut = cutGlyph(px, SIZE);
    expect(cut?.painted).toEqual({ x: 2, y: 16, w: 36, h: 22 });
    // Painted 36 wide: drawn in a 41px square (36 / 0.875), centred on it.
    expect(cut).toMatchObject({ side: 41 });
    expect(Math.abs((cut ? cut.x + cut.side / 2 : 0) - 20)).toBeLessThanOrEqual(1);
    expect(cut?.alpha[34 * SIZE + 4]).toBe(107);
  });

  it("finds nothing in an icon that is all tile, or blank", () => {
    expect(
      cutGlyph(
        picture(() => TILE),
        SIZE
      )
    ).toBeNull();
    expect(
      cutGlyph(
        picture(() => CLEAR),
        SIZE
      )
    ).toBeNull();
  });
});
