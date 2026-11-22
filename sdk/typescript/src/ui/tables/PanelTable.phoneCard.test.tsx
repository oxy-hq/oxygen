// @vitest-environment jsdom
//
// 2.32.0 PanelTable `phoneCard` and `rowProps` (decision 99, Quadrogen's `PhoneRows` and its
// `rowProps`), driven as a reader drives them: on a phone layout each row is a card in the tile's
// fill and edge, the title with the status and the chevron on its first line, the subtitle on its
// own line, every figure (no cap) with a sentence-long one across the card; a card that opens is
// pressed anywhere on it, its title the one Tab stop; per-row attributes on the desk row and on the
// card. Without `phoneCard` a phone still gets the table (keptPieces.defaults.test.tsx).

import { afterAll, afterEach, describe, expect, it, vi } from "vitest";
import { paintUi, rgb, themeValue, unpaintUi } from "../paint.harness";
import { $, $$, click, render, stubMatchMedia } from "../ui.harness";
import { type PanelColumn, PanelTable, type PanelTableProps } from "./PanelTable";

afterEach(() => {
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
});
afterAll(() => unpaintUi());

interface Plant {
  id: string;
  name: string;
  status: string;
  region: string;
  purity: string;
  gap: string;
  kwh: string;
  alarms: string;
  action: string;
}
const plant = (n: number): Plant => ({
  id: `p${n}`,
  name: `Plant 0${n}`,
  status: n === 1 ? "Needs action" : "On track",
  region: "US West",
  purity: "93.57",
  gap: "-4.01",
  kwh: "0.31",
  alarms: "5",
  action: "Swap the membrane before the next shift."
});
const ROWS = [plant(1), plant(2)];
const COLUMNS: PanelColumn<Plant>[] = [
  { header: "Status", cell: (p) => <span data-status=''>{p.status}</span> },
  { header: "Plant", cell: (p) => p.name, className: "plant-name" },
  { header: "Region", cell: (p) => p.region },
  { header: "Purity %", align: "right", cell: (p) => p.purity },
  {
    header: "Gap",
    align: "right",
    cell: (p) => p.gap,
    className: (p) => (p.id === "p1" ? "bad" : undefined)
  },
  { header: "kWh/Nm3", align: "right", cell: (p) => p.kwh },
  { header: "Alarms 30d", align: "right", cell: (p) => p.alarms },
  { header: "Action", phoneWide: true, cell: (p) => p.action }
];

async function mount(props: Partial<PanelTableProps<Plant>>, phone = true) {
  vi.stubGlobal("matchMedia", stubMatchMedia(phone).matchMedia);
  await render(
    <PanelTable
      caption='Plants'
      columns={COLUMNS}
      rows={ROWS}
      rowKey={(p) => p.id}
      phoneCard={{ title: 1, aside: 0, subtitle: (p) => `${p.region} · dairy` }}
      {...props}
    />
  );
}

const cards = () => $$(".oxy-ui-panel-table__card");

