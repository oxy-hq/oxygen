// @vitest-environment jsdom
//
// The labelled figure: an eyebrow over a 20px figure over a note, the figure at the bottom of a
// tile with no caption, boxed as a tile, the caller's judged colour on the figure or the caption,
// the source tip as a hover hint over the tile on a desk and as an ⓘ after the label where a
// phone layout provides its tap form; and the auto-fit row of them.

import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { PhoneHint } from "../../shell/phone/PhoneHint";
import { TapHintFrame } from "../containers";
import { ui } from "../recipes";
import { $, $$, fire, render } from "../ui.harness";
import { Stat, StatGrid, useTippedMin } from "./Stat";

afterEach(() => {
  document.body.innerHTML = "";
});

const tile = () => $(".oxy-ui-stat") as HTMLElement;
const parts = () => [...tile().children] as HTMLElement[];
const phoneFrame = (label: string, text: string) => <PhoneHint label={label}>{text}</PhoneHint>;
const Probe = () => <span data-min={useTippedMin("9rem")} />;
const OnPhone = ({ children }: { children: ReactNode }) => (
  <TapHintFrame.Provider value={phoneFrame}>{children}</TapHintFrame.Provider>
);

describe("Stat", () => {
  it("draws an eyebrow, the figure and its note, in that order", async () => {
    await render(<Stat label='To order now' value='$12.4k' sub='14 items under reorder' />);
    const [label, value, sub] = parts();
    expect(label.className).toContain(ui.eyebrow);
    expect(label.textContent).toBe("To order now");
    expect(value.className).toContain(ui.figure);
    expect(value.textContent).toBe("$12.4k");
    expect(sub.className).toContain(ui.note);
    expect(sub.textContent).toBe("14 items under reorder");
    expect(value.classList.contains("oxy-ui-stat__value--end")).toBe(false);
  });

  it("holds the figure at the bottom of a tile with no caption, so a row lines its figures up", async () => {
    await render(<Stat label='Open' value='3' />);
    expect(parts()).toHaveLength(2);
    expect(parts()[1].classList).toContain("oxy-ui-stat__value--end");
  });

  it("draws the tile's box only when boxed", async () => {
    const page = await render(<Stat label='Open' value='3' />);
    expect(tile().classList.contains(ui.tile)).toBe(false);
    await page.rerender(<Stat label='Open' value='3' boxed className='wide' />);
    expect(tile().classList).toContain(ui.tile);
    expect(tile().classList).toContain(ui.paddingSm);
    expect(tile().classList).toContain("wide");
  });

  it("wears the caller's judgement on the figure, or on the caption instead", async () => {
    const page = await render(<Stat label='Loss' value='$1' valueColor='var(--status-error)' />);
    expect(parts()[1].style.color).toBe("var(--status-error)");
    await page.rerender(
      <Stat label='Sales' value='$90k' sub='+0.4% MTD' subColor='var(--status-success)' />
    );
    expect(parts()[1].style.color).toBe("");
    expect(parts()[2].style.color).toBe("var(--status-success)");
  });

  it("puts a trend icon in front of the label", async () => {
    await render(
      <Stat label='Sales' value='$90k' icon={<svg aria-hidden='true' data-trend='' />} />
    );
    const label = parts()[0];
    expect(label.firstElementChild?.hasAttribute("data-trend")).toBe(true);
    expect(label.classList).toContain("oxy-ui-stat__label-row");
  });

  it("on a desk shows its source as a hover hint over the whole tile", async () => {
    await render(<Stat label='Income' value='$133k' tip='Census ACS, 2023.' boxed />);
    const anchor = $(".oxy-ui-hover-hint") as HTMLElement;
    expect(anchor.firstElementChild).toBe(tile());
    expect(tile().classList).toContain("oxy-ui-stat--fill");
    await fire(anchor, new PointerEvent("pointerover", { bubbles: true, pointerType: "mouse" }));
    expect($("[role='tooltip']")?.textContent).toBe("Census ACS, 2023.");
    expect($$("button")).toHaveLength(0);
  });

  it("on a phone layout shows it as an ⓘ after the label, and wraps nothing", async () => {
    await render(
      <OnPhone>
        <Stat label='Income' value='$133k' tip='Census ACS, 2023.' boxed />
      </OnPhone>
    );
    expect($(".oxy-ui-hover-hint")).toBeNull();
    const hint = parts()[0].querySelector("button") as HTMLButtonElement;
    expect(hint.getAttribute("aria-label")).toBe("About Income");
    expect(parts()[0].classList).toContain("oxy-ui-stat__label-row");
    expect(tile().classList.contains("oxy-ui-stat--fill")).toBe(false);
  });
});

describe("StatGrid", () => {
  it("fits as many tiles as it can at min each", async () => {
    const page = await render(
      <StatGrid>
        <Stat label='A' value='1' boxed />
      </StatGrid>
    );
    const grid = $(".oxy-ui-stat-grid") as HTMLElement;
    expect(grid.style.gridTemplateColumns).toBe("repeat(auto-fit, minmax(9rem, 1fr))");
    await page.rerender(
      <StatGrid min='12rem' style={{ marginTop: 4 }}>
        <Stat label='A' value='1' boxed />
      </StatGrid>
    );
    expect(grid.style.gridTemplateColumns).toBe("repeat(auto-fit, minmax(12rem, 1fr))");
    expect(grid.style.marginTop).toBe("4px");
  });

  it("widens a tipped tile's min by the ⓘ only where the ⓘ is drawn", async () => {
    const page = await render(<Probe />);
    expect($("[data-min]")?.dataset.min).toBe("9rem");
    await page.rerender(
      <OnPhone>
        <Probe />
      </OnPhone>
    );
    expect($("[data-min]")?.dataset.min).toBe("calc(9rem + 1.75rem)");
  });
});
