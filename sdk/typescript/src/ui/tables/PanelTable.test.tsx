// @vitest-environment jsdom
//
// The panel table frame, driven as a reader drives it: the columns and rows it is given under a
// caption that names it; the row's name as a row header; figures set right; the lead and the
// footnote; a row that opens from a press anywhere on it or from its name's button, without its
// actions cell opening it too; the selected row; a totals row; and rows that expand into child
// rows on the same columns, saying `aria-expanded`, one open at a time.

import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ui } from "../recipes";
import { $, $$, click, render } from "../ui.harness";
import { type PanelColumn, PanelTable } from "./PanelTable";

afterEach(() => {
  document.body.innerHTML = "";
});

interface Line {
  name: string;
  amount: number;
  parts?: Line[];
}
const LINES: Line[] = [
  {
    name: "Alpha",
    amount: 30,
    parts: [
      { name: "Alpha one", amount: 20 },
      { name: "Alpha two", amount: 10 }
    ]
  },
  { name: "Beta", amount: 12 }
];
const COLUMNS: PanelColumn<Line>[] = [
  { header: "Item", cell: (l) => l.name },
  {
    header: "Amount",
    align: "right",
    cell: (l) => `$${l.amount}`,
    className: (l) => (l.amount > 20 ? "big" : undefined),
    total: "$42"
  }
];

/** A caller that holds the open row itself, starting on the second. */
function Held({ seen }: { seen: (string | null)[] }) {
  const both: Line[] = [LINES[0], { ...LINES[0], name: "Gamma" }];
  const [open, setOpen] = useState<string | null>("Gamma");
  return (
    <PanelTable
      columns={COLUMNS}
      rows={both}
      rowKey={(l) => l.name}
      subRows={(l) => l.parts}
      expanded={open}
      onExpandedChange={(k) => {
        seen.push(k);
        setOpen(k);
      }}
    />
  );
}

const bodyRows = () => $$("tbody tr");
const texts = (sel: string) => $$(sel).map((e) => e.textContent);

