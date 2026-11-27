/**
 * Two pictures of one screen, compared pixel by pixel.
 *
 * The comparison runs INSIDE the browser the check already has open: a blank
 * page decodes both PNGs onto a canvas, reads their pixels, and paints the
 * diff picture. No image library is added to this package for it.
 *
 * {@link comparePixels} is the comparison itself, over plain pixel arrays, so
 * it is tested here without a browser. {@link compareInPage} is what the page
 * runs around it. A function reaches a page as its SOURCE, so each uses
 * nothing from outside itself: `comparePixels` is installed in the page once
 * ({@link installExpression}), and `compareInPage` finds it there.
 */

/** A pixel differs when any of its four channels differs by more than this (of 255). */
export const CHANNEL_TOLERANCE = 3;
/** A screen changed when more than this share of its pixels differ, or the pictures' sizes do. */
export const CHANGED_RATIO = 0.001;

/** What comparing two pictures found. */
export interface PixelComparison {
  /** The compared area: the wider width and the taller height of the two. */
  width: number;
  height: number;
  /** Whether both pictures have the same width and height. */
  sameSize: boolean;
  /** Pixels that differ, counting every pixel only one of the pictures has. */
  differing: number;
  /** `differing` over the compared area: 0 when they are the same picture. */
  ratio: number;
}

/** One picture's pixels: `width * height * 4` RGBA bytes. */
export interface Pixels {
  data: ArrayLike<number>;
  width: number;
  height: number;
}

/**
 * Compare picture `a` with picture `b`. When `diff` is given
 * (`width * height * 4` bytes of the compared area) it is painted: `a` faded
 * where the two agree, solid magenta where they differ.
 *
 * SENT TO THE PAGE AS ITS SOURCE: it may use nothing from outside itself, so
 * the two steps of one pixel are functions inside it.
 */
export function comparePixels(
  a: Pixels,
  b: Pixels,
  tolerance: number,
  diff?: { [index: number]: number }
): PixelComparison {
  const width = Math.max(a.width, b.width);
  const height = Math.max(a.height, b.height);

  /** Whether the pixel at (x, y) differs: only one picture has it, or a channel is too far apart. */
  const differsAt = (x: number, y: number): boolean => {
    const inBoth = x < a.width && y < a.height && x < b.width && y < b.height;
    if (!inBoth) return true;
    const ai = (y * a.width + x) * 4;
    const bi = (y * b.width + x) * 4;
    for (let c = 0; c < 4; c += 1) {
      if (Math.abs((a.data[ai + c] ?? 0) - (b.data[bi + c] ?? 0)) > tolerance) return true;
    }
    return false;
  };

  /** Paint the diff's pixel at (x, y): solid magenta where the two differ, `a` faded where they agree. */
  const paint = (into: { [index: number]: number }, x: number, y: number, differs: boolean) => {
    const di = (y * width + x) * 4;
    if (differs) {
      into[di] = 219;
      into[di + 1] = 39;
      into[di + 2] = 119;
    } else {
      // Three quarters of the way to white: the screen is still readable under the marks.
      const ai = (y * a.width + x) * 4;
      for (let c = 0; c < 3; c += 1) {
        const value = a.data[ai + c] ?? 255;
        into[di + c] = value + Math.round((255 - value) * 0.75);
      }
    }
    into[di + 3] = 255;
  };

  let differing = 0;
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      const differs = differsAt(x, y);
      if (differs) differing += 1;
      if (diff) paint(diff, x, y, differs);
    }
  }
  const area = width * height;
  return {
    width,
    height,
    sameSize: a.width === b.width && a.height === b.height,
    differing,
    ratio: area === 0 ? 0 : differing / area
  };
}

/** What the page is handed: both pictures as base64 PNG. */
export interface CompareInput {
  a: string;
  b: string;
  tolerance: number;
}

/** What the page answers: the comparison, and the diff picture (base64 PNG) when a pixel differs. */
export interface CompareAnswer extends PixelComparison {
  diff: string | null;
}

/** The little of a page the comparison touches. */
interface PageCanvas {
  width: number;
  height: number;
  getContext(kind: "2d"): {
    drawImage(image: unknown, x: number, y: number): void;
    getImageData(x: number, y: number, w: number, h: number): { data: Uint8ClampedArray };
    createImageData(w: number, h: number): { data: Uint8ClampedArray };
    putImageData(data: unknown, x: number, y: number): void;
  } | null;
  toDataURL(type: string): string;
}

/** The name {@link comparePixels} is installed under in the page that compares. */
const INSTALLED = "__oxyComparePixels";

interface PageScope {
  document: { createElement(tag: "canvas"): PageCanvas };
  createImageBitmap(blob: Blob): Promise<{ width: number; height: number }>;
  [INSTALLED]?: typeof comparePixels;
}

/**
 * The expression that installs {@link comparePixels} in a page, for the
 * comparisons that follow. Evaluated once, in the blank page that compares.
 */
export function installExpression(): string {
  return `void (globalThis.${INSTALLED} = ${comparePixels.toString()})`;
}

/**
 * Runs in the blank page: decode both PNGs, compare them with the installed
 * comparison, and paint the diff. SENT TO THE PAGE AS ITS SOURCE: it uses only
 * its argument and the page, and takes what it needs of the page before it
 * first waits.
 */
export async function compareInPage(input: CompareInput): Promise<CompareAnswer> {
  const scope = globalThis as unknown as PageScope;
  const page = scope.document;
  const bitmapOf = scope.createImageBitmap.bind(scope);
  const compare = scope.__oxyComparePixels;
  if (!compare) throw new Error("the comparison was not installed in this page");
  const pixelsOf = async (base64: string) => {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
    const bitmap = await bitmapOf(new Blob([bytes], { type: "image/png" }));
    const canvas = page.createElement("canvas");
    canvas.width = bitmap.width;
    canvas.height = bitmap.height;
    const context = canvas.getContext("2d");
    if (!context) throw new Error("the page has no 2d canvas");
    context.drawImage(bitmap, 0, 0);
    return {
      width: bitmap.width,
      height: bitmap.height,
      data: context.getImageData(0, 0, bitmap.width, bitmap.height).data
    };
  };
  const [a, b] = [await pixelsOf(input.a), await pixelsOf(input.b)];
  const canvas = page.createElement("canvas");
  canvas.width = Math.max(a.width, b.width);
  canvas.height = Math.max(a.height, b.height);
  const context = canvas.getContext("2d");
  if (!context) throw new Error("the page has no 2d canvas");
  const painted = context.createImageData(canvas.width, canvas.height);
  const found = compare(a, b, input.tolerance, painted.data);
  if (found.differing === 0) return { ...found, diff: null };
  context.putImageData(painted, 0, 0);
  return { ...found, diff: canvas.toDataURL("image/png").split(",")[1] ?? null };
}

/** Whether a comparison is a changed screen. */
export function isChanged(found: Pick<PixelComparison, "ratio" | "sameSize">): boolean {
  return found.ratio > CHANGED_RATIO || !found.sameSize;
}
