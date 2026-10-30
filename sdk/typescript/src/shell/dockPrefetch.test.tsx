// @vitest-environment jsdom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DOCK_PREFETCH_RULES, useDockPrefetch } from "./dockPrefetch";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function Dock() {
  useDockPrefetch();
  return null;
}

const rules = () => Array.from(document.head.querySelectorAll('script[type="speculationrules"]'));

let root: Root | null = null;
function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  act(() => root?.render(<Dock />));
}

afterEach(() => {
  act(() => root?.unmount());
  root = null;
  document.head.innerHTML = "";
  Reflect.deleteProperty(HTMLScriptElement, "supports");
  vi.unstubAllGlobals();
});

describe("useDockPrefetch", () => {
  it("does nothing where the browser has no Speculation Rules", () => {
    mount();
    expect(rules()).toHaveLength(0);
  });

  describe("where the browser has them", () => {
    // jsdom has no `HTMLScriptElement.supports` and no navigation timing, so
    // the test gives it both.
    beforeEach(() => {
      Object.defineProperty(HTMLScriptElement, "supports", {
        value: (type: string) => type === "speculationrules",
        configurable: true
      });
      vi.stubGlobal("PerformanceNavigationTiming", { prototype: { deliveryType: "" } });
    });

    /// The opened app counts its own open only if its navigation entry can say
    /// it came from a prefetch. Without that, a prefetch would lose the open.
    it("does nothing where the opened page could not report the prefetch", () => {
      vi.stubGlobal("PerformanceNavigationTiming", { prototype: {} });
      mount();
      expect(rules()).toHaveLength(0);
    });

    it("prefetches the dock's other apps on hover, and removes the rules with the shell", () => {
      mount();
      expect(rules()).toHaveLength(1);
      expect(JSON.parse(rules()[0].textContent ?? "")).toEqual(DOCK_PREFETCH_RULES);
      act(() => root?.unmount());
      root = null;
      expect(rules()).toHaveLength(0);
    });

    /// A prerender would run the app, its queries and its functions, for a hover.
    it("asks for a prefetch, never a prerender", () => {
      expect(Object.keys(DOCK_PREFETCH_RULES)).toEqual(["prefetch"]);
      expect(DOCK_PREFETCH_RULES.prefetch[0].eagerness).toBe("moderate");
    });

    /// The selector is only worth anything if it matches the rail's markup.
    it("matches the rail's app links but not the current app", () => {
      document.body.innerHTML = `
        <nav class="oxy-rail">
          <a class="oxy-rail__item" href="/customer-apps/acme/orders/">Orders</a>
          <a class="oxy-rail__item" href="/customer-apps/acme/ops/" aria-current="page">Ops</a>
        </nav>`;
      const selector = DOCK_PREFETCH_RULES.prefetch[0].where.and[1].selector_matches;
      const matched = Array.from(document.querySelectorAll(selector)).map((a) => a.textContent);
      expect(matched).toEqual(["Orders"]);
    });
  });
});
