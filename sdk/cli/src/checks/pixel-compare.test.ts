/**
 * The pixel comparison itself, over pictures written out by hand. The same
 * function is what the browser runs (`installExpression` carries its source),
 * so what it answers here is what a real run answers for the same pixels.
 */

import { Script } from "node:vm";
import { describe, expect, it } from "vitest";
import {
  CHANGED_RATIO,
  CHANNEL_TOLERANCE,
  comparePixels,
  installExpression,
  isChanged
} from "./pixel-compare.js";

type Rgba = [number, number, number, number];
const WHITE: Rgba = [255, 255, 255, 255];
const BLACK: Rgba = [0, 0, 0, 255];

/** A `width` by `height` picture of `fill`, with the pixels `paint` names in their own colour. */
function picture(width: number, height: number, fill: Rgba, paint: Record<string, Rgba> = {}) {
  const data: number[] = [];
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) data.push(...(paint[`${x},${y}`] ?? fill));
  }
  return { data, width, height };
}

function compare(
  a: ReturnType<typeof picture>,
  b: ReturnType<typeof picture>,
  diff?: number[],
  tolerance = CHANNEL_TOLERANCE
) {
  return comparePixels(a, b, tolerance, diff);
}

describe("comparePixels", () => {
  it("finds nothing between a picture and itself", () => {
    const a = picture(4, 3, WHITE, { "1,1": BLACK });
    expect(compare(a, a)).toEqual({ width: 4, height: 3, sameSize: true, differing: 0, ratio: 0 });
  });

  it("counts each pixel that differs, as a share of the picture", () => {
    const a = picture(10, 10, WHITE);
    const b = picture(10, 10, WHITE, { "0,0": BLACK, "9,9": BLACK, "4,5": [255, 0, 0, 255] });
    expect(compare(a, b)).toEqual({
      width: 10,
      height: 10,
      sameSize: true,
      differing: 3,
      ratio: 0.03
    });
  });

  it("lets a channel drift by the tolerance and no further", () => {
    const a = picture(2, 1, [100, 100, 100, 255]);
    const within = picture(2, 1, [100 + CHANNEL_TOLERANCE, 100 - CHANNEL_TOLERANCE, 100, 255]);
    const past = picture(2, 1, [100, 100, 100 + CHANNEL_TOLERANCE + 1, 255]);
    expect(compare(a, within).differing).toBe(0);
    expect(compare(a, past).differing).toBe(2);
    // With no tolerance, any drift is a difference; alpha counts like a colour.
    expect(compare(a, within, undefined, 0).differing).toBe(2);
    expect(compare(a, picture(2, 1, [100, 100, 100, 200])).differing).toBe(2);
  });

  it("compares over the larger area when the sizes differ: what one picture lacks, differs", () => {
    const short = picture(4, 2, WHITE);
    const tall = picture(4, 5, WHITE);
    expect(compare(short, tall)).toEqual({
      width: 4,
      height: 5,
      sameSize: false,
      // Three rows of four pixels that only the taller one has.
      differing: 12,
      ratio: 0.6
    });
    const wide = picture(6, 2, WHITE);
    expect(compare(short, wide)).toMatchObject({ width: 6, height: 2, differing: 4 });
  });

  it("paints the diff: the first picture faded where they agree, magenta where they do not", () => {
    const a = picture(2, 1, [0, 100, 200, 255]);
    const b = picture(2, 1, [0, 100, 200, 255], { "1,0": WHITE });
    const diff: number[] = [];
    expect(compare(a, b, diff).differing).toBe(1);
    expect(diff).toEqual([
      // Three quarters of the way to white.
      191, 216, 241, 255, 219, 39, 119, 255
    ]);
  });

  it("answers zero for two empty pictures, not a division by nothing", () => {
    expect(compare(picture(0, 0, WHITE), picture(0, 0, WHITE)).ratio).toBe(0);
  });
});

describe("isChanged", () => {
  it("is a share of differing pixels past one in a thousand, or a different size", () => {
    expect(isChanged({ ratio: 0, sameSize: true })).toBe(false);
    expect(isChanged({ ratio: CHANGED_RATIO, sameSize: true })).toBe(false);
    expect(isChanged({ ratio: 0.0011, sameSize: true })).toBe(true);
    expect(isChanged({ ratio: 0, sameSize: false })).toBe(true);
  });

  it("holds for a real pair: one changed pixel in a 40 by 40 screen is not a changed screen, two are", () => {
    const a = picture(40, 40, WHITE);
    expect(isChanged(compare(a, picture(40, 40, WHITE, { "3,3": BLACK })))).toBe(false);
    expect(isChanged(compare(a, picture(40, 40, WHITE, { "3,3": BLACK, "4,3": BLACK })))).toBe(
      true
    );
  });
});

describe("installExpression", () => {
  it("is one expression that leaves the comparison in the page, carrying nothing of this module", () => {
    const expression = installExpression();
    // Evaluated alone, as a page evaluates it: no argument, nothing called.
    const page: { __oxyComparePixels?: typeof comparePixels } = {};
    const answered: unknown = new Script(expression).runInNewContext(page);
    expect(answered).toBeUndefined();
    expect(expression).not.toMatch(/\bimport\b|\brequire\(|CHANNEL_TOLERANCE|CHANGED_RATIO/);
    // What it left is the comparison itself: it answers what this module's does.
    const a = picture(10, 10, WHITE);
    const b = picture(10, 10, WHITE, { "0,0": BLACK, "9,9": BLACK });
    const installed = page.__oxyComparePixels;
    expect(installed?.(a, b, CHANNEL_TOLERANCE)).toEqual(compare(a, b));
  });
});
