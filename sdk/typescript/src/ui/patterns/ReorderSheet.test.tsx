// @vitest-environment jsdom
//
// ReorderSheet: the frame lays out its slots (find, actions, the one primary action, the column
// toggles, the sheet, a footer); the toggles fold and unfold their column groups; the selection
// feeds the primary action, which keeps focus when a commit clears it; the find filters the rows
// and, when nothing matches, says so with a Clear that puts focus back in the field. Each piece of
// state can be the caller's.

import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { $, $$, click, fire, render } from "../ui.harness";
import { ReorderSheet, type ReorderSheetProps } from "./ReorderSheet";

afterEach(() => {
  document.body.innerHTML = "";
});

interface Row {
  sku: string;
  name: string;
  onHand: number;
  order: number;
}

const ROWS: Row[] = [
  { sku: "A-100", name: "Steel bracket", onHand: 40, order: 12 },
  { sku: "B-200", name: "Rubber seal", onHand: 5, order: 30 },
  { sku: "C-300", name: "Copper pipe", onHand: 90, order: 0 }
];

const base: ReorderSheetProps<Row> = {
  label: "Order sheet",
  rows: ROWS,
  rowKey: (r) => r.sku,
  pinned: [{ key: "sku", label: "SKU", width: "7rem", render: (r) => r.sku }],
  groups: [
    {
      id: "stock",
      label: "Stock",
      short: "Stk",
      columns: [{ key: "on", label: "On hand", numeric: true, render: (r) => r.onHand }]
    },
    {
      id: "order",
      label: "Reorder",
      short: "Ord",
      columns: [{ key: "qty", label: "Qty", numeric: true, render: (r) => r.order }]
    }
  ],
  search: { label: "Search SKU or item", text: (r) => [r.sku, r.name] },
  selectable: (r) => r.order > 0,
  commit: { label: (n) => `Draft order · ${n}` }
};

const draft = (n: number) => `Draft order · ${n}`;

/** The folded groups held by the screen. */
function HeldHidden() {
  const [hidden, setHidden] = useState<ReadonlySet<string>>(new Set(["order"]));
  return <ReorderSheet {...base} hidden={hidden} onHiddenChange={setHidden} />;
}

/** The selection held by the screen, which clears it on commit. */
function HeldSelection() {
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set(["A-100"]));
  return (
    <ReorderSheet
      {...base}
      selected={selected}
      onSelectedChange={setSelected}
      commit={{ label: draft, onCommit: () => setSelected(new Set()) }}
    />
  );
}

const input = () => $("input[type='search']") as HTMLInputElement;
const commitButton = () =>
  $$(".oxy-ui-btn--primary").find((b) => b.textContent?.startsWith("Draft")) as HTMLButtonElement;
const chip = (name: string) =>
  $$(".oxy-ui-toggle-chip").find((c) => c.textContent === name) as HTMLButtonElement;
const box = (sku: string) => $(`input[aria-label='Select ${sku}']`) as HTMLInputElement;

async function type(value: string) {
  const el = input();
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  setter?.call(el, value);
  await fire(el, new Event("input", { bubbles: true }));
}