describe("PanelTable", () => {
  it("renders the given columns and rows, named by its caption", async () => {
    await render(
      <PanelTable
        caption='What the lines were'
        lead='One line carried the month.'
        footnote='A further $3 came from smaller lines.'
        columns={COLUMNS}
        rows={LINES}
        rowKey={(l) => l.name}
      />
    );
    const table = $("table") as HTMLTableElement;
    const caption = $("h3") as HTMLElement;
    expect(caption.className).toContain(ui.eyebrow);
    expect(table.getAttribute("aria-labelledby")).toBe(caption.id);
    expect(caption.textContent).toBe("What the lines were");
    expect(texts("thead th")).toEqual(["Item", "Amount"]);
    expect($$("thead th").every((th) => th.getAttribute("scope") === "col")).toBe(true);
    expect(bodyRows()).toHaveLength(2);
    // The row's name is its header, so every cell after it is named by its row.
    const name = bodyRows()[0].children[0] as HTMLElement;
    expect(name.tagName).toBe("TH");
    expect(name.getAttribute("scope")).toBe("row");
    expect(texts("tbody td")).toEqual(["$30", "$12"]);
    const amount = bodyRows()[0].children[1] as HTMLElement;
    expect(amount.className).toContain("oxy-ui-panel-table__cell--num");
    expect(amount.className).toContain("big");
    expect((bodyRows()[1].children[1] as HTMLElement).className).not.toContain("big");
    expect(texts(".oxy-ui-panel-table__note")).toEqual([
      "One line carried the month.",
      "A further $3 came from smaller lines."
    ]);
    // Nothing to press: a plain table draws no buttons and no totals.
    expect($("tbody button")).toBeNull();
    expect($("tfoot")).toBeNull();
  });

  it("keeps rows as data cells when they are records, not names, and takes a label", async () => {
    await render(
      <PanelTable
        label='The comparison'
        rowHeader={false}
        columns={COLUMNS}
        rows={LINES}
        rowKey={(l) => l.name}
        rowClassName={(l) => (l.name === "Beta" ? "subject" : undefined)}
      />
    );
    expect($("h3")).toBeNull();
    expect($("table")?.getAttribute("aria-label")).toBe("The comparison");
    expect($("tbody th")).toBeNull();
    expect(bodyRows()[1].className).toContain("subject");
  });

  it("opens a row from a press on it or on its name's button, never from its actions", async () => {
    const onRowOpen = vi.fn();
    const post = vi.fn();
    await render(
      <PanelTable
        columns={COLUMNS}
        rows={LINES}
        rowKey={(l) => l.name}
        onRowOpen={onRowOpen}
        isSelected={(l) => l.name === "Beta"}
        actions={(l) => (
          <button type='button' onClick={() => post(l.name)}>
            Post
          </button>
        )}
      />
    );
    // The actions column has a heading for assistive tech, and a cell on every row.
    expect(texts("thead th")).toEqual(["Item", "Amount", "Actions"]);
    expect($$(".oxy-ui-panel-table__cell--actions")).toHaveLength(2);

    const opener = bodyRows()[0].querySelector(".oxy-ui-panel-table__open") as HTMLButtonElement;
    expect(opener.tagName).toBe("BUTTON");
    expect(opener.textContent).toBe("Alpha");
    opener.focus();
    expect(document.activeElement).toBe(opener);
    await click(opener);
    expect(onRowOpen).toHaveBeenCalledTimes(1);
    expect(onRowOpen).toHaveBeenLastCalledWith(LINES[0]);

    await click(bodyRows()[1].children[1]);
    expect(onRowOpen).toHaveBeenCalledTimes(2);
    expect(onRowOpen).toHaveBeenLastCalledWith(LINES[1]);

    await click(bodyRows()[1].querySelector(".oxy-ui-panel-table__cell--actions button"));
    expect(post).toHaveBeenCalledWith("Beta");
    expect(onRowOpen).toHaveBeenCalledTimes(2);
  });

  it("gives the selected row the selected fill and aria-current, the rest the hover", async () => {
    await render(
      <PanelTable
        columns={COLUMNS}
        rows={LINES}
        rowKey={(l) => l.name}
        onRowOpen={() => {}}
        isSelected={(l) => l.name === "Beta"}
      />
    );
    const [alpha, beta] = bodyRows();
    expect(beta.className).toContain(ui.rowSelected);
    expect(beta.className).not.toContain(ui.rowHover);
    expect(alpha.className).toContain(ui.rowHover);
    expect(beta.querySelector("button")?.getAttribute("aria-current")).toBe("true");
    expect(alpha.querySelector("button")?.hasAttribute("aria-current")).toBe(false);
  });

  it("closes with a totals row: its label as the row's header, each column's total", async () => {
    await render(
      <PanelTable
        columns={COLUMNS}
        rows={LINES}
        rowKey={(l) => l.name}
        totalLabel='Total (2 lines)'
        actions={() => null}
      />
    );
    const total = $("tfoot tr") as HTMLElement;
    expect(total.className).toContain("oxy-ui-panel-table__totals");
    const [label, amount, actionsCell] = [...total.children] as HTMLElement[];
    expect(label.tagName).toBe("TH");
    expect(label.getAttribute("scope")).toBe("row");
    expect(label.textContent).toBe("Total (2 lines)");
    expect(amount.textContent).toBe("$42");
    expect(amount.className).toContain("oxy-ui-panel-table__cell--num");
    expect(actionsCell.textContent).toBe("");
    expect($("table")?.className).toContain("oxy-ui-panel-table__table--totals");
  });

  it("expands a row into child rows on the same columns, saying aria-expanded", async () => {
    await render(
      <PanelTable columns={COLUMNS} rows={LINES} rowKey={(l) => l.name} subRows={(l) => l.parts} />
    );
    const toggle = () => bodyRows()[0].querySelector("button") as HTMLButtonElement;
    // Only a row with child rows opens.
    expect(bodyRows()[1].querySelector("button")).toBeNull();
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    expect(bodyRows()).toHaveLength(2);

    await click(toggle());
    expect(toggle().getAttribute("aria-expanded")).toBe("true");
    expect(bodyRows().map((r) => r.children[0].textContent)).toEqual([
      "Alpha",
      "Alpha one",
      "Alpha two",
      "Beta"
    ]);
    const kids = $$(".oxy-ui-panel-table__sub-row");
    expect(kids).toHaveLength(2);
    expect(kids[1].hasAttribute("data-last")).toBe(true);
    expect(kids[0].children[0].className).toContain("oxy-ui-panel-table__cell--sub");
    expect(kids[0].children[1].textContent).toBe("$20");
    // The open row takes the selected fill; its children open nothing.
    expect(bodyRows()[0].className).toContain(ui.rowSelected);
    expect(kids[0].querySelector("button")).toBeNull();

    // A press anywhere on the open row closes it again.
    await click(bodyRows()[0].children[1]);
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    expect($$(".oxy-ui-panel-table__sub-row")).toHaveLength(0);
  });

  it("keeps one row open at a time, and can be held by its caller", async () => {
    const seen: (string | null)[] = [];
    await render(<Held seen={seen} />);
    const expandedOf = () => $$("tbody button").map((b) => b.getAttribute("aria-expanded"));
    expect(expandedOf()).toEqual(["false", "true"]);
    await click($$("tbody button")[0]);
    expect(seen).toEqual(["Alpha"]);
    expect(expandedOf()).toEqual(["true", "false"]);
  });
});
