// @vitest-environment jsdom
//
// The wide sheet: its box is a keyboard stop, named, only while it overflows either way (the arrow
// keys then scroll it); the table is named; pinned columns hold at summed offsets and their first
// cell names the row; a folded group is a stub that still names itself; the action column sits
// at the right edge; a row's status is the first cell's stripe. jsdom lays nothing out, so the
// box's sizes are set as a browser would report them.

import { afterEach, describe, expect, it } from "vitest";
import { ui } from "../recipes";
import { $, $$, render } from "../ui.harness";
import { DataSheet, type PinnedColumn, type SheetGroup } from "./DataSheet";

const SIZES = ["scrollWidth", "clientWidth", "scrollHeight", "clientHeight"] as const;
afterEach(() => {
  document.body.innerHTML = "";
  for (const name of SIZES)
    delete (HTMLElement.prototype as unknown as Record<string, unknown>)[name];
});

function sizes(across: [number, number], down: [number, number]) {
  const values = [...across, ...down];
  SIZES.forEach((name, i) => {
    Object.defineProperty(HTMLElement.prototype, name, {
      configurable: true,
      get: () => values[i]
    });
  });
}

interface Item {
  sku: string;
  name: string;
  onHand: number;
  sold: number;
  tone?: "warning" | "error" | "neutral";
}
const ROWS: Item[] = [
  { sku: "A-1", name: "Rice", onHand: 12, sold: 40, tone: "warning" },
  { sku: "B-2", name: "Salmon", onHand: 3, sold: 18, tone: "neutral" }
];
const PINNED: PinnedColumn<Item>[] = [
  { key: "sku", label: "SKU", width: "6rem", render: (r) => r.sku },
  { key: "name", label: "Item", width: "10rem", render: (r) => r.name }
];
const GROUPS: SheetGroup<Item>[] = [
  {
    id: "stock",
    label: "Stock",
    short: "Stk",
    columns: [{ key: "onHand", label: "On hand", numeric: true, render: (r) => r.onHand }]
  },
  {
    id: "sales",
    label: "Sales",
    short: "Sls",
    columns: [{ key: "sold", label: "Sold", numeric: true, render: (r) => r.sold }]
  }
];

const sheet = (hidden?: ReadonlySet<string>) => (
  <DataSheet
    label='Reorder sheet'
    rows={ROWS}
    rowKey={(r) => r.sku}
    pinned={PINNED}
    groups={GROUPS}
    hidden={hidden}
    action={{ label: "Order", render: (r) => <button type='button'>Order {r.name}</button> }}
    rowTone={(r) => r.tone}
  />
);
const box = () => $(".oxy-ui-data-sheet") as HTMLElement;

describe("DataSheet keyboard scroll focus", () => {
  it("is a named Tab stop while it overflows across", async () => {
    sizes([1400, 600], [300, 300]);
    await render(sheet());
    expect(box().getAttribute("tabindex")).toBe("0");
    expect(box().getAttribute("role")).toBe("region");
    expect(box().getAttribute("aria-label")).toBe("Reorder sheet");
    expect(box().className).toContain(ui.focusRing);
    box().focus();
    expect(document.activeElement).toBe(box());
  });

  it("is a Tab stop while it overflows down, and none at all while everything fits", async () => {
    sizes([600, 600], [900, 480]);
    const page = await render(sheet());
    expect(box().getAttribute("tabindex")).toBe("0");
    await page.unmount();
    sizes([600, 600], [300, 300]);
    await render(sheet());
    expect(box().hasAttribute("tabindex")).toBe(false);
    expect(box().hasAttribute("role")).toBe(false);
    expect(box().hasAttribute("aria-label")).toBe(false);
  });
});

