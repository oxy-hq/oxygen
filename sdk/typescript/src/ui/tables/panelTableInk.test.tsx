// @vitest-environment jsdom
//
// SDK-GAPS 6: the reference's subject row (`font-medium text-foreground` on the row) drew its
// figures in full ink at medium weight, and its name at the regular weight. The cells must take
// the row's ink and weight, so a caller's row class reaches them; the name keeps its own.
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { paintUi, rgb, themeValue, unpaintUi } from "../paint.harness";
import { render } from "../ui.harness";
import { PanelTable } from "./PanelTable";

type Row = { name: string; share: string; subject?: boolean };
const ROWS: Row[] = [
  { name: "Fairhaven", share: "32.4%" },
  { name: "Alder Creek", share: "24.8%", subject: true }
];

let rowRule: HTMLStyleElement;
beforeAll(() => {
  paintUi();
  // The caller's own class, unlayered as an app's utility is.
  rowRule = document.createElement("style");
  rowRule.textContent = ".subject-row { color: rgb(24, 24, 27); font-weight: 500; }";
  document.head.append(rowRule);
});
afterAll(() => {
  rowRule.remove();
  unpaintUi();
});
afterEach(() => {
  document.body.innerHTML = "";
});

async function cells(): Promise<{ plain: HTMLElement[]; subject: HTMLElement[] }> {
  await render(
    <PanelTable
      caption='Compared'
      columns={[
        { header: "Store", cell: (r: Row) => r.name },
        { header: "% of sales", align: "right", cell: (r: Row) => r.share }
      ]}
      rows={ROWS}
      rowKey={(r) => r.name}
      rowClassName={(r) => (r.subject ? "subject-row" : undefined)}
    />
  );
  const rows = [...document.querySelectorAll<HTMLTableRowElement>("tbody tr")];
  return { plain: [...rows[0].cells], subject: [...rows[1].cells] };
}

describe("PanelTable ink and weight", () => {
  it("a row's own class reaches its figures: full ink at the row's weight", async () => {
    const { subject } = await cells();
    const figure = subject[1];
    expect(getComputedStyle(figure).color).toBe("rgb(24, 24, 27)");
    expect(getComputedStyle(figure).fontWeight).toBe("500");
  });

  it("a plain row's figures stay in the dim ink at the regular weight", async () => {
    const { plain } = await cells();
    expect(rgb(getComputedStyle(plain[1]).color)).toBe(rgb(themeValue("--dim-foreground")));
    expect(getComputedStyle(plain[1]).fontWeight).toBe("400");
  });

  it("the name keeps full ink at the regular weight in every row", async () => {
    const { plain, subject } = await cells();
    for (const name of [plain[0], subject[0]]) {
      expect(rgb(getComputedStyle(name).color)).toBe(rgb(themeValue("--foreground")));
      expect(getComputedStyle(name).fontWeight).toBe("400");
    }
  });

  it("puts no side padding on its cells, as the reference's compact table", async () => {
    const { subject } = await cells();
    for (const cell of subject) {
      expect(getComputedStyle(cell).paddingLeft).toBe("0px");
      expect(getComputedStyle(cell).paddingRight).toBe("0px");
    }
  });
});
