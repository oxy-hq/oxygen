/**
 * What the promote checks read from an open page, beyond `window.__OXY_APP__`:
 * the root page's link targets (to find the app's screens), a screen's
 * visible text, and whether its fonts have loaded.
 *
 * EACH FUNCTION HERE RUNS IN THE PAGE. Playwright sends a function as its
 * source, so each one uses nothing from outside itself: no import, no helper,
 * no constant of this module. This package compiles without the DOM's types,
 * so each names the little of the page it touches.
 *
 * A page's text is a customer's data. It is read for one use, the file the
 * platform keeps for a changed screen, and is never printed.
 */

/** The most link targets read from one page. */
export const MAX_LINKS = 2_000;

/**
 * Every `<a href>` target of the page, absolute, in document order. A link
 * that downloads is no screen. Nothing is clicked.
 */
export function readLinks(max: number): string[] {
  const page = (
    globalThis as {
      document?: {
        querySelectorAll(
          selector: string
        ): ArrayLike<{ href?: unknown; hasAttribute(name: string): boolean }>;
      };
    }
  ).document;
  if (!page) return [];
  const hrefs: string[] = [];
  for (const anchor of Array.from(page.querySelectorAll("a[href]"))) {
    if (hrefs.length >= max) break;
    if (anchor.hasAttribute("download")) continue;
    if (typeof anchor.href === "string") hrefs.push(anchor.href);
  }
  return hrefs;
}

/**
 * The page's visible text, at most `max` characters: what a person reads on
 * the screen, with runs of blank lines closed up.
 */
export function readVisibleText(max: number): string {
  const body = (globalThis as { document?: { body?: { innerText?: unknown } | null } }).document
    ?.body;
  // biome-ignore lint/performance/useDomNodeTextContent: the VISIBLE text is the point; textContent also carries hidden nodes and script source
  const raw = typeof body?.innerText === "string" ? body.innerText : "";
  const text = raw
    // Far more than is kept, so the tidying below never walks a whole long page.
    .slice(0, max * 4)
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
  // By character, so a cut never splits one.
  return Array.from(text).slice(0, max).join("");
}

/**
 * Resolves `true` once the page's fonts have loaded (at once, where the page
 * cannot say), and `false` when loading them failed.
 */
export function fontsLoaded(): Promise<boolean> {
  const fonts = (globalThis as { document?: { fonts?: { ready?: PromiseLike<unknown> } } }).document
    ?.fonts;
  return Promise.resolve(fonts?.ready).then(
    () => true,
    () => false
  );
}
