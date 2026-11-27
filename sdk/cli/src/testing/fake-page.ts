/**
 * What a scripted page holds, for `fake-playwright.ts`: the little of a
 * document the checks' in-page functions touch (`checks/page-reads.ts`,
 * `checks/pixel-compare.ts`, `window.__OXY_APP__`), and pictures small enough
 * to write out by hand.
 *
 * THE CHECKS' OWN FUNCTIONS RUN HERE. A function handed to `evaluate` is
 * called with a stand-in `document` and `createImageBitmap` in place while it
 * starts, so its real code reads the scripted links and text and compares the
 * scripted pictures. Each of those functions takes what it needs of the page
 * before it first waits for anything, which is what lets two pages be read
 * side by side.
 *
 * AN EXPRESSION IS EVALUATED AS REAL PLAYWRIGHT EVALUATES ONE: alone, with no
 * argument, and a function it evaluates to is not called. What it leaves on
 * the page's global object stays there for the functions that follow.
 *
 * A FAKE PICTURE is its width, height and RGBA pixels as JSON, in place of a
 * PNG. The stand-in canvas decodes and encodes that, so the check's real
 * comparison runs over those pixels and a test that scripts two pictures gets
 * the ratio and the diff the browser would produce for them.
 */

import { Script } from "node:vm";

/** What a page holds once it is up. */
export interface PageContent {
  /** `window.__OXY_APP__` as the served document set it. */
  app?: unknown;
  /** The page's `<a href>` targets, as the browser resolves them; `download` marks a link that downloads. */
  links?: (string | { href: string; download: true })[];
  /** The page's visible text. */
  text?: string;
}

type Rgba = readonly [number, number, number, number];

interface FakePicture {
  width: number;
  height: number;
  /** `width * height * 4` bytes. */
  data: number[];
}

/**
 * A picture of `width` by `height`, every pixel `fill`, except the ones
 * `paint` answers a colour for.
 */
export function fakePng(
  width: number,
  height: number,
  fill: Rgba,
  paint?: (x: number, y: number) => Rgba | undefined
): Uint8Array {
  const data: number[] = [];
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) data.push(...(paint?.(x, y) ?? fill));
  }
  const picture: FakePicture = { width, height, data };
  return Buffer.from(JSON.stringify(picture), "utf8");
}

/** The pixels of a fake picture; throws on bytes that are not one. */
export function readFakePng(bytes: Uint8Array): FakePicture {
  return JSON.parse(Buffer.from(bytes).toString("utf8")) as FakePicture;
}

/** A canvas that holds a fake picture's pixels, and encodes them as one. */
function fakeCanvas() {
  const canvas = {
    width: 0,
    height: 0,
    pixels: [] as number[],
    getContext: () => ({
      drawImage: (bitmap: FakePicture) => {
        canvas.pixels = [...bitmap.data];
      },
      getImageData: (_x: number, _y: number, width: number, height: number) => ({
        data: canvas.pixels.slice(0, width * height * 4)
      }),
      createImageData: (width: number, height: number) => ({
        data: new Array<number>(width * height * 4).fill(0)
      }),
      putImageData: (image: { data: ArrayLike<number> }) => {
        canvas.pixels = Array.from(image.data);
      }
    }),
    toDataURL: () => {
      const picture: FakePicture = {
        width: canvas.width,
        height: canvas.height,
        data: canvas.pixels
      };
      return `data:image/png;base64,${Buffer.from(JSON.stringify(picture), "utf8").toString("base64")}`;
    }
  };
  return canvas;
}

/** The stand-in page: its `document`, and the one global function the comparison calls. */
function fakeScope(content: PageContent): Record<string, unknown> {
  return {
    __OXY_APP__: content.app,
    document: {
      querySelectorAll: (selector: string) =>
        selector === "a[href]"
          ? (content.links ?? []).map((link) => ({
              href: typeof link === "string" ? link : link.href,
              hasAttribute: (name: string) => name === "download" && typeof link !== "string"
            }))
          : [],
      body: { innerText: content.text ?? "" },
      fonts: { ready: Promise.resolve() },
      createElement: () => fakeCanvas()
    },
    createImageBitmap: async (blob: Blob) => readFakePng(new Uint8Array(await blob.arrayBuffer()))
  };
}

/**
 * Call `run` with `content` in place as the page, and `installed` (what
 * earlier expressions left on the page's global object) beside it. What `run`
 * takes of the page, it must take before it first waits.
 */
export function inFakePage<T>(
  content: PageContent,
  run: () => T,
  installed: Record<string, unknown> = {}
): T {
  const scope = globalThis as Record<string, unknown>;
  const page = { ...fakeScope(content), ...installed };
  const before = Object.keys(page).map(
    (name) => [name, Object.getOwnPropertyDescriptor(scope, name)] as const
  );
  Object.assign(scope, page);
  try {
    return run();
  } finally {
    for (const [name, descriptor] of before) {
      if (descriptor) Object.defineProperty(scope, name, descriptor);
      else delete scope[name];
    }
  }
}

/**
 * Evaluate `expression` as a page would, alone and with no argument, and
 * answer what it left on the page's global object.
 */
export function leftByExpression(expression: string): Record<string, unknown> {
  const page: Record<string, unknown> = {};
  new Script(expression).runInNewContext(page);
  return page;
}
