// Shared by the navigation group's jsdom tests (not itself a test file): jsdom lays nothing out,
// so the widths a strip is measured by are stubbed per class, and ResizeObserver, which jsdom
// lacks, is a fake the test fires when it changes a width, as a browser fires the real one.

import { act } from "react";

type Size = (el: HTMLElement) => number | undefined;

const props = ["clientWidth", "scrollWidth"] as const;
const real = new Map(
  props.map((p) => [p, Object.getOwnPropertyDescriptor(Element.prototype, p)] as const)
);
const realRect = Element.prototype.getBoundingClientRect;

/**
 * Stub `clientWidth` / `scrollWidth` (and optionally the box) by a function of the element:
 * return undefined to fall back to jsdom's 0.
 */
export function stubLayout(sizes: {
  clientWidth?: Size;
  scrollWidth?: Size;
  rect?: (el: HTMLElement) => { left: number; width: number } | undefined;
}): void {
  for (const p of props) {
    const size = sizes[p];
    if (!size) continue;
    Object.defineProperty(HTMLElement.prototype, p, {
      configurable: true,
      get(this: HTMLElement) {
        return size(this) ?? 0;
      }
    });
  }
  const rect = sizes.rect;
  if (rect)
    Element.prototype.getBoundingClientRect = function (this: Element) {
      const r = this instanceof HTMLElement ? rect(this) : undefined;
      const left = r?.left ?? 0;
      const width = r?.width ?? 0;
      return {
        x: left,
        y: 0,
        left,
        top: 0,
        right: left + width,
        bottom: 0,
        width,
        height: 0,
        toJSON: () => ({})
      } as DOMRect;
    };
}

/** Put jsdom's own layout back. */
export function restoreLayout(): void {
  for (const p of props) delete (HTMLElement.prototype as unknown as Record<string, unknown>)[p];
  for (const [p, d] of real) if (d) Object.defineProperty(Element.prototype, p, d);
  Element.prototype.getBoundingClientRect = realRect;
}

const observers = new Set<FakeResizeObserver>();

class FakeResizeObserver {
  constructor(private readonly callback: () => void) {}
  observe() {
    observers.add(this);
  }
  unobserve() {}
  disconnect() {
    observers.delete(this);
  }
  fire() {
    this.callback();
  }
}

/** Install the fake ResizeObserver; returns `resize()`, which fires every live observer. */
export function fakeResizeObserver(): { resize(): Promise<void>; uninstall(): void } {
  const had = (globalThis as { ResizeObserver?: unknown }).ResizeObserver;
  (globalThis as { ResizeObserver?: unknown }).ResizeObserver = FakeResizeObserver;
  return {
    async resize() {
      await act(async () => {
        for (const o of [...observers]) o.fire();
      });
    },
    uninstall() {
      observers.clear();
      (globalThis as { ResizeObserver?: unknown }).ResizeObserver = had;
    }
  };
}
