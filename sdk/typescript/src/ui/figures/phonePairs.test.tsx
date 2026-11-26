// @vitest-environment jsdom
//
// 2.32.0 figure grids on a phone (decision 99): StatGrid `phonePairs` (Quadrogen's two tiles to a
// row whatever `min` is, an odd last tile across) and PhoneKpiStrip `oddLastAcross` (Site Scout's
// KPI row, upright only). Both opt-in: without them a grid is as it was
// (keptPieces.defaults.test.tsx), because an odd count in any current caller would otherwise move.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, afterEach, describe, expect, it, vi } from "vitest";
import { PhoneKpiStrip } from "../../shell/phone/PhoneKpiStrip";
import { HoverHint } from "../containers";
import { paintUi, unpaintUi } from "../paint.harness";
import { $, $$, render, stubMatchMedia } from "../ui.harness";
import { Stat, StatGrid } from "./Stat";

afterEach(() => {
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
});
afterAll(() => unpaintUi());

const grid = () => $(".oxy-ui-stat-grid") as HTMLElement;

async function mountGrid(phone: boolean, count = 3, phonePairs = true) {
  vi.stubGlobal("matchMedia", stubMatchMedia(phone).matchMedia);
  await render(
    <StatGrid min='12rem' phonePairs={phonePairs}>
      {Array.from({ length: count }, (_, i) => `Figure ${i + 1}`).map((label) => (
        <Stat key={label} boxed label={label} value={label} />
      ))}
    </StatGrid>
  );
}

describe("StatGrid phonePairs", () => {
  it("on a phone, sets two columns whatever min is", async () => {
    await mountGrid(true);
    expect(grid().style.gridTemplateColumns).toBe("repeat(2, minmax(0px, 1fr))");
    expect(grid().className).toContain("oxy-ui-stat-grid--pairs");
  });

  it("on a desk, keeps the auto-fit row at min", async () => {
    await mountGrid(false);
    expect(grid().style.gridTemplateColumns).toBe("repeat(auto-fit, minmax(12rem, 1fr))");
    expect(grid().className).not.toContain("--pairs");
  });

  it("spans an odd last tile across the row, and no tile of an even count", async () => {
    await mountGrid(true, 3);
    paintUi();
    const spans = () => $$(".oxy-ui-stat-grid > *").map((t) => getComputedStyle(t).gridColumn);
    expect(spans().map((s) => s === "1 / -1")).toEqual([false, false, true]);
    document.body.innerHTML = "";
    await mountGrid(true, 4);
    expect(spans().some((s) => s === "1 / -1")).toBe(false);
  });

  it("spans a tile wrapped in its hint, since the rule is on the grid's child", async () => {
    vi.stubGlobal("matchMedia", stubMatchMedia(true).matchMedia);
    await render(
      <StatGrid phonePairs>
        <Stat boxed label='A' value='1' />
        <HoverHint text='Where the figure comes from.'>
          <Stat boxed label='B' value='2' />
        </HoverHint>
        <Stat boxed label='C' value='3' tip='The source.' />
      </StatGrid>
    );
    paintUi();
    const last = grid().lastElementChild as HTMLElement;
    expect(last.className).toContain("oxy-ui-hover-hint");
    expect(getComputedStyle(last).gridColumn).toBe("1 / -1");
  });
});

describe("PhoneKpiStrip oddLastAcross", () => {
  const items = [
    { label: "Sales", value: "1" },
    { label: "Labour", value: "2" },
    { label: "Waste", value: "3" }
  ];

  it("marks the strip when asked, and only then", async () => {
    await render(<PhoneKpiStrip items={items} oddLastAcross />);
    expect($(".oxy-phone-kpis")?.className).toContain("oxy-phone-kpis--odd-across");
    document.body.innerHTML = "";
    await render(<PhoneKpiStrip items={items} />);
    expect($(".oxy-phone-kpis")?.className).not.toContain("--odd-across");
  });

  it("spans the odd last tile across the row, upright only (shell.css)", () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const css = readFileSync(join(here, "..", "..", "shell", "shell.css"), "utf8").replace(
      /\/\*[\s\S]*?\*\//g,
      ""
    );
    const block = /@media \(orientation: portrait\)\s*\{([\s\S]*?\})\s*\}/.exec(css)?.[1] ?? "";
    expect(block).toMatch(
      /\.oxy-phone-kpis--odd-across > \.oxy-phone-kpi:last-child:nth-child\(odd\)\s*\{\s*grid-column: 1 \/ -1;/
    );
    // Nowhere outside it: on its side the strip is left as the app laid it out.
    expect(css.split("oxy-phone-kpis--odd-across")).toHaveLength(2);
  });
});
