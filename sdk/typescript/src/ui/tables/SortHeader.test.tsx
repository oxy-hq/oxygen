// @vitest-environment jsdom
//
// A sortable heading as a reader meets it: a real <button> in a <th> whose `aria-sort` follows the
// three-state cycle (first direction, the other, back to the table's own order), a title that
// says what the next press does, an arrow that is always drawn, and the rows it reorders.

import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { $, $$, click, render } from "../ui.harness";
import { SortHeader } from "./SortHeader";
import { nextSort, type SortState, sortRows } from "./tableSort";

afterEach(() => {
  document.body.innerHTML = "";
});

type Key = "store" | "cost";
const ROWS = [
  { store: "North", cost: 12 as number | null },
  { store: "Harbour", cost: null },
  { store: "East", cost: 30 }
];

function Table() {
  const [sort, setSort] = useState<SortState<Key> | null>(null);
  const onSort = (key: Key, firstDir: "asc" | "desc") => setSort(nextSort(sort, key, firstDir));
  const rows = sort ? sortRows(ROWS, (r) => r[sort.key], sort.dir) : ROWS;
  return (
    <table>
      <thead>
        <tr>
          <SortHeader sortKey='store' label='store' firstDir='asc' sort={sort} onSort={onSort}>
            Store
          </SortHeader>
          <SortHeader
            sortKey='cost'
            label='food cost'
            firstDir='desc'
            sort={sort}
            onSort={onSort}
            align='right'
          >
            Food cost
          </SortHeader>
        </tr>
      </thead>
      <tbody>
        {rows.map((r) => (
          <tr key={r.store}>
            <td>{r.store}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

const headers = () => $$("th");
const sortOf = () => headers().map((th) => th.getAttribute("aria-sort"));
const order = () => $$("tbody td").map((td) => td.textContent);
const button = (i: number) => headers()[i].querySelector("button") as HTMLButtonElement;

describe("SortHeader", () => {
  it("is a column header holding a real button, no column sorted at first", async () => {
    await render(<Table />);
    expect(headers().every((th) => th.getAttribute("scope") === "col")).toBe(true);
    expect(sortOf()).toEqual(["none", "none"]);
    const b = button(1);
    expect(b.tagName).toBe("BUTTON");
    expect(b.type).toBe("button");
    expect(b.textContent).toBe("Food cost");
    b.focus();
    expect(document.activeElement).toBe(b);
    expect(b.title).toBe("Sort by food cost");
  });

  it("cycles first direction, the other, then back to the table's own order", async () => {
    await render(<Table />);
    expect(order()).toEqual(["North", "Harbour", "East"]);

    await click(button(1));
    expect(sortOf()).toEqual(["none", "descending"]);
    expect(button(1).title).toBe("Sort by food cost, the other way");
    // Biggest first; the store with no figure stays last.
    expect(order()).toEqual(["East", "North", "Harbour"]);

    await click(button(1));
    expect(sortOf()).toEqual(["none", "ascending"]);
    expect(button(1).title).toBe("Back to the default order");
    expect(order()).toEqual(["North", "East", "Harbour"]);

    await click(button(1));
    expect(sortOf()).toEqual(["none", "none"]);
    expect(button(1).title).toBe("Sort by food cost");
    expect(order()).toEqual(["North", "Harbour", "East"]);
  });

  it("starts another column at its own first direction", async () => {
    await render(<Table />);
    await click(button(1));
    await click(button(0));
    expect(sortOf()).toEqual(["ascending", "none"]);
    expect(order()).toEqual(["East", "Harbour", "North"]);
  });

  it("always draws the arrow, shown only on the sorted column, pointing its way", async () => {
    await render(<Table />);
    const arrow = (i: number) => headers()[i].querySelector(".oxy-ui-sort-header__arrow");
    expect(arrow(0)).not.toBeNull();
    expect(arrow(0)?.hasAttribute("data-on")).toBe(false);
    expect(arrow(0)?.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
    await click(button(1));
    expect(arrow(1)?.hasAttribute("data-on")).toBe(true);
    const down = arrow(1)?.querySelector("path")?.getAttribute("d");
    await click(button(1));
    expect(arrow(1)?.querySelector("path")?.getAttribute("d")).not.toBe(down);
  });

  it("reports the press with its key and first direction, takes the th's props and aligns", async () => {
    const onSort = vi.fn();
    await render(
      <table>
        <thead>
          <tr>
            <SortHeader
              sortKey='cost'
              label='cost'
              firstDir='desc'
              sort={{ key: "cost", dir: "desc" }}
              onSort={onSort}
              align='right'
              className='mine'
              id='cost-head'
            >
              Cost
            </SortHeader>
          </tr>
        </thead>
      </table>
    );
    await click($("button"));
    expect(onSort).toHaveBeenCalledWith("cost", "desc");
    const th = $("th") as HTMLElement;
    expect(th.id).toBe("cost-head");
    expect(th.className).toContain("mine");
    expect(th.className).toContain("oxy-ui-sort-header--right");
    expect(th.className).toContain("oxy-ui-eyebrow");
    expect($("button")?.className).toContain("oxy-ui-focus");
  });
});
