// @vitest-environment jsdom
//
// FacilityActivityFeed: a named list of one-line rows (time, who, what, where); a row that names
// a place is a button that hands the place to the screen; a row with none, or a feed with no
// handler, is plain text, not a disabled button; an empty feed says so; the box's height is the
// caller's.

import { afterEach, describe, expect, it, vi } from "vitest";
import { $, $$, click, render } from "../ui.harness";
import { type ActivityFeedEntry, FacilityActivityFeed } from "./FacilityActivityFeed";

afterEach(() => {
  document.body.innerHTML = "";
});

const ENTRIES: ActivityFeedEntry[] = [
  { ts: "09:41", actor: "A. Lee", text: "Picked order 12 · two cases", loc: "A.01.01" },
  { ts: "09:38", actor: "B. Ruiz", text: "Count variance flagged" },
  { ts: "09:30", actor: "C. Ode", text: "Received delivery 7", loc: "RCV.03" }
];

describe("FacilityActivityFeed", () => {
  it("lists each entry on one line: time, who, what, and where", async () => {
    await render(
      <FacilityActivityFeed entries={ENTRIES} label='Live activity' maxHeight='12rem' />
    );
    const list = $("ul") as HTMLElement;
    expect(list.getAttribute("aria-label")).toBe("Live activity");
    expect(list.style.maxHeight).toBe("12rem");
    const items = $$("li.oxy-ui-activity-feed__item");
    expect(items.map((li) => li.textContent)).toEqual([
      "09:41A. LeePicked order 12 · two casesA.01.01",
      "09:38B. RuizCount variance flagged",
      "09:30C. OdeReceived delivery 7RCV.03"
    ]);
    const [first] = items;
    expect(
      [...first.querySelectorAll(".oxy-ui-activity-feed__ink")].map((s) => s.textContent)
    ).toEqual(["A. Lee", "A.01.01"]);
    expect(first.querySelector(".oxy-ui-activity-feed__text")?.textContent).toBe(
      "Picked order 12 · two cases"
    );
    // No handler: nothing to press.
    expect($$("li button")).toHaveLength(0);
  });

  it("hands the place a row names to the screen; a row with none is not a control", async () => {
    const onSelectLocation = vi.fn();
    await render(<FacilityActivityFeed entries={ENTRIES} onSelectLocation={onSelectLocation} />);
    const buttons = $$("li button");
    expect(buttons).toHaveLength(2);
    expect(buttons.every((b) => !b.hasAttribute("disabled"))).toBe(true);
    const plain = $$("li")[1].firstElementChild as HTMLElement;
    expect(plain.tagName).toBe("SPAN");
    await click(buttons[1]);
    expect(onSelectLocation).toHaveBeenCalledWith("RCV.03");
    await click(plain);
    expect(onSelectLocation).toHaveBeenCalledTimes(1);
  });

  it("says so when there is no activity", async () => {
    const page = await render(<FacilityActivityFeed entries={[]} />);
    expect($("ul")).toBeNull();
    expect($(".oxy-ui-activity-feed__empty")?.textContent).toBe("No activity yet.");
    await page.rerender(<FacilityActivityFeed entries={[]} empty='Quiet so far today.' />);
    expect($(".oxy-ui-activity-feed__empty")?.textContent).toBe("Quiet so far today.");
  });
});
