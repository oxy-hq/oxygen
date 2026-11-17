// @vitest-environment jsdom
//
// The tab bar never shows more than five slots: past five, the fifth is
// "More" and the rest open in a sheet, so no section is cut or scrolled off.

import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PhoneTabBar, type PhoneTabItem } from "./PhoneTabBar";
import { $, $$, click, render } from "./phone.harness";

afterEach(() => {
  document.body.innerHTML = "";
});

const slots = () => $$(".oxy-phone-tabbar__slot");
const labels = () => slots().map((s) => s.querySelector(".oxy-phone-tabbar__label")?.textContent);
const current = () => slots().find((s) => s.getAttribute("aria-current") === "page");

function Bar({ items, onSelect }: { items: PhoneTabItem[]; onSelect?: (id: string) => void }) {
  const [active, setActive] = useState(items[0].id);
  return (
    <PhoneTabBar
      items={items}
      activeId={active}
      onSelect={(id) => {
        setActive(id);
        onSelect?.(id);
      }}
    />
  );
}

const SEVEN: PhoneTabItem[] = [
  { id: "today", label: "Today" },
  { id: "stores", label: "Stores", badge: 3 },
  { id: "post", label: "Post" },
  { id: "credit", label: "Credit memos" },
  { id: "grubhub", label: "GrubHub", badge: 0 },
  { id: "doordash", label: "DoorDash", badge: 2 },
  { id: "settings", label: "Settings" }
];

