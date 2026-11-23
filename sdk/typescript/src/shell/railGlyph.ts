import { useEffect, useState } from "react";

/**
 * THE copy of the mark cutter. `web-app/src/components/Shell/railGlyph.ts` is a
 * byte-identical copy (the web-app does not depend on this package), held to it by
 * `web-app/src/components/Shell/railGlyph.drift.test.ts`: change this file, copy it.
 *
 * The rail draws each app as a bare mark in the rail's own grey, the way
 * the reference design does (a filled Phosphor glyph in `text-subtle-foreground`,
 * the current one in `text-foreground`). Here an app's mark is its manifest icon:
 * a picture in its own colours, often on a square of its own. So the mark is cut
 * out of the picture: the icon is drawn once onto a canvas, its ground (the
 * colour its edges agree on, when they agree) is keyed out with any frame round
 * it, and what is left is cropped to its own box and kept as an alpha mask the
 * row's text colour shows through.
 */

/** The canvas the icon is read on, square. */
export const GLYPH_PX = 96;

/** Where the ground is sampled: the edges' midpoints and the corners' diagonals,
 *  inset far enough to clear a rounded tile's corners and a frame's stroke. */
const SAMPLE_INSET = 0.12;
/** A tile's outer band, cleared so a frame round the tile does not become part of the mark. */
const FRAME = 0.08;
/** Colour distance (RGB, 0–441) under which a pixel is ground, and the ramp past it. */
const KEY_FLOOR = 16;
const KEY_RAMP = 64;
/** Alpha at or under which a pixel counts as empty when cropping. */
const EMPTY = 8;
/** The design's app mark (the SDK's `AppMark`): one glyph whose own box takes the middle
 *  56% of its square. A mark cut from such a tile keeps that box, so it is drawn the size
 *  the design draws a glyph, its own margin included. */
const MARK_GLYPH_BOX = 0.56;
/** What a filled Phosphor glyph covers of its own box (about 16 to 240 of 256): a mark cut
 *  from an icon on nothing is given the same margin, so it is drawn the same size. */
const GLYPH_FILL = 0.875;

/** A colour and its alpha. A tile may be translucent: the design's own mark is, in the
 *  dark (blue-600 at 30%, the page showing through). */
type Rgba = [number, number, number, number];

const distance = (a: Rgba, b: Rgba) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);

const ramp = (over: number) => Math.min(1, Math.max(0, (over - KEY_FLOOR) / KEY_RAMP));

/** The colour (and alpha) the icon's edges agree on, when it sits on a tile; null when it does not. */
export function groundOf(px: Uint8ClampedArray, size: number): Rgba | null {
  const lo = Math.round(size * SAMPLE_INSET);
  const hi = size - 1 - lo;
  const mid = Math.round(size / 2);
  const points: Array<[number, number]> = [
    [mid, lo],
    [mid, hi],
    [lo, mid],
    [hi, mid],
    [lo, lo],
    [hi, lo],
    [lo, hi],
    [hi, hi]
  ];
  const painted: Rgba[] = [];
  for (const [x, y] of points) {
    const i = (y * size + x) * 4;
    if (px[i + 3] >= 24) painted.push([px[i], px[i + 1], px[i + 2], px[i + 3]]);
  }
  // Six of eight: a glyph that reaches one or two sample points does not hide the ground.
  if (painted.length < 6) return null;
  const mean = [0, 1, 2, 3].map(
    (c) => painted.reduce((sum, rgba) => sum + rgba[c], 0) / painted.length
  ) as Rgba;
  const agreeing = painted.filter(
    (rgba) => distance(rgba, mean) < 24 && Math.abs(rgba[3] - mean[3]) < 24
  );
  return agreeing.length >= 6 ? mean : null;
}

/** The mark's alpha (one byte a pixel, the canvas's size), the square it is drawn in (`x`,
 *  `y`, `side`, which may reach past the canvas: that is margin) and the painted extent. */
export type Cut = {
  alpha: Uint8ClampedArray;
  x: number;
  y: number;
  side: number;
  painted: { x: number; y: number; w: number; h: number };
};

/** The square a mark is drawn in, its margin included: on the design's tile the glyph's own
 *  box, else the painted extent with a Phosphor glyph's margin round it. */
function markSquare(painted: Cut["painted"], size: number, tiled: boolean) {
  const lo = Math.round((size * (1 - MARK_GLYPH_BOX)) / 2);
  const side = Math.round(size * MARK_GLYPH_BOX);
  const inGlyphBox =
    painted.x >= lo - 1 &&
    painted.y >= lo - 1 &&
    painted.x + painted.w <= lo + side + 1 &&
    painted.y + painted.h <= lo + side + 1;
  if (tiled && inGlyphBox) return { x: lo, y: lo, side };
  const padded = Math.round(Math.max(painted.w, painted.h) / GLYPH_FILL);
  return {
    x: Math.round(painted.x + painted.w / 2 - padded / 2),
    y: Math.round(painted.y + painted.h / 2 - padded / 2),
    side: padded
  };
}