describe("DataSheet", () => {
  it("names its table, bands its groups and heads every column", async () => {
    await render(sheet());
    expect($("table")?.getAttribute("aria-label")).toBe("Reorder sheet");
    const band = $$("thead tr")[0];
    expect(band.className).toBe(ui.sheetBand);
    expect([...band.children].map((c) => c.textContent)).toEqual(["", "Stock", "Sales", ""]);
    expect((band.children[0] as HTMLTableCellElement).colSpan).toBe(2);
    expect($$("thead tr")[1].className).toBe(ui.sheetHeader);
    expect($$("thead tr")[1].querySelectorAll("th")).toHaveLength(5);
    expect($$("tbody tr").every((tr) => tr.className === ui.sheetRow)).toBe(true);
  });

  it("pins identity columns at summed fixed widths, the first naming the row", async () => {
    await render(sheet());
    const [sku, name] = [...($("tbody tr") as HTMLElement).children] as HTMLElement[];
    expect(sku.tagName).toBe("TH");
    expect(sku.getAttribute("scope")).toBe("row");
    expect(sku.className).toContain("oxy-ui-data-sheet__pin--first");
    expect(sku.style.getPropertyValue("--oxy-ui-pin-left")).toBe("0px");
    expect(sku.style.width).toBe("6rem");
    expect(name.tagName).toBe("TD");
    expect(name.className).toContain("oxy-ui-data-sheet__pin--last");
    expect(name.style.getPropertyValue("--oxy-ui-pin-left")).toBe("calc(6rem)");
  });

  it("offsets each pinned column by the sum of every width before it", async () => {
    const three: PinnedColumn<Item>[] = [
      ...PINNED,
      { key: "unit", label: "Unit", width: "4rem", render: () => "case" }
    ];
    await render(
      <DataSheet label='Sheet' rows={ROWS} rowKey={(r) => r.sku} pinned={three} groups={GROUPS} />
    );
    const cells = [...($("tbody tr") as HTMLElement).children].slice(0, 3) as HTMLElement[];
    expect(cells.map((c) => c.style.getPropertyValue("--oxy-ui-pin-left"))).toEqual([
      "0px",
      "calc(6rem)",
      "calc(6rem + 10rem)"
    ]);
    expect(cells[2].className).toContain("oxy-ui-data-sheet__pin--last");
    expect(cells[1].className).not.toContain("oxy-ui-data-sheet__pin--last");
    expect(($("thead tr th") as HTMLTableCellElement).colSpan).toBe(3);
  });

  it("folds a group to a stub that still names it", async () => {
    await render(sheet(new Set(["sales"])));
    const band = $$("thead tr")[0];
    expect([...band.children].map((c) => c.textContent)).toEqual(["", "Stock", "Sls", ""]);
    expect(band.children[2].className).toContain("oxy-ui-data-sheet__band--folded");
    const stub = $$("thead tr")[1].children[3] as HTMLElement;
    expect(stub.getAttribute("aria-label")).toBe("Sales (folded)");
    expect($("tbody tr")?.textContent).not.toContain("40");
    expect($$("tbody .oxy-ui-data-sheet__cell--folded")).toHaveLength(2);
  });

  it("pins the action column right and draws each row's status as its first cell's stripe", async () => {
    await render(sheet());
    const first = $("tbody tr") as HTMLElement;
    const action = first.lastElementChild as HTMLElement;
    expect(action.className).toContain("oxy-ui-data-sheet__action");
    expect(action.querySelector("button")?.textContent).toBe("Order Rice");
    expect($$("thead tr")[1].lastElementChild?.textContent).toBe("Order");
    expect((first.children[0] as HTMLElement).dataset.tone).toBe("warning");
    // Neutral keeps the stripe's 3px, drawn clear, so its row lines up with toned rows (the
    // reference's `border-l-transparent`, SDK-GAPS 5a); a row with no tone has no stripe.
    expect(($$("tbody tr")[1].children[0] as HTMLElement).dataset.tone).toBe("neutral");
    expect((first.children[2] as HTMLElement).className).toContain("oxy-ui-data-sheet__cell--num");
  });
});
