// @vitest-environment jsdom
//
// The link list: one link per row on a shared track, the current row marked for the eye and for
// a screen reader, the dropped columns flagged for the container query, and a router-free press
// that a modified press (a new tab) leaves to the browser.

import { afterEach, describe, expect, it, vi } from "vitest";
import { ui } from "../recipes";
import { $, $$, click, fire, render } from "../ui.harness";
import { LinkList, type LinkListColumn } from "./LinkList";

afterEach(() => {
  document.body.innerHTML = "";
});

interface Report {
  id: string;
  date: string;
  by: string;
}
const ROWS: Report[] = [
  { id: "R-1", date: "Jul 16", by: "Sam" },
  { id: "R-2", date: "Jul 17", by: "Ana" }
];
const COLUMNS: LinkListColumn<Report>[] = [
  { width: "5rem", cell: (r) => r.id },
  { width: "minmax(0,1fr)", cell: (r) => r.date, truncate: true },
  { width: "6rem", cell: (r) => r.by, wide: true, align: "right" }
];

describe("LinkList", () => {
  it("is a named list of links, one per row, with the current one marked", async () => {
    await render(
      <LinkList
        label='Filed reports'
        rows={ROWS}
        rowKey={(r) => r.id}
        columns={COLUMNS}
        href={(r) => `/reports/${r.id}`}
        isCurrent={(r) => r.id === "R-2"}
      />
    );
    const list = $("ul") as HTMLElement;
    expect(list.getAttribute("aria-label")).toBe("Filed reports");
    expect(list.className).toContain(ui.card);
    const links = $$("li > a") as HTMLAnchorElement[];
    expect(links.map((a) => a.getAttribute("href"))).toEqual(["/reports/R-1", "/reports/R-2"]);
    expect(links[1].getAttribute("aria-current")).toBe("page");
    expect(links[0].hasAttribute("aria-current")).toBe(false);
    expect(links[1].className).toContain(ui.rowSelected);
    expect(links[0].className).toContain(ui.rowHover);
    expect(links[0].className).toContain(ui.focusRingInset);
    expect(links[0].textContent).toBe("R-1Jul 16Sam");
  });

  it("hands the stylesheet both tracks and flags the columns that drop when narrow", async () => {
    await render(
      <LinkList rows={ROWS} rowKey={(r) => r.id} columns={COLUMNS} href={(r) => `#${r.id}`} />
    );
    const list = $("ul") as HTMLElement;
    expect(list.style.getPropertyValue("--oxy-ui-link-list-full")).toBe("5rem minmax(0,1fr) 6rem");
    expect(list.style.getPropertyValue("--oxy-ui-link-list-compact")).toBe("5rem minmax(0,1fr)");
    const cells = [...($("a") as HTMLElement).children];
    expect(cells[2].className).toContain("oxy-ui-link-list__cell--wide");
    expect(cells[2].className).toContain("oxy-ui-link-list__cell--num");
    expect(cells[1].className).toContain("oxy-ui-link-list__cell--truncate");
    expect(cells[0].className).not.toContain("--wide");
  });

  it("navigates through onNavigate on a plain press, and lets a modified press through", async () => {
    const onNavigate = vi.fn();
    await render(
      <LinkList
        rows={ROWS}
        rowKey={(r) => r.id}
        columns={COLUMNS}
        href={(r) => `#/reports/${r.id}`}
        onNavigate={onNavigate}
      />
    );
    const link = $$("a")[1];
    await click(link);
    expect(onNavigate).toHaveBeenCalledWith("#/reports/R-2", ROWS[1]);
    const newTab = new MouseEvent("click", { bubbles: true, cancelable: true, metaKey: true });
    await fire(link, newTab);
    expect(onNavigate).toHaveBeenCalledTimes(1);
    expect(newTab.defaultPrevented).toBe(false);
  });
});