describe("PanelTable phoneCard", () => {
  it("draws a card per row on a phone, in a list named by the caption, and no table", async () => {
    await mount({});
    expect($("table")).toBeNull();
    const list = $("ul.oxy-ui-panel-table__cards");
    expect(list?.getAttribute("aria-labelledby")).toBe($("h3")?.id);
    expect(cards()).toHaveLength(2);
  });

  it("keeps the table on a desk", async () => {
    await mount({}, false);
    expect($("table")).not.toBeNull();
    expect(cards()).toHaveLength(0);
  });

  it("puts the title, then the status and the chevron, on the first line, and the subtitle under it", async () => {
    await mount({ onRowOpen: () => {} });
    const card = cards()[0];
    const head = card.querySelector(".oxy-ui-panel-table__card-head");
    expect(head?.children[0].textContent).toBe("Plant 01");
    expect(head?.children[0].className).toContain("plant-name");
    const trailing = head?.querySelector(".oxy-ui-panel-table__card-trailing");
    expect(trailing?.children[0].hasAttribute("data-status")).toBe(true);
    expect(trailing?.children[1].tagName.toLowerCase()).toBe("svg");
    expect(trailing?.children[1].getAttribute("aria-hidden")).toBe("true");
    expect(card.querySelector(".oxy-ui-panel-table__card-subtitle")?.textContent).toBe(
      "US West · dairy"
    );
  });

  it("draws every other column as a figure, with no cap, a phoneWide one across the card", async () => {
    await mount({});
    const figures = [...cards()[0].querySelectorAll(".oxy-ui-panel-table__card-figure")];
    expect(figures.map((f) => f.querySelector("dt")?.textContent)).toEqual([
      "Region",
      "Purity %",
      "Gap",
      "kWh/Nm3",
      "Alarms 30d",
      "Action"
    ]);
    expect(figures[2].querySelector("dd")?.className).toContain("bad");
    expect(figures[5].className).toContain("oxy-ui-panel-table__card-figure--wide");
    expect(figures[4].className).not.toContain("--wide");
  });

  it("draws only the chosen figures, in their order, and a value alone under an empty header", async () => {
    const cols = COLUMNS.map((c, j) => (j === 6 ? { ...c, header: "" } : c));
    await mount({ columns: cols, phoneCard: { title: 1, aside: 0, figures: [6, 3] } });
    const figures = [...cards()[0].querySelectorAll(".oxy-ui-panel-table__card-figure")];
    expect(figures).toHaveLength(2);
    expect(figures[0].querySelector("dt")).toBeNull();
    expect(figures[0].textContent).toBe("5");
    expect(figures[1].textContent).toBe("Purity %93.57");
  });

  it("opens from a press anywhere on the card, once, its title the button the keyboard reaches", async () => {
    const opened: string[] = [];
    await mount({ onRowOpen: (p) => opened.push(p.id), isSelected: (p) => p.id === "p2" });
    const buttons = $$("button");
    expect(buttons.map((b) => b.textContent)).toEqual(["Plant 01", "Plant 02"]);
    expect(buttons[1].getAttribute("aria-current")).toBe("true");
    expect(buttons[0].getAttribute("aria-current")).toBeNull();
    await click(cards()[0].querySelector(".oxy-ui-panel-table__card-figures"));
    await click(buttons[1]);
    expect(opened).toEqual(["p1", "p2"]);
    expect(cards()[0].className).toContain("oxy-ui-panel-table__card--open");
  });

  it("draws no button and no chevron for a card that does not open", async () => {
    await mount({});
    expect($$("button")).toHaveLength(0);
    expect($$(".oxy-ui-panel-table__card svg")).toHaveLength(0);
  });

  it("keeps an actions press to the actions", async () => {
    const opened: string[] = [];
    const acted: string[] = [];
    await mount({
      onRowOpen: (p) => opened.push(p.id),
      actions: (p) => (
        <button type='button' onClick={() => acted.push(p.id)}>
          Post
        </button>
      )
    });
    await click($$(".oxy-ui-panel-table__card-actions button")[0]);
    expect(acted).toEqual(["p1"]);
    expect(opened).toEqual([]);
  });

  it("draws a plain-text aside in full ink, the chevron muted, the figures dim", async () => {
    await mount({
      columns: COLUMNS.map((c, j) => (j === 0 ? { ...c, cell: (p: Plant) => p.status } : c)),
      onRowOpen: () => {}
    });
    for (const mode of ["light", "dark"] as const) {
      paintUi({ mode });
      const card = cards()[0];
      const trailing = card.querySelector(".oxy-ui-panel-table__card-trailing") as HTMLElement;
      expect(rgb(getComputedStyle(trailing).color), mode).toBe(
        rgb(themeValue("--foreground", mode))
      );
      const chevron = card.querySelector(".oxy-ui-panel-table__card-chevron") as Element;
      expect(rgb(getComputedStyle(chevron).color), mode).toBe(
        rgb(themeValue("--muted-foreground", mode))
      );
      const value = card.querySelector(".oxy-ui-panel-table__card-value") as HTMLElement;
      expect(rgb(getComputedStyle(value).color), mode).toBe(
        rgb(themeValue("--dim-foreground", mode))
      );
    }
  });

  it("takes the tile's fill and edge, not the phone kit's grey", async () => {
    await mount({});
    for (const mode of ["light", "dark"] as const) {
      paintUi({ mode });
      const style = getComputedStyle(cards()[0]);
      expect(rgb(style.backgroundColor), mode).toBe(rgb(themeValue("--field", mode)));
      expect(style.borderTopWidth, mode).toBe("1px");
      expect(rgb(style.borderTopColor), mode).toBe(rgb(themeValue("--border-subtle", mode)));
      expect(style.paddingLeft, mode).toBe("12px");
      expect(style.paddingTop, mode).toBe("8px");
    }
  });
});

describe("PanelTable rowProps", () => {
  const attrs = (p: Plant) => ({
    "data-unit": p.id,
    "data-selected": p.id === "p1" ? "true" : "false",
    title: "Open plant",
    tabIndex: p.id === "p1" ? 0 : undefined,
    className: p.id === "p1" ? "spot" : undefined
  });

  it("spreads each row's attributes on the desk row, its class beside the table's own", async () => {
    await mount({ rowProps: attrs, rowClassName: () => "own" }, false);
    const rows = $$("tbody tr");
    expect(rows.map((r) => r.getAttribute("data-unit"))).toEqual(["p1", "p2"]);
    expect(rows.map((r) => r.getAttribute("title"))).toEqual(["Open plant", "Open plant"]);
    expect(rows.map((r) => r.getAttribute("tabindex"))).toEqual(["0", null]);
    expect(rows[0].className).toContain("oxy-ui-panel-table__row");
    expect(rows[0].className).toContain("own");
    expect(rows[0].className).toContain("spot");
    expect(rows[1].className).not.toContain("spot");
  });

  it("makes a desk row's onClick the row's own when the table does not open rows", async () => {
    const pressed: string[] = [];
    await mount({ rowProps: (p) => ({ onClick: () => pressed.push(p.id) }) }, false);
    await click($$("tbody tr")[1]);
    expect(pressed).toEqual(["p2"]);
    // No opener button: the row's own handler is not the table's opening.
    expect($$("tbody button")).toHaveLength(0);
  });

  it("puts data-* and the class on the card, leaves the desk's title and tabIndex behind", async () => {
    await mount({ rowProps: attrs });
    const [first] = cards();
    expect(first.getAttribute("data-unit")).toBe("p1");
    expect(first.getAttribute("data-selected")).toBe("true");
    expect(first.className).toContain("spot");
    expect(first.getAttribute("title")).toBeNull();
    expect(first.getAttribute("tabindex")).toBeNull();
  });

  it("opens a card from the row's onClick when the table has no onRowOpen", async () => {
    const pressed: string[] = [];
    await mount({ rowProps: (p) => ({ onClick: () => pressed.push(p.id) }) });
    expect($$("button").map((b) => b.textContent)).toEqual(["Plant 01", "Plant 02"]);
    await click($$("button")[0]);
    expect(pressed).toEqual(["p1"]);
  });
});