describe("PhoneTabBar", () => {
  it("shows up to five items as named slots inside a nav landmark", async () => {
    const onSelect = vi.fn();
    const page = await render(<Bar items={SEVEN.slice(0, 5)} onSelect={onSelect} />);
    expect($("nav.oxy-phone-tabbar")?.getAttribute("aria-label")).toBe("Sections");
    expect(labels()).toEqual(["Today", "Stores", "Post", "Credit memos", "GrubHub"]);
    expect(current()?.textContent).toBe("Today");

    await click(slots()[2]);
    expect(onSelect).toHaveBeenCalledWith("post");
    expect(current()?.textContent).toBe("Post");
    // Only one slot is current.
    expect(slots().filter((s) => s.hasAttribute("aria-current"))).toHaveLength(1);
    await page.unmount();
  });

  it("puts a count on a slot, and nothing for 0", async () => {
    const page = await render(<Bar items={SEVEN.slice(0, 5)} />);
    const badges = $$(".oxy-phone-tabbar__badge").map((b) => b.textContent);
    expect(badges).toEqual(["3"]);
    await page.unmount();
  });

  it("past five, shows four and More, which lists the rest with their badges", async () => {
    const onSelect = vi.fn();
    const page = await render(<Bar items={SEVEN} onSelect={onSelect} />);
    expect(labels()).toEqual(["Today", "Stores", "Post", "Credit memos", "More"]);
    const more = slots()[4];
    // A badge waits behind More, so More shows a dot.
    expect(more.querySelector(".oxy-phone-tabbar__dot")).not.toBeNull();
    expect(more.getAttribute("aria-expanded")).toBe("false");
    expect($('[data-testid="phone-sheet"]')).toBeNull();

    await click(more);
    const sheet = $('[data-testid="phone-sheet"]') as HTMLElement;
    expect(sheet.querySelector(".oxy-phone-sheet__title")?.textContent).toBe("More");
    const rows = [...sheet.querySelectorAll(".oxy-phone-more__row")];
    expect(rows.map((r) => r.querySelector(".oxy-phone-more__label")?.textContent)).toEqual([
      "GrubHub",
      "DoorDash",
      "Settings"
    ]);
    expect(rows.map((r) => r.querySelector(".oxy-phone-tabbar__badge")?.textContent)).toEqual([
      undefined,
      "2",
      undefined
    ]);

    await click(rows[1]);
    expect(onSelect).toHaveBeenLastCalledWith("doordash");
    expect($('[data-testid="phone-sheet"]')).toBeNull();
    // An overflow section is current: More carries it.
    expect(current()?.textContent).toContain("More");
    await page.unmount();
  });

  it("names More from moreLabel", async () => {
    const page = await render(
      <PhoneTabBar items={SEVEN} activeId='today' onSelect={() => {}} moreLabel='All' />
    );
    expect(labels()[4]).toBe("All");
    await page.unmount();
  });

  it("closes More when the items no longer overflow, and does not reopen it", async () => {
    const seven = Array.from({ length: 7 }, (_, i) => ({ id: `t${i}`, label: `T${i}` }));
    const bar = (items: PhoneTabItem[]) => (
      <PhoneTabBar items={items} activeId='t0' onSelect={() => {}} />
    );
    const page = await render(bar(seven));
    await click(slots()[4]);
    expect($('[data-testid="phone-sheet"]')).not.toBeNull();
    await page.rerender(bar(seven.slice(0, 3)));
    expect($('[data-testid="phone-sheet"]')).toBeNull();
    await page.rerender(bar(seven));
    expect($('[data-testid="phone-sheet"]')).toBeNull();
    await page.unmount();
  });

  it("shows no badge for the string 0, or for a null from an API", async () => {
    const page = await render(
      <PhoneTabBar
        items={[
          { id: "a", label: "A", badge: "0" },
          { id: "b", label: "B", badge: null as unknown as undefined },
          { id: "c", label: "C", badge: "New" }
        ]}
        activeId='a'
        onSelect={() => {}}
      />
    );
    expect($$(".oxy-phone-tabbar__badge").map((b) => b.textContent)).toEqual(["New"]);
    await page.unmount();
  });

  it("draws a badge with no tone as before, and a toned one with its tone, on the bar and in More", async () => {
    const page = await render(
      <PhoneTabBar
        items={[
          { id: "a", label: "A", badge: 3 },
          { id: "b", label: "B", badge: 1, badgeTone: "neutral" },
          { id: "c", label: "C", badge: 2, badgeTone: "bad" },
          { id: "d", label: "D" },
          { id: "e", label: "E", badge: 4, badgeTone: "warn" },
          { id: "f", label: "F", badge: 5 }
        ]}
        activeId='a'
        onSelect={() => {}}
      />
    );
    expect(
      $$(".oxy-phone-tabbar__slot > .oxy-phone-tabbar__badge").map((b) => b.className)
    ).toEqual([
      "oxy-phone-tabbar__badge",
      "oxy-phone-tabbar__badge oxy-phone-tabbar__badge--toned oxy-phone-tone--neutral",
      "oxy-phone-tabbar__badge oxy-phone-tabbar__badge--toned oxy-phone-tone--bad"
    ]);
    // F behind More has no tone, so More's dot is the one it always was.
    expect(slots()[4].querySelector(".oxy-phone-tabbar__dot")?.className).toBe(
      "oxy-phone-tabbar__dot"
    );
    await click(slots()[4]);
    expect($$(".oxy-phone-more__row .oxy-phone-tabbar__badge").map((b) => b.className)).toEqual([
      "oxy-phone-tabbar__badge oxy-phone-more__badge oxy-phone-tabbar__badge--toned oxy-phone-tone--warn",
      "oxy-phone-tabbar__badge oxy-phone-more__badge"
    ]);
    await page.unmount();
  });

  it("tones More's dot with the strongest tone behind it once every badge there has one", async () => {
    // error > warning > neutral, success counted as neutral (the design system's strongestTone).
    const items = (...tones: (PhoneTabItem["badgeTone"] | "none")[]): PhoneTabItem[] => [
      ...["a", "b", "c", "d"].map((id) => ({ id, label: id })),
      ...tones.map((t, i) => ({
        id: `x${i}`,
        label: `X${i}`,
        badge: 1,
        ...(t === "none" ? {} : { badgeTone: t })
      }))
    ];
    const dot = () => slots()[4].querySelector(".oxy-phone-tabbar__dot")?.className;
    const bar = (list: PhoneTabItem[]) => (
      <PhoneTabBar items={list} activeId='a' onSelect={() => {}} />
    );
    const page = await render(bar(items("neutral", "warn", "good")));
    expect(dot()).toBe("oxy-phone-tabbar__dot oxy-phone-tabbar__dot--toned oxy-phone-tone--warn");
    await page.rerender(bar(items("warn", "bad")));
    expect(dot()).toBe("oxy-phone-tabbar__dot oxy-phone-tabbar__dot--toned oxy-phone-tone--bad");
    await page.rerender(bar(items("good", "good")));
    expect(dot()).toBe(
      "oxy-phone-tabbar__dot oxy-phone-tabbar__dot--toned oxy-phone-tone--neutral"
    );
    await page.rerender(bar(items("neutral", "none")));
    expect(dot()).toBe("oxy-phone-tabbar__dot");
    await page.unmount();
  });
});
