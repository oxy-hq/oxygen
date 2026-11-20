// @vitest-environment jsdom
//
// CoverageRebalanceList: each row states its move (from, the connector, the destination and the
// result in the point colour) beside its own Apply toggle, described by the row's text; applying
// flips the row to the success tint and the button to secondary "Applied", and swaps the idle mark
// for a reset; the reset hands focus to the first row's Apply before it unmounts; an empty list
// says so; the footnote sits under the rows.

import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { $, $$, click, render } from "../ui.harness";
import { CoverageRebalanceList, type CoverageRebalanceListProps } from "./CoverageRebalanceList";

afterEach(() => {
  document.body.innerHTML = "";
});

interface Move {
  from: string;
  to: string;
  hours: number;
  saving: string;
}

const MOVES: Move[] = [
  { from: "East", to: "West", hours: 6, saving: "$240" },
  { from: "North", to: "South", hours: 4, saving: "$160" }
];
const key = (m: Move) => `${m.from}>${m.to}`;

const base: CoverageRebalanceListProps<Move> = {
  title: "Moves · Monday",
  recs: MOVES,
  recKey: key,
  from: (m) => m.from,
  to: (m) => m.to,
  detail: (m) => `lunch · move ${m.hours}h · save`,
  result: (m) => m.saving,
  applied: new Set(),
  onToggle: () => {},
  idle: <span data-idle=''>*</span>,
  foot: "Nothing is published."
};

function Held(props: Partial<CoverageRebalanceListProps<Move>>) {
  const [applied, setApplied] = useState<ReadonlySet<string>>(new Set());
  return (
    <CoverageRebalanceList
      {...base}
      applied={applied}
      onToggle={(k) =>
        setApplied((cur) => {
          const next = new Set(cur);
          if (next.has(k)) next.delete(k);
          else next.add(k);
          return next;
        })
      }
      onReset={() => setApplied(new Set())}
      {...props}
    />
  );
}

const rows = () => $$(".oxy-ui-rebalance-list__row");
const apply = (i: number) => rows()[i].querySelector("button") as HTMLButtonElement;
const reset = () =>
  $$(".oxy-ui-panel-header button").find((b) => b.textContent === "Reset") as
    | HTMLButtonElement
    | undefined;

describe("CoverageRebalanceList", () => {
  it("states each move in its row, beside an Apply described by that row", async () => {
    await render(<CoverageRebalanceList {...base} />);
    expect($(".oxy-ui-panel-header")?.textContent).toBe("Moves · Monday*");
    expect(rows()).toHaveLength(2);
    const [first] = rows();
    expect(first.querySelector(".oxy-ui-rebalance-list__from")?.textContent).toBe("East");
    expect(first.querySelector(".oxy-ui-rebalance-list__arrow")?.getAttribute("aria-hidden")).toBe(
      "true"
    );
    expect(first.querySelector(".oxy-ui-rebalance-list__to")?.textContent).toBe("West");
    expect(first.querySelector(".oxy-ui-rebalance-list__result")?.textContent).toBe("$240");
    const text = first.querySelector(".oxy-ui-rebalance-list__text") as HTMLElement;
    expect(text.textContent).toBe("EastWestlunch · move 6h · save $240");
    const button = apply(0);
    expect(button.textContent).toBe("Apply");
    expect(button.getAttribute("aria-pressed")).toBe("false");
    expect(button.classList).toContain("oxy-ui-btn--primary");
    expect(button.getAttribute("aria-describedby")).toBe(text.id);
    expect(text.id).not.toBe(rows()[1].querySelector(".oxy-ui-rebalance-list__text")?.id);
    expect($(".oxy-ui-rebalance-list__foot")?.textContent).toBe("Nothing is published.");
    expect(reset()).toBeUndefined();
  });

  it("applies each row on its own, and an applied row changes its look and its word", async () => {
    await render(<Held />);
    await click(apply(1));
    expect(apply(1).getAttribute("aria-pressed")).toBe("true");
    expect(apply(1).textContent).toBe("Applied");
    expect(apply(1).classList).toContain("oxy-ui-btn--secondary");
    expect(rows()[1].classList).toContain("oxy-ui-rebalance-list__row--applied");
    expect(rows()[0].classList).not.toContain("oxy-ui-rebalance-list__row--applied");
    expect(apply(0).getAttribute("aria-pressed")).toBe("false");
    // The idle mark gives way to the reset.
    expect($("[data-idle]")).toBeNull();
    expect(reset()).toBeDefined();
    await click(apply(1));
    expect(apply(1).getAttribute("aria-pressed")).toBe("false");
    expect($("[data-idle]")).not.toBeNull();
  });

  it("hands focus to the first row's Apply when the reset clears every move", async () => {
    await render(<Held />);
    await click(apply(0));
    await click(apply(1));
    const button = reset() as HTMLButtonElement;
    button.focus();
    await click(button);
    expect(reset()).toBeUndefined();
    expect(button.isConnected).toBe(false);
    expect(document.activeElement).toBe(apply(0));
    expect(rows().every((r) => !r.classList.contains("oxy-ui-rebalance-list__row--applied"))).toBe(
      true
    );
  });

  it("offers no reset without onReset, and says so when there is no move", async () => {
    const page = await render(<CoverageRebalanceList {...base} applied={new Set(["East>West"])} />);
    expect(reset()).toBeUndefined();
    expect($("[data-idle]")).not.toBeNull();
    await page.rerender(<CoverageRebalanceList {...base} recs={[]} />);
    expect(rows()).toHaveLength(0);
    expect($(".oxy-ui-rebalance-list__empty")?.textContent).toBe("Nothing to rebalance.");
    expect($(".oxy-ui-rebalance-list__foot")).toBeNull();
    await page.rerender(<CoverageRebalanceList {...base} recs={[]} empty='No nearby move.' />);
    expect($(".oxy-ui-rebalance-list__empty")?.textContent).toBe("No nearby move.");
  });
});
