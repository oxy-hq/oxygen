// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { prefetchApp } from "./prefetchApp";

const links = () => Array.from(document.head.querySelectorAll('link[rel="prefetch"]'));
const rules = () => Array.from(document.head.querySelectorAll('script[type="speculationrules"]'));

describe("prefetchApp", () => {
  beforeEach(() => {
    document.head.innerHTML = "";
  });

  it("adds a document prefetch link for the app", () => {
    prefetchApp("/customer-apps/acme/revenue/");
    expect(links()).toHaveLength(1);
    const link = links()[0] as HTMLLinkElement;
    expect(link.getAttribute("href")).toBe("/customer-apps/acme/revenue/");
    // `as=document` is what makes the browser treat this as a navigation target
    // and honour the response's own preload hints, rather than parking bytes.
    expect(link.getAttribute("as")).toBe("document");
  });

  /// A card fires this on every pointer enter. Without de-duplication `<head>`
  /// accumulates one link per hover for the life of the page.
  it("is idempotent per URL", () => {
    prefetchApp("/customer-apps/acme/revenue/");
    prefetchApp("/customer-apps/acme/revenue/");
    prefetchApp("/customer-apps/acme/costs/");
    expect(links()).toHaveLength(2);
  });

  it("ignores an empty URL", () => {
    prefetchApp("");
    expect(links()).toHaveLength(0);
  });

  describe("where the browser has Speculation Rules", () => {
    // jsdom has no `HTMLScriptElement.supports` and no navigation timing, so
    // the test gives it both.
    beforeEach(() => {
      Object.defineProperty(HTMLScriptElement, "supports", {
        value: (type: string) => type === "speculationrules",
        configurable: true
      });
      vi.stubGlobal("PerformanceNavigationTiming", { prototype: { deliveryType: "" } });
    });
    afterEach(() => {
      Reflect.deleteProperty(HTMLScriptElement, "supports");
      vi.unstubAllGlobals();
    });

    /// Without `deliveryType` the opened page cannot tell it came from the
    /// prefetch, so nobody would count the open. A link's open revalidates and
    /// the server counts it.
    it("uses a link where the opened page could not report the prefetch", () => {
      vi.stubGlobal("PerformanceNavigationTiming", { prototype: {} });
      prefetchApp("/customer-apps/acme/revenue/");
      expect(rules()).toHaveLength(0);
      expect(links()).toHaveLength(1);
    });

    /// A rules prefetch lets the click reuse the document itself, not just its
    /// chunks. A prefetch and never a prerender: a prerender would run the app.
    it("asks for a Speculation Rules prefetch instead of a link", () => {
      prefetchApp("/customer-apps/acme/revenue/");
      expect(links()).toHaveLength(0);
      expect(rules()).toHaveLength(1);
      expect(JSON.parse(rules()[0].textContent ?? "")).toEqual({
        prefetch: [{ source: "list", urls: ["/customer-apps/acme/revenue/"] }]
      });
    });

    it("is idempotent per URL", () => {
      prefetchApp("/customer-apps/acme/revenue/");
      prefetchApp("/customer-apps/acme/revenue/");
      prefetchApp("/customer-apps/acme/costs/");
      expect(rules()).toHaveLength(2);
    });
  });
});
