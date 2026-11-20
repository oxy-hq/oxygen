// @vitest-environment jsdom
//
// DefaultTabs, driven as a person does: one Tab stop, the arrows and Home/End, the select it
// becomes when the strip is wider than its room and the strip it becomes again when the room
// comes back (widths stubbed: jsdom lays nothing out), counts that wear the worst tone, the
// scrolling strip that keeps the selected tab in view, and the panel the tabs point at.

import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { strongestTone } from "../tone";
import { $, $$, click, fire, press, render } from "../ui.harness";
import { DefaultTabs, type DefaultTabsProps, type TabOption, TabPanel } from "./DefaultTabs";
import { fakeResizeObserver, restoreLayout, stubLayout } from "./layout.harness";

type View = "all" | "open" | "late" | "done";
const OPTIONS: TabOption<View>[] = [
  { value: "all", label: "All", count: 12 },
  { value: "open", label: "Open", count: 4 },
  { value: "late", label: "Late", count: 0 },
  { value: "done", label: "Done" }
];

afterEach(() => {
  document.body.innerHTML = "";
  restoreLayout();
});

function Strip(props: Partial<DefaultTabsProps<View>> & { onPick?: (v: View) => void }) {
  const [value, setValue] = useState<View>(props.value ?? "all");
  return (
    <DefaultTabs
      group='work'
      label='Which work'
      options={OPTIONS}
      {...props}
      value={value}
      onChange={(v) => {
        props.onPick?.(v);
        setValue(v);
      }}
    />
  );
}

function WithPanel() {
  const [v, setV] = useState<View>("open");
  return (
    <>
      <DefaultTabs
        group='work'
        label='Which work'
        options={OPTIONS}
        value={v}
        onChange={setV}
        hasPanel
      />
      <TabPanel group='work' value={v}>
        body
      </TabPanel>
    </>
  );
}

const tabs = () => $$('[role="tab"]');
const selected = () => $('[role="tab"][aria-selected="true"]')?.textContent;

describe("DefaultTabs: the keyboard", () => {
  it("is one Tab stop, the selected tab, in a labelled tablist", async () => {
    await render(<Strip value='open' />);
    expect($('[role="tablist"]')?.getAttribute("aria-label")).toBe("Which work");
    expect(tabs().map((t) => t.tabIndex)).toEqual([-1, 0, -1, -1]);
    expect(selected()).toBe("Open4");
  });

  it("moves along with Left/Right, wraps at both ends, and selects as it goes", async () => {
    const onPick = vi.fn();
    await render(<Strip onPick={onPick} />);
    tabs()[0].focus();
    await press("ArrowRight");
    expect(document.activeElement).toBe(tabs()[1]);
    expect(selected()).toBe("Open4");
    await press("ArrowLeft");
    await press("ArrowLeft");
    expect(document.activeElement).toBe(tabs()[3]);
    expect(selected()).toBe("Done");
    await press("ArrowRight");
    expect(document.activeElement).toBe(tabs()[0]);
    expect(onPick.mock.calls.map((c) => c[0])).toEqual(["open", "all", "done", "all"]);
  });

  it("jumps to the ends with Home and End", async () => {
    await render(<Strip value='open' />);
    tabs()[1].focus();
    await press("End");
    expect(document.activeElement).toBe(tabs()[3]);
    expect(selected()).toBe("Done");
    await press("Home");
    expect(document.activeElement).toBe(tabs()[0]);
    expect(selected()).toBe("All12");
  });

  it("only moves focus with activation='manual'; a press selects", async () => {
    const onPick = vi.fn();
    await render(<Strip activation='manual' onPick={onPick} />);
    tabs()[0].focus();
    await press("ArrowRight");
    await press("ArrowRight");
    expect(document.activeElement).toBe(tabs()[2]);
    expect(onPick).not.toHaveBeenCalled();
    expect(selected()).toBe("All12");
    await click(tabs()[2]);
    expect(onPick).toHaveBeenCalledWith("late");
    expect(selected()).toBe("Late0");
  });
});

describe("DefaultTabs: counts", () => {
  it("draws a count whenever it is a number, a 0 included, and none without one", async () => {
    await render(<Strip />);
    expect(tabs().map((t) => t.querySelector(".oxy-ui-tab-count")?.textContent ?? null)).toEqual([
      "12",
      "4",
      "0",
      null
    ]);
  });

  it("wears the worst tone of what it holds, and the neutral chip without one", async () => {
    const rows = ["success", "warning", undefined, "error", "info"] as const;
    const toned: TabOption<View>[] = [
      { value: "all", label: "All", count: 5, countTone: strongestTone(rows) },
      { value: "open", label: "Open", count: 2, countTone: strongestTone(["warning", "info"]) },
      { value: "late", label: "Late", count: 1, countTone: strongestTone(["success"]) },
      // Info passed straight in still reads neutral: a count shouts only error and warning.
      { value: "done", label: "Done", count: 1, countTone: "info" }
    ];
    await render(<Strip value='open' options={toned} />);
    const chip = (i: number) => tabs()[i].querySelector(".oxy-ui-tab-count")?.className ?? "";
    expect(chip(0)).toContain("oxy-ui-tone-error");
    expect(chip(1)).toContain("oxy-ui-tone-warning");
    // Success is not news a count shouts: neutral, on the unselected tab's tint.
    for (const i of [2, 3]) {
      expect(chip(i)).toContain("oxy-ui-tab-count--neutral");
      expect(chip(i)).not.toContain("oxy-ui-tone-");
    }
  });

  it("puts `trailing` in the count's place", async () => {
    const opts: TabOption<View>[] = [
      { value: "all", label: "All", count: 3, trailing: <em data-testid='mark'>$40</em> },
      { value: "open", label: "Open" }
    ];
    await render(<Strip options={opts} />);
    expect($('[data-testid="mark"]')).not.toBeNull();
    expect(tabs()[0].querySelector(".oxy-ui-tab-count")).toBeNull();
  });
});

