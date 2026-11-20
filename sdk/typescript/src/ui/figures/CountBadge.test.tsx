// @vitest-environment jsdom
//
// The count badge: the tab count's recipe, neutral unless it carries the worst status it holds,
// the selected ground's neutral on a selected segment, square, capped by `max`, and read as
// words when it has them.

import { afterEach, describe, expect, it } from "vitest";
import { ui } from "../recipes";
import { strongestTone } from "../tone";
import { $, render } from "../ui.harness";
import { CountBadge, countBadgeText } from "./CountBadge";

afterEach(() => {
  document.body.innerHTML = "";
});

const badge = () => $(".oxy-ui-count-badge") as HTMLSpanElement;

describe("CountBadge", () => {
  it("is the neutral tab count by default, and the selected ground's neutral when selected", async () => {
    const page = await render(<CountBadge count={12} />);
    expect(badge().classList).toContain(ui.tabCount);
    expect(badge().classList).toContain(ui.tabCountNeutral);
    expect(badge().textContent).toBe("12");
    await page.rerender(<CountBadge count={12} selected />);
    expect(badge().classList).toContain(ui.tabCountNeutralOn);
    expect(badge().classList.contains(ui.tabCountNeutral)).toBe(false);
    await page.rerender(<CountBadge count={12} tone='neutral' selected />);
    expect(badge().classList).toContain(ui.tabCountNeutralOn);
  });

  it("wears the worst status it holds, selected or not", async () => {
    const tone = strongestTone(["success", "warning", undefined, "warning"]);
    const page = await render(<CountBadge count={3} tone={tone} />);
    expect(badge().classList).toContain(ui.chipTone.warning);
    expect(badge().dataset.tone).toBe("warning");
    await page.rerender(<CountBadge count={3} tone='error' selected />);
    expect(badge().classList).toContain(ui.chipTone.error);
    expect(badge().classList.contains(ui.tabCountNeutralOn)).toBe(false);
  });

  it("caps the figure at max and still reads the whole count", async () => {
    expect(countBadgeText(140, 99)).toBe("99+");
    expect(countBadgeText(99, 99)).toBe("99");
    const page = await render(<CountBadge count={140} max={99} />);
    const shown = badge().querySelector("[aria-hidden='true']");
    expect(shown?.textContent).toBe("99+");
    expect(badge().querySelector(".oxy-ui-count-badge__label")?.textContent).toBe("140");
    await page.rerender(<CountBadge count={3} label='3 overdue' tone='error' />);
    expect(badge().querySelector("[aria-hidden='true']")?.textContent).toBe("3");
    expect(badge().querySelector(".oxy-ui-count-badge__label")?.textContent).toBe("3 overdue");
  });
});
