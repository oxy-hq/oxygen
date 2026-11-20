// @vitest-environment jsdom
//
// A scroll box is a Tab stop exactly while it has something to scroll to, across or down, and it
// follows a resize. jsdom lays nothing out, so the sizes are set the way a browser reports them
// (as Store Ops' scrollFocus.test.tsx does), and ResizeObserver is a stub the test fires.

import { afterEach, describe, expect, it, vi } from "vitest";
import { $, render } from "../ui.harness";
import { overflows, useScrollFocusable } from "./scrollFocus";

const SIZES = ["scrollWidth", "clientWidth", "scrollHeight", "clientHeight"] as const;
type Sizes = Record<(typeof SIZES)[number], number>;

let sizes: Sizes = { scrollWidth: 0, clientWidth: 0, scrollHeight: 0, clientHeight: 0 };
for (const name of SIZES)
  Object.defineProperty(HTMLElement.prototype, name, {
    configurable: true,
    get: () => sizes[name]
  });

afterEach(() => {
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
});

function Sheet() {
  const box = useScrollFocusable<HTMLDivElement>();
  return (
    // biome-ignore lint/a11y/useAriaPropsSupportedByRole: named only while it is a region (it scrolls).
    <div
      ref={box.ref}
      id='box'
      tabIndex={box.tabIndex}
      role={box.scrolls ? "region" : undefined}
      aria-label={box.scrolls ? "Stores" : undefined}
    >
      <table />
    </div>
  );
}

describe("overflows", () => {
  it("is true when the content is wider than the box: a sheet on a phone", () => {
    expect(
      overflows({ scrollWidth: 1131, clientWidth: 252, scrollHeight: 478, clientHeight: 478 })
    ).toBe(true);
  });

  it("is true when the content is taller than the box: a sheet under its maxHeight", () => {
    expect(
      overflows({ scrollWidth: 1302, clientWidth: 1302, scrollHeight: 593, clientHeight: 478 })
    ).toBe(true);
  });

  it("is false when everything fits, and ignores a one-pixel rounding difference", () => {
    expect(
      overflows({ scrollWidth: 900, clientWidth: 900, scrollHeight: 300, clientHeight: 300 })
    ).toBe(false);
    expect(
      overflows({ scrollWidth: 901, clientWidth: 900, scrollHeight: 301, clientHeight: 300 })
    ).toBe(false);
  });
});

describe("useScrollFocusable", () => {
  it("makes a box that scrolls a named Tab stop", async () => {
    sizes = { scrollWidth: 900, clientWidth: 360, scrollHeight: 200, clientHeight: 200 };
    await render(<Sheet />);
    expect($("#box")?.getAttribute("tabindex")).toBe("0");
    expect($("#box")?.getAttribute("role")).toBe("region");
  });

  it("adds no stop for a box that fits", async () => {
    sizes = { scrollWidth: 360, clientWidth: 360, scrollHeight: 200, clientHeight: 200 };
    await render(<Sheet />);
    expect($("#box")?.hasAttribute("tabindex")).toBe(false);
    expect($("#box")?.hasAttribute("role")).toBe(false);
  });

  it("follows a resize: the content grows past the box, and the box becomes a stop", async () => {
    const observers: { fire: () => void; watched: Element[] }[] = [];
    vi.stubGlobal(
      "ResizeObserver",
      class {
        watched: Element[] = [];
        constructor(private readonly fn: () => void) {
          observers.push({ fire: () => this.fn(), watched: this.watched });
        }
        observe(el: Element) {
          this.watched.push(el);
        }
        disconnect() {}
      }
    );
    sizes = { scrollWidth: 360, clientWidth: 360, scrollHeight: 200, clientHeight: 200 };
    await render(<Sheet />);
    expect($("#box")?.hasAttribute("tabindex")).toBe(false);
    // The box and its table are both watched.
    expect(observers[0].watched.map((el) => el.tagName)).toEqual(["DIV", "TABLE"]);
    sizes = { ...sizes, scrollHeight: 640 };
    const { act } = await import("react");
    await act(async () => observers[0].fire());
    expect($("#box")?.getAttribute("tabindex")).toBe("0");
  });
});
