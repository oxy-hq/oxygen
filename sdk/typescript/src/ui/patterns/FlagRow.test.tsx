// @vitest-environment jsdom
//
// FlagRow: the five columns hold their slots in reading order; the row is one button that opens
// its detail (`aria-haspopup`), says when it is the open one (`aria-current`) and carries the
// selected or hover recipe; the row's own actions sit beside that button, never inside it, and
// pressing one does not open the row; a row with no `onSelect` is static.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, afterEach, describe, expect, it, vi } from "vitest";
import { paintUi, unpaintUi } from "../paint.harness";
import { $, click, render } from "../ui.harness";
import { FlagRow, type FlagRowProps } from "./FlagRow";

afterEach(() => {
  document.body.innerHTML = "";
});

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "patterns.css"), "utf8");

const cells: FlagRowProps = {
  rank: 4,
  name: "Riverside",
  measure: "Food cost",
  evidence: "Food cost ran 3.1 points over comparable sites for 4 months.",
  sparkline: <svg data-spark='' role='img' aria-label='trend' />,
  figure: "$12,400",
  deviation: <span data-chip=''>+18%</span>,
  deviationNote: "vs peers"
};

const li = () => $("li.oxy-ui-flag-row") as HTMLElement;
const main = () => $(".oxy-ui-flag-row__main") as HTMLElement;

describe("FlagRow", () => {
  it("lays out rank, name and measure, the sentence, the trend, the figure and its deviation", async () => {
    await render(
      <ul>
        <FlagRow {...cells} onSelect={() => {}} />
      </ul>
    );
    const parts = [
      ".oxy-ui-flag-row__rank",
      ".oxy-ui-flag-row__name",
      ".oxy-ui-flag-row__measure",
      ".oxy-ui-flag-row__evidence",
      ".oxy-ui-flag-row__trend",
      ".oxy-ui-flag-row__figure",
      ".oxy-ui-flag-row__deviation"
    ].map((sel) => $(sel)?.textContent);
    expect(parts).toEqual([
      "4",
      "Riverside",
      "Food cost",
      "Food cost ran 3.1 points over comparable sites for 4 months.",
      "",
      "$12,400",
      "+18%vs peers"
    ]);
    expect($(".oxy-ui-flag-row__trend [data-spark]")).not.toBeNull();
    // Reading order, left to right: rank, the claim, the trend, the figures.
    expect([...main().children].map((c) => c.className.split(" ").pop())).toEqual([
      "oxy-ui-flag-row__rank",
      "oxy-ui-flag-row__claim",
      "oxy-ui-flag-row__trend",
      "oxy-ui-flag-row__figures"
    ]);
    // The trend waits for 40rem; the name holds its column from 48rem.
    expect(css).toMatch(/\.oxy-ui-flag-row__trend \{\s*display: none;/);
    expect(css).toMatch(
      /@media \(min-width: 40rem\) \{\s*\.oxy-ui-flag-row__trend \{\s*display: block;/
    );
  });

  it("is one button that opens the row's detail, current while it is the open one", async () => {
    const onSelect = vi.fn();
    const page = await render(
      <ul>
        <FlagRow {...cells} onSelect={onSelect} />
      </ul>
    );
    expect(main().tagName).toBe("BUTTON");
    expect(main().getAttribute("aria-haspopup")).toBe("dialog");
    expect(main().hasAttribute("aria-current")).toBe(false);
    expect(li().classList).toContain("oxy-ui-row-hover");
    expect(li().classList).not.toContain("oxy-ui-row-selected");
    await click(main());
    expect(onSelect).toHaveBeenCalledTimes(1);

    await page.rerender(
      <ul>
        <FlagRow {...cells} onSelect={onSelect} selected opens={false} />
      </ul>
    );
    expect(main().getAttribute("aria-current")).toBe("true");
    expect(main().hasAttribute("aria-haspopup")).toBe(false);
    // Selection outranks hover: one or the other.
    expect(li().classList).toContain("oxy-ui-row-selected");
    expect(li().classList).not.toContain("oxy-ui-row-hover");
  });

  it("puts the row's own actions beside its button, and pressing one does not open the row", async () => {
    const onSelect = vi.fn();
    const onSnooze = vi.fn();
    await render(
      <ul>
        <FlagRow
          {...cells}
          onSelect={onSelect}
          actions={
            <button type='button' data-snooze='' onClick={onSnooze}>
              Snooze
            </button>
          }
        />
      </ul>
    );
    const snooze = $("[data-snooze]") as HTMLButtonElement;
    expect(main().contains(snooze)).toBe(false);
    expect(snooze.closest(".oxy-ui-flag-row__actions")?.parentElement).toBe(li());
    expect(main().querySelector("button")).toBeNull();
    await click(snooze);
    expect(onSnooze).toHaveBeenCalledTimes(1);
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("is static without onSelect: no button, no hover; only what it is given", async () => {
    await render(
      <ul>
        <FlagRow name='Harbour' figure='$900' />
      </ul>
    );
    expect(main().tagName).toBe("DIV");
    expect($("li button")).toBeNull();
    expect(li().classList).not.toContain("oxy-ui-row-hover");
    for (const part of ["rank", "measure", "evidence", "trend", "deviation", "actions"])
      expect($(`.oxy-ui-flag-row__${part}`), part).toBeNull();
    expect(li().textContent).toBe("Harbour$900");
  });
});

// The row's lines, with ui.css painted on: the reference's FlagRow (oxy-hq/reference-app@81ee8aa
// `src/components/FlagRow.tsx`) sets the name `text-base font-medium` and the figure `text-xl
// font-semibold`, both on the scale's 1.4 line. On the text recipes' 1.375 and 1.25 the figure's
// line was 30px, not 33.6px, and every row 81px where the reference's is 84.7 (SDK-GAPS 5).
describe("FlagRow, painted", () => {
  afterAll(() => unpaintUi());

  it("sets the name and the figure on the reference's sizes, weights and 1.4 line", async () => {
    paintUi();
    await render(
      <ul>
        <FlagRow {...cells} onSelect={() => {}} />
      </ul>
    );
    const name = getComputedStyle($(".oxy-ui-flag-row__name") as HTMLElement);
    const figure = getComputedStyle($(".oxy-ui-flag-row__figure") as HTMLElement);
    // 16px on a 22.4px line and 24px on a 33.6px one, as the reference's computed.
    expect([name.fontSize, name.lineHeight, name.fontWeight]).toEqual(["16px", "22.4px", "500"]);
    expect([figure.fontSize, figure.lineHeight, figure.fontWeight]).toEqual([
      "24px",
      "33.6px",
      "600"
    ]);
  });
});
