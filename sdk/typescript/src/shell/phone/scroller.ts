// Finding the box a phone page scrolls in, for scrollKey and for the page's
// height. `overflow-y` alone cannot answer it: `overflow-x: hidden` computes
// `overflow-y` to auto on a box that never scrolls, and <body>'s overflow
// belongs to the viewport unless <html> sets its own.

const SCROLLS = new Set(["auto", "scroll", "overlay"]);

function clipsOwnContent(node: HTMLElement): boolean {
  if (
    node === document.body &&
    getComputedStyle(document.documentElement).overflowY === "visible"
  ) {
    return false;
  }
  return getComputedStyle(node).overflowY !== "visible";
}

function documentScroller(): HTMLElement {
  return (document.scrollingElement as HTMLElement | null) ?? document.documentElement;
}

/** The nearest box around `el` that really scrolls, else the document. */
export function nearestScroller(el: HTMLElement | null): HTMLElement {
  const root = document.documentElement;
  for (let node = el?.parentElement; node && node !== root; node = node.parentElement) {
    if (!clipsOwnContent(node) || !SCROLLS.has(getComputedStyle(node).overflowY)) continue;
    if (node.scrollHeight > node.clientHeight || node.scrollTop > 0) return node;
  }
  return documentScroller();
}

function outerBottom(el: Element): number {
  return (
    el.getBoundingClientRect().bottom + (Number.parseFloat(getComputedStyle(el).marginBottom) || 0)
  );
}

// The last child that takes up room: a fixed or absolute one (a portaled sheet)
// says nothing about where the content ends.
function lastInFlow(node: HTMLElement): Element | null {
  for (let c = node.lastElementChild; c; c = c.previousElementSibling) {
    const { position, display } = getComputedStyle(c);
    if (position !== "fixed" && position !== "absolute" && display !== "none") return c;
  }
  return null;
}

/**
 * How tall `page` must be for its end to reach the bottom of the box that
 * bounds it, through any wrappers that merely hug their content; 0 when it needs
 * no help (the content already overflows, or the page fills the box).
 * Call it with the page's own min-height cleared.
 */
export function fillHeight(page: HTMLElement): number {
  const root = document.documentElement;
  let free = 0;
  for (let node = page.parentElement; node; node = node.parentElement) {
    const isRoot = node === root;
    const bounds = isRoot || clipsOwnContent(node);
    if (bounds && node.scrollHeight > node.clientHeight) return 0;
    const last = lastInFlow(node);
    if (last) {
      const style = getComputedStyle(node);
      const inner = isRoot
        ? node.clientHeight
        : node.getBoundingClientRect().top +
          node.clientTop +
          node.clientHeight -
          (Number.parseFloat(style.paddingBottom) || 0);
      free += Math.max(0, inner - outerBottom(last));
    }
    if (bounds) break;
  }
  return free > 0 ? page.getBoundingClientRect().height + free : 0;
}