describe("ReorderSheet", () => {
  it("lays out the find, the actions, the primary action, the toggles and the sheet", async () => {
    await render(
      <ReorderSheet
        {...base}
        actions={<button type='button'>Export CSV</button>}
        footer='Prices exclude freight.'
      />
    );
    const toolbar = $(".oxy-ui-reorder-sheet__toolbar") as HTMLElement;
    expect(toolbar.querySelector("input[type='search']")?.getAttribute("aria-label")).toBe(
      "Search SKU or item"
    );
    const actions = toolbar.querySelector(".oxy-ui-reorder-sheet__actions") as HTMLElement;
    expect([...actions.querySelectorAll("button")].map((b) => b.textContent)).toEqual([
      "Export CSV",
      "Draft order · 0"
    ]);
    expect($("[role='group']")?.getAttribute("aria-label")).toBe("Columns");
    expect($$(".oxy-ui-toggle-chip").map((c) => c.textContent)).toEqual(["Stock", "Reorder"]);
    expect($("table")?.getAttribute("aria-label")).toBe("Order sheet");
    expect($$("tbody tr")).toHaveLength(3);
    expect($(".oxy-ui-reorder-sheet__footer")?.textContent).toBe("Prices exclude freight.");
  });

  it("folds a column group to its stub with its toggle, and unfolds it again", async () => {
    const seen: string[][] = [];
    await render(<ReorderSheet {...base} onHiddenChange={(h) => seen.push([...h])} />);
    expect(chip("Stock").getAttribute("aria-pressed")).toBe("true");
    expect($$("thead th").some((th) => th.textContent === "On hand")).toBe(true);

    await click(chip("Stock"));
    expect(chip("Stock").getAttribute("aria-pressed")).toBe("false");
    expect($$("thead th").some((th) => th.textContent === "On hand")).toBe(false);
    expect($("th[aria-label='Stock (folded)']")).not.toBeNull();
    expect($$("thead th").some((th) => th.textContent === "Stk")).toBe(true);
    expect(chip("Reorder").getAttribute("aria-pressed")).toBe("true");

    await click(chip("Stock"));
    expect(chip("Stock").getAttribute("aria-pressed")).toBe("true");
    expect($$("thead th").some((th) => th.textContent === "On hand")).toBe(true);
    expect(seen).toEqual([["stock"], []]);
  });

  it("takes the folded groups from the caller when it holds them", async () => {
    await render(<HeldHidden />);
    expect(chip("Reorder").getAttribute("aria-pressed")).toBe("false");
    await click(chip("Reorder"));
    expect(chip("Reorder").getAttribute("aria-pressed")).toBe("true");
  });

  it("picks only the rows that can be picked, and the count feeds the primary action", async () => {
    const onCommit = vi.fn();
    await render(<ReorderSheet {...base} commit={{ label: draft, onCommit }} />);
    expect(box("A-100")).not.toBeNull();
    expect(box("B-200")).not.toBeNull();
    expect(box("C-300")).toBeNull();
    expect($$(".oxy-ui-reorder-sheet__dash")).toHaveLength(1);
    expect($$("thead th").some((th) => th.textContent === "Select")).toBe(true);

    // Nothing picked: the action says so and does nothing.
    expect(commitButton().getAttribute("aria-disabled")).toBe("true");
    expect(commitButton().disabled).toBe(false);
    await click(commitButton());
    expect(onCommit).not.toHaveBeenCalled();

    await click(box("A-100"));
    await click(box("B-200"));
    expect(commitButton().textContent).toBe("Draft order · 2");
    expect(commitButton().hasAttribute("aria-disabled")).toBe(false);
    await click(box("A-100"));
    expect(commitButton().textContent).toBe("Draft order · 1");
    await click(commitButton());
    expect(onCommit).toHaveBeenCalledWith(["B-200"]);
  });

  it("keeps focus on the primary action when the commit clears the selection", async () => {
    await render(<HeldSelection />);
    const button = commitButton();
    button.focus();
    await click(button);
    expect(button.textContent).toBe("Draft order · 0");
    expect(button.getAttribute("aria-disabled")).toBe("true");
    expect(document.activeElement).toBe(button);
  });

  it("filters the rows as the find is typed, by every word", async () => {
    await render(<ReorderSheet {...base} />);
    await type("seal");
    expect($$("tbody tr").map((r) => r.querySelector("th")?.textContent)).toEqual(["B-200"]);
    await type("c-3 copper");
    expect($$("tbody tr").map((r) => r.querySelector("th")?.textContent)).toEqual(["C-300"]);
  });

  it("says when nothing matches, and Clear brings the rows and focus back", async () => {
    await render(<ReorderSheet {...base} noMatch={(q) => `No item matches “${q}”.`} />);
    await type("  gasket ");
    expect($("table")).toBeNull();
    const empty = $(".oxy-ui-reorder-sheet__empty") as HTMLElement;
    expect(empty.getAttribute("role")).toBe("status");
    expect(empty.textContent).toContain("No item matches “gasket”.");
    const clear = [...empty.querySelectorAll("button")].find(
      (b) => b.textContent === "Clear search"
    );
    await click(clear ?? null);
    expect(input().value).toBe("");
    expect($$("tbody tr")).toHaveLength(3);
    expect(document.activeElement).toBe(input());
  });

  it("hands the footer the picked keys, and draws no selection column without `selectable`", async () => {
    const page = await render(
      <ReorderSheet {...base} footer={(keys) => `Picked: ${keys.join(", ") || "none"}`} />
    );
    expect($(".oxy-ui-reorder-sheet__footer")?.textContent).toBe("Picked: none");
    await click(box("B-200"));
    expect($(".oxy-ui-reorder-sheet__footer")?.textContent).toBe("Picked: B-200");
    await page.unmount();

    await render(<ReorderSheet {...base} selectable={undefined} search={undefined} />);
    expect($$("input[type='checkbox']")).toHaveLength(0);
    expect($$("thead th").some((th) => th.textContent === "Select")).toBe(false);
    expect($("input[type='search']")).toBeNull();
  });
});