describe("DefaultTabs: a strip wider than its room", () => {
  // The strip's options need 400px; its box (and its parent) have `room`.
  let room = 600;
  const NEED = 400;
  const layout = () =>
    stubLayout({
      scrollWidth: (el) => (el.classList.contains("oxy-ui-default-tabs") ? NEED : undefined),
      clientWidth: (el) =>
        el.classList.contains("oxy-ui-default-tabs")
          ? Math.min(room, NEED)
          : el.dataset.testid === "room"
            ? room
            : undefined
    });

  async function mount(props: Parameters<typeof Strip>[0] = {}) {
    await render(
      <div data-testid='room'>
        <Strip {...props} />
      </div>
    );
  }

  it("is a strip while it fits, a select once it does not, and a strip again when room returns", async () => {
    const ro = fakeResizeObserver();
    try {
      room = 600;
      layout();
      await mount({ value: "open" });
      expect($('[role="tablist"]')).not.toBeNull();
      expect($("select")).toBeNull();

      room = 300;
      await ro.resize();
      const select = $("select") as HTMLSelectElement;
      expect($('[role="tablist"]')).toBeNull();
      expect(select.getAttribute("aria-label")).toBe("Which work");
      expect(select.value).toBe("open");
      expect([...select.options].map((o) => o.textContent)).toEqual([
        "All (12)",
        "Open (4)",
        "Late (0)",
        "Done"
      ]);
      expect(select.className).toContain("oxy-ui-select");

      room = 500;
      await ro.resize();
      expect($("select")).toBeNull();
      expect(selected()).toBe("Open4");
    } finally {
      ro.uninstall();
    }
  });

  it("chooses from the select as it does from the strip", async () => {
    room = 300;
    layout();
    const onPick = vi.fn();
    await mount({ onPick });
    const select = $("select") as HTMLSelectElement;
    select.value = "late";
    await fire(select, new Event("change", { bubbles: true }));
    expect(onPick).toHaveBeenCalledWith("late");
  });

  it("never becomes a select with overflow='scroll'; it scrolls, the selected tab in view", async () => {
    room = 300;
    // Four 100px tabs in a 300px strip: the fourth sits at 300-400, past the strip's right edge.
    stubLayout({
      scrollWidth: (el) => (el.getAttribute("role") === "tablist" ? NEED : undefined),
      clientWidth: (el) => (el.getAttribute("role") === "tablist" ? room : undefined),
      rect: (el) => {
        const list = el.closest('[role="tablist"]') as HTMLElement | null;
        if (el.getAttribute("role") === "tablist") return { left: 0, width: room };
        if (el.getAttribute("role") === "tab" && list) {
          const i = [...list.children].indexOf(el);
          return { left: i * 100 - list.scrollLeft, width: 100 };
        }
        return undefined;
      }
    });
    await mount({ overflow: "scroll", value: "done" });
    const list = $('[role="tablist"]') as HTMLElement;
    expect($("select")).toBeNull();
    expect(list.className).toContain("oxy-ui-default-tabs--scroll");
    expect(list.scrollLeft).toBe(100);
  });
});

describe("DefaultTabs: sizes and the panel", () => {
  it("draws the touch strip full width at the crew size", async () => {
    await render(<Strip size='touch' />);
    expect($('[role="tablist"]')?.className).toContain("oxy-ui-default-tabs--touch");
  });

  it("takes the 32px floor when it shares a row", async () => {
    await render(<Strip sharesRow />);
    for (const t of tabs()) expect(t.className).toContain("oxy-ui-segment--shares-row");
  });

  it("points each tab at the one panel, which the selected tab labels", async () => {
    await render(<WithPanel />);
    const panel = $('[role="tabpanel"]') as HTMLElement;
    for (const t of tabs()) expect(t.getAttribute("aria-controls")).toBe(panel.id);
    expect(document.getElementById(panel.getAttribute("aria-labelledby") ?? "")).toBe(tabs()[1]);
    await click(tabs()[3]);
    expect(document.getElementById(panel.getAttribute("aria-labelledby") ?? "")).toBe(tabs()[3]);
  });

  it("keeps a legend on the strip's own row, and draws no wrapper without one", async () => {
    await render(<Strip legend={<span data-testid='legend'>Key</span>} />);
    expect($('[data-testid="legend"]')?.parentElement?.className).toBe("oxy-ui-default-tabs-row");
    document.body.innerHTML = "";
    await render(<Strip />);
    expect($(".oxy-ui-default-tabs-row")).toBeNull();
  });
});
