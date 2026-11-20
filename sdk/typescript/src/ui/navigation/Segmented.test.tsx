// @vitest-environment jsdom
//
// Segmented is a radio group: one Tab stop on the checked option (the first when none is), the
// arrows move and CHECK as they go, wrapping at the ends, a press checks; and it becomes a select
// when its options outgrow their room (widths stubbed: jsdom lays nothing out).

import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { $, $$, click, press, render } from "../ui.harness";
import { fakeResizeObserver, restoreLayout, stubLayout } from "./layout.harness";
import { Segmented, type SegmentedProps } from "./Segmented";

type Period = "day" | "week" | "month";
const OPTIONS = [
  { value: "day", label: "Day" },
  { value: "week", label: "Week" },
  { value: "month", label: "Month" }
] as const;

afterEach(() => {
  document.body.innerHTML = "";
  restoreLayout();
});

function Field(props: Partial<SegmentedProps<Period>> & { onPick?: (v: Period) => void }) {
  const [value, setValue] = useState<Period>((props.value as Period) ?? "week");
  return (
    <>
      <span id='period-label'>Period</span>
      <Segmented
        labelledBy='period-label'
        options={OPTIONS}
        {...props}
        value={value}
        onChange={(v) => {
          props.onPick?.(v);
          setValue(v);
        }}
      />
    </>
  );
}

const radios = () => $$('[role="radio"]');
const checked = () => $('[role="radio"][aria-checked="true"]')?.textContent;

describe("Segmented", () => {
  it("is a radio group named by its label, one Tab stop on the checked option", async () => {
    await render(<Field />);
    const group = $('[role="radiogroup"]') as HTMLElement;
    expect(document.getElementById(group.getAttribute("aria-labelledby") ?? "")?.textContent).toBe(
      "Period"
    );
    expect(radios().map((r) => r.getAttribute("aria-checked"))).toEqual(["false", "true", "false"]);
    expect(radios().map((r) => r.tabIndex)).toEqual([-1, 0, -1]);
  });

  it("takes an aria-label when no visible label names it", async () => {
    await render(
      <Segmented label='Period' options={OPTIONS} value='day' onChange={() => undefined} />
    );
    expect($('[role="radiogroup"]')?.getAttribute("aria-label")).toBe("Period");
  });

  it("puts the Tab stop on the first option when none is checked", async () => {
    await render(
      <Segmented
        label='Period'
        options={OPTIONS}
        value={"year" as Period}
        onChange={() => undefined}
      />
    );
    expect(radios().map((r) => r.tabIndex)).toEqual([0, -1, -1]);
  });

  it("checks as the arrows move, Right/Down forward and Left/Up back, wrapping", async () => {
    const onPick = vi.fn();
    await render(<Field onPick={onPick} />);
    radios()[1].focus();
    await press("ArrowRight");
    expect(document.activeElement).toBe(radios()[2]);
    expect(checked()).toBe("Month");
    await press("ArrowDown");
    expect(document.activeElement).toBe(radios()[0]);
    expect(checked()).toBe("Day");
    await press("ArrowUp");
    expect(checked()).toBe("Month");
    await press("ArrowLeft");
    expect(checked()).toBe("Week");
    expect(onPick.mock.calls.map((c) => c[0])).toEqual(["month", "day", "month", "week"]);
  });

  it("checks the option pressed", async () => {
    await render(<Field />);
    await click(radios()[0]);
    expect(checked()).toBe("Day");
    expect(radios()[0].tabIndex).toBe(0);
  });

  it("draws the selected segment with the segment recipe: fill, weight and ink", async () => {
    await render(<Field />);
    expect(radios()[1].className).toContain("oxy-ui-segment--on");
    expect(radios()[0].className).toContain("oxy-ui-segment--off");
  });

  it("gives way to a select when its options outgrow the room, and back", async () => {
    const ro = fakeResizeObserver();
    let room = 400;
    stubLayout({
      scrollWidth: (el) => (el.getAttribute("role") === "radiogroup" ? 240 : undefined),
      clientWidth: (el) =>
        el.getAttribute("role") === "radiogroup"
          ? Math.min(room, 240)
          : el.dataset.testid === "room"
            ? room
            : undefined
    });
    try {
      await render(
        <div data-testid='room'>
          <Field />
        </div>
      );
      expect($('[role="radiogroup"]')).not.toBeNull();
      room = 200;
      await ro.resize();
      const select = $("select") as HTMLSelectElement;
      expect(select.getAttribute("aria-labelledby")).toBe("period-label");
      expect(select.value).toBe("week");
      room = 300;
      await ro.resize();
      expect($("select")).toBeNull();
      expect(checked()).toBe("Week");
    } finally {
      ro.uninstall();
    }
  });
});