/**
 * The mark's alpha and the square it is drawn in; null when nothing is left (an icon
 * that is all ground, or a blank one).
 */
export function cutGlyph(px: Uint8ClampedArray, size: number): Cut | null {
  const ground = groundOf(px, size);
  const frame = ground ? Math.round(size * FRAME) : 0;
  const alpha = new Uint8ClampedArray(size * size);
  let [x0, y0, x1, y1] = [size, size, -1, -1];
  for (let y = frame; y < size - frame; y++) {
    for (let x = frame; x < size - frame; x++) {
      const i = y * size + x;
      const a = px[i * 4 + 3];
      // Kept for a colour of its own, or for being more opaque than a translucent
      // ground (a glyph laid on it). A tile's soft edge is the ground's colour, fainter: gone.
      const keep = ground
        ? Math.max(
            ramp(distance([px[i * 4], px[i * 4 + 1], px[i * 4 + 2], a], ground)),
            ramp(a - ground[3])
          )
        : 1;
      const out = Math.round(a * keep);
      alpha[i] = out;
      if (out > EMPTY) {
        x0 = Math.min(x0, x);
        y0 = Math.min(y0, y);
        x1 = Math.max(x1, x);
        y1 = Math.max(y1, y);
      }
    }
  }
  if (x1 < 0) return null;
  const painted = { x: x0, y: y0, w: x1 - x0 + 1, h: y1 - y0 + 1 };
  return { alpha, ...markSquare(painted, size, !!ground), painted };
}

/** The cut's square as a PNG mask (margin beyond the canvas left empty); null without a
 *  2D canvas. Drawn at the full size of its cell, as a glyph's own box is. */
function toMaskUrl(cut: Cut, size: number): string | null {
  const { side } = cut;
  const canvas = document.createElement("canvas");
  canvas.width = side;
  canvas.height = side;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const out = ctx.createImageData(side, side);
  for (let y = 0; y < side; y++) {
    const sy = y + cut.y;
    if (sy < 0 || sy >= size) continue;
    for (let x = 0; x < side; x++) {
      const sx = x + cut.x;
      if (sx < 0 || sx >= size) continue;
      out.data[(y * side + x) * 4 + 3] = cut.alpha[sy * size + sx];
    }
  }
  ctx.putImageData(out, 0, 0);
  return canvas.toDataURL("image/png");
}

/** Draws the icon on a square canvas, fitted, and cuts its mark out. */
function maskFromImage(img: HTMLImageElement): string | null {
  const canvas = document.createElement("canvas");
  canvas.width = GLYPH_PX;
  canvas.height = GLYPH_PX;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) return null;
  // An SVG without its own size reports 0 × 0: it is drawn square.
  const w = img.naturalWidth || GLYPH_PX;
  const h = img.naturalHeight || GLYPH_PX;
  const scale = GLYPH_PX / Math.max(w, h);
  ctx.drawImage(img, (GLYPH_PX - w * scale) / 2, (GLYPH_PX - h * scale) / 2, w * scale, h * scale);
  const cut = cutGlyph(ctx.getImageData(0, 0, GLYPH_PX, GLYPH_PX).data, GLYPH_PX);
  return cut ? toMaskUrl(cut, GLYPH_PX) : null;
}

/** One cut per icon URL for the page's life: the rail redraws on every route. */
const cuts = new Map<string, Promise<string | null>>();

function cutFromUrl(url: string): Promise<string | null> {
  const known = cuts.get(url);
  if (known) return known;
  const cut = new Promise<string | null>((resolve) => {
    const img = new Image();
    img.decoding = "async";
    img.onload = () => {
      try {
        resolve(maskFromImage(img));
      } catch {
        // A canvas the picture tainted (an icon served from another origin) cannot be read.
        resolve(null);
      }
    };
    img.onerror = () => resolve(null);
    img.src = url;
  });
  cuts.set(url, cut);
  return cut;
}

/**
 * The icon's mark as a mask URL: `undefined` while it is being cut, `null` when
 * there is none to be had (the icon failed to load or is all ground).
 */
export function useRailGlyph(url: string | undefined): string | null | undefined {
  const [mask, setMask] = useState<{ url: string; mask: string | null } | null>(null);
  useEffect(() => {
    if (!url) return;
    let live = true;
    void cutFromUrl(url).then((m) => {
      if (live) setMask({ url, mask: m });
    });
    return () => {
      live = false;
    };
  }, [url]);
  if (!url) return null;
  return mask?.url === url ? mask.mask : undefined;
}
