// @vitest-environment jsdom
//
// 2.32.0 AlertStrip `hint` and the `check` tone (decision 99, bmg reorder-sheet's attention row):
// an alert's hint opens on hover and on keyboard focus, a plain callout with a hint becomes a Tab
// stop, and the check alert draws the theme's purple status in the callout recipe. An alert with
// neither is drawn as before (keptPieces.defaults.test.tsx).

import { act } from "react";
import { afterAll, afterEach, describe, expect, it } from "vitest";
import { paintUi, rgb, themeValue, unpaintUi } from "../paint.harness";
import { $, $$, fire, render } from "../ui.harness";
import { AlertStrip } from "./AlertStrip";

afterEach(() => {
  document.body.innerHTML = "";
});
afterAll(() => unpaintUi());

const bubble = () => $('[role="tooltip"]');
const focusOn = async (el: Element | null | undefined) => {
  await act(async () => {
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    (el as HTMLElement | null)?.focus();
  });
};

const HINT = "Nothing sellable and real demand, but Amazon holds units in Researching.";

describe("AlertStrip hint", () => {
  it("opens on a mouse hover over a door, with the alert's words", async () => {
    await render(
      <AlertStrip
        alerts={[{ tone: "check", label: "1 SKU to check", hint: HINT, onSelect: () => {} }]}
      />
    );
    const anchor = $(".oxy-ui-hover-hint");
    expect(anchor?.className).toContain("on-control");
    await fire(anchor, new PointerEvent("pointerover", { bubbles: true, pointerType: "mouse" }));
    expect(bubble()?.textContent).toBe(HINT);
  });

  it("opens on keyboard focus of the door, and describes it", async () => {
    await render(
      <AlertStrip alerts={[{ tone: "warning", label: "2 at risk", hint: HINT, href: "/risk" }]} />
    );
    const door = $("a");
    await focusOn(door);
    expect(bubble()?.textContent).toBe(HINT);
    expect(door?.getAttribute("aria-describedby")).toBe(bubble()?.id);
  });

  it("makes a plain callout with a hint a Tab stop, and leaves one without a hint out", async () => {
    await render(
      <AlertStrip
        alerts={[
          { tone: "neutral", label: "1 reports negative stock", hint: HINT },
          { tone: "neutral", label: "1 discontinued" }
        ]}
      />
    );
    const stops = $$('[tabindex="0"]');
    expect(stops).toHaveLength(1);
    expect(stops[0].textContent).toContain("1 reports negative stock");
    await focusOn(stops[0]);
    expect(bubble()?.textContent).toBe(HINT);
  });

  it("keeps the door the same element and the same Tab stops with or without a hint", async () => {
    await render(<AlertStrip alerts={[{ tone: "error", label: "3 out", onSelect: () => {} }]} />);
    const plain = $$("button").length;
    document.body.innerHTML = "";
    await render(
      <AlertStrip alerts={[{ tone: "error", label: "3 out", onSelect: () => {}, hint: HINT }]} />
    );
    expect($$("button")).toHaveLength(plain);
    expect($$('[tabindex="0"]')).toHaveLength(0);
  });
});

describe("AlertStrip check tone", () => {
  it("draws the check callout in the theme's check status, in light and dark", async () => {
    await render(
      <AlertStrip alerts={[{ tone: "check", label: "1 to check", onSelect: () => {} }]} />
    );
    const door = $("button") as HTMLElement;
    expect(door.getAttribute("data-tone")).toBe("check");
    expect(door.className).toContain("oxy-ui-callout--check");
    for (const mode of ["light", "dark"] as const) {
      paintUi({ mode });
      const style = getComputedStyle(door);
      expect(rgb(style.color), mode).toBe(rgb(themeValue("--status-check", mode)));
      expect(rgb(style.backgroundColor), mode).toBe(rgb(themeValue("--status-check-bg", mode)));
      expect(rgb(style.borderTopColor), mode).toBe(rgb(themeValue("--status-check-line", mode)));
    }
    expect(themeValue("--status-check", "light")).toBe("#7e22ce");
    expect(themeValue("--status-check", "dark")).toBe("#c084fc");
  });
});
