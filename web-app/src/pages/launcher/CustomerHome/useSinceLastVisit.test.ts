// @vitest-environment jsdom

import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { WidgetCard } from "@/types/homeWidgets";
import { useSinceLastVisit, widgetFingerprint } from "./useSinceLastVisit";

const card = (key: string, headline: string): WidgetCard =>
  ({
    key,
    view: { level: "ok", headline, detail: null, badge: null, rows: [], buttons: [] }
  }) as unknown as WidgetCard;

const STORE = "oxy.home.seen.acme";
const remembered = (...cards: WidgetCard[]) =>
  window.localStorage.setItem(
    STORE,
    JSON.stringify(Object.fromEntries(cards.map((c) => [c.key, widgetFingerprint(c)])))
  );

afterEach(() => window.localStorage.clear());

describe("useSinceLastVisit", () => {
  it("marks what changed and what is new, and nothing that stayed the same", () => {
    remembered(card("a", "3 open"), card("b", "All done"));
    const now = [card("a", "1 open"), card("b", "All done"), card("c", "Hello")];
    const { result } = renderHook(() => useSinceLastVisit("acme", now));
    expect([...result.current]).toEqual([
      ["a", "updated"],
      ["c", "new"]
    ]);
  });

  it("keeps the marks through the visit, and clears what was seen when Back restores the page", () => {
    remembered(card("a", "3 open"));
    const now = [card("a", "1 open")];
    const { result } = renderHook(() => useSinceLastVisit("acme", now));
    act(() => {
      Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" });
      document.dispatchEvent(new Event("visibilitychange"));
    });
    // Still marked: the person has not come back yet.
    expect(result.current.get("a")).toBe("updated");
    act(() => {
      window.dispatchEvent(Object.assign(new Event("pageshow"), { persisted: true }));
    });
    expect(result.current.size).toBe(0);
  });

  it("keeps each org's memory to itself when the home switches org", () => {
    window.localStorage.setItem("oxy.home.seen.other", JSON.stringify({ a: "other-org" }));
    remembered(card("a", "3 open"));
    const { result, rerender } = renderHook(({ org, cards }) => useSinceLastVisit(org, cards), {
      initialProps: { org: "acme", cards: [card("a", "3 open")] }
    });
    rerender({ org: "other", cards: [card("a", "Other org's view")] });
    // Leaving acme remembered acme's own view, not the other org's.
    expect(JSON.parse(window.localStorage.getItem(STORE) ?? "{}")).toEqual({
      a: widgetFingerprint(card("a", "3 open"))
    });
    expect(result.current.get("a")).toBe("updated");
  });

  it("does not forget a widget that failed this visit", () => {
    remembered(card("a", "3 open"));
    const failed = { key: "a", view: null } as unknown as WidgetCard;
    const { unmount } = renderHook(() => useSinceLastVisit("acme", [failed]));
    unmount();
    expect(JSON.parse(window.localStorage.getItem(STORE) ?? "{}")).toEqual({
      a: widgetFingerprint(card("a", "3 open"))
    });
  });
});
