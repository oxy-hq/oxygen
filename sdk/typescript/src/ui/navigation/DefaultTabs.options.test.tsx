// @vitest-environment jsdom
//
// DefaultTabs' 2.31.0 options, driven as a person does: a disabled option the arrows pass over,
// a click cannot choose and the select cannot pick, while focus is never dropped; the equal-share
// strip; a hint on a tab that its keyboard focus opens; a label with markup and its words for the
// select; and counts printed with their grouping. Plus the top bar's new widths (ParentTabs).

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { act, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { $, $$, click, press, render } from "../ui.harness";
import { DefaultTabs, type DefaultTabsProps, type TabOption } from "./DefaultTabs";
import { restoreLayout, stubLayout } from "./layout.harness";
import { ParentTabs } from "./ParentTabs";
import type { TopBarFrom } from "./parentTab";

type Market = "all" | "sf" | "la" | "sd";

afterEach(() => {
  document.body.innerHTML = "";
  restoreLayout();
});

const tabs = () => $$('[role="tab"]');
const selected = () => tabs().find((t) => t.getAttribute("aria-selected") === "true");

function Strip({
  options,
  start = "sf",
  onPick,
  ...rest
}: Partial<DefaultTabsProps<Market>> & {
  options: TabOption<Market>[];
  start?: Market;
  onPick?: (v: Market) => void;
}) {
  const [value, setValue] = useState<Market>(start);
  return (
    <DefaultTabs
      group='market'
      label='Market'
      {...rest}
      options={options}
      value={value}
      onChange={(v) => {
        onPick?.(v);
        setValue(v);
      }}
    />
  );
}

const MARKETS: TabOption<Market>[] = [
  { value: "all", label: "All", disabled: true },
  { value: "sf", label: "San Francisco" },
  { value: "la", label: "Los Angeles", disabled: true },
  { value: "sd", label: "San Diego" }
];

describe("DefaultTabs: a disabled option", () => {
  it("is passed over by the arrows, Home and End, and says aria-disabled", async () => {
    await render(<Strip options={MARKETS} />);
    expect(tabs().map((t) => t.getAttribute("aria-disabled"))).toEqual([
      "true",
      null,
      "true",
      null
    ]);
    // Never the `disabled` attribute: a disabled control drops focus to the page.
    expect(tabs().some((t) => t.hasAttribute("disabled"))).toBe(false);
    selected()?.focus();
    await press("ArrowRight");
    expect(selected()?.textContent).toBe("San Diego");
    expect(document.activeElement).toBe(selected());
    await press("ArrowRight");
    expect(selected()?.textContent).toBe("San Francisco");
    await press("ArrowLeft");
    expect(selected()?.textContent).toBe("San Diego");
    await press("Home");
    expect(selected()?.textContent).toBe("San Francisco");
    await press("End");
    expect(selected()?.textContent).toBe("San Diego");
  });

  it("takes no click, and the select offers it disabled", async () => {
    const onPick = vi.fn();
    await render(<Strip options={MARKETS} onPick={onPick} />);
    await click(tabs()[2]);
    expect(onPick).not.toHaveBeenCalled();
    expect(selected()?.textContent).toBe("San Francisco");
    document.body.innerHTML = "";
    stubLayout({
      scrollWidth: (el) => (el.classList.contains("oxy-ui-default-tabs") ? 400 : undefined),
      clientWidth: (el) => (el.classList.contains("oxy-ui-default-tabs") ? 300 : undefined)
    });
    await render(<Strip options={MARKETS} />);
    const select = $("select") as HTMLSelectElement;
    expect([...select.options].map((o) => o.disabled)).toEqual([true, false, true, false]);
  });

  it("keeps the Tab stop on a selected tab that is disabled, and gives a missing value the first enabled", async () => {
    let page = await render(<Strip options={MARKETS} start='la' />);
    expect(tabs().map((t) => t.tabIndex)).toEqual([-1, -1, 0, -1]);
    // From a disabled selected tab, the arrows move on to an enabled one.
    tabs()[2].focus();
    await press("ArrowRight");
    expect(selected()?.textContent).toBe("San Diego");
    await page.unmount();
    page = await render(
      <DefaultTabs
        group='m'
        label='Market'
        options={MARKETS}
        value={"none" as Market}
        onChange={() => {}}
      />
    );
    expect(tabs().map((t) => t.tabIndex)).toEqual([-1, 0, -1, -1]);
  });
});

describe("DefaultTabs: stretch, hint, a label with markup, grouped counts", () => {
  it("stretch: the strip takes the row and each segment an equal part", async () => {
    await render(<Strip options={MARKETS.slice(1)} stretch />);
    expect($('[role="tablist"]')?.classList).toContain("oxy-ui-default-tabs--stretch");
    const css = readFileSync(
      join(dirname(fileURLToPath(import.meta.url)), "navigation.css"),
      "utf8"
    );
    expect(css).toMatch(/\.oxy-ui-default-tabs--stretch > \* \{\s*flex: 1 1 0%;/);
  });

  it("hint: a tab's hint opens on its keyboard focus and describes it", async () => {
    await render(
      <Strip
        options={[
          { value: "sf", label: "Month", hint: "Sales tax owed for the month." },
          { value: "sd", label: "Quarter" }
        ]}
      />
    );
    const month = tabs()[0];
    expect(month.parentElement?.classList).toContain("oxy-ui-default-tabs__hint");
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
      month.focus();
    });
    const bubble = $('[role="tooltip"]') as HTMLElement;
    expect(bubble.textContent).toBe("Sales tax owed for the month.");
    expect(month.getAttribute("aria-describedby")).toBe(bubble.id);
    // The arrows still find the next tab through the hint around it.
    await press("ArrowRight");
    expect(document.activeElement?.textContent).toBe("Quarter");
  });

  it("takes a label with markup, and its words for the select", async () => {
    stubLayout({
      scrollWidth: (el) => (el.classList.contains("oxy-ui-default-tabs") ? 400 : undefined),
      clientWidth: (el) => (el.classList.contains("oxy-ui-default-tabs") ? 300 : undefined)
    });
    await render(
      <Strip
        options={[
          {
            value: "sf",
            label: (
              <>
                <b>Nov</b> payroll
              </>
            ),
            text: "Nov payroll",
            count: 12345
          },
          { value: "sd", label: <i>Dec</i> }
        ]}
      />
    );
    const select = $("select") as HTMLSelectElement;
    expect([...select.options].map((o) => o.textContent)).toEqual(["Nov payroll (12,345)", "sd"]);
  });

  it("prints a count with its grouping", async () => {
    await render(
      <Strip
        options={[
          { value: "sf", label: "Ads", count: 12345 },
          { value: "sd", label: "Paused", count: 0 }
        ]}
      />
    );
    expect($$(".oxy-ui-default-tabs__count").map((c) => c.textContent)).toEqual(["12,345", "0"]);
  });
});

describe("ParentTabs: the top bar's widths", () => {
  const STEPS: [TopBarFrom, number][] = [
    ["sm", 640],
    ["md", 768],
    ["lg", 1024],
    ["xl", 1280],
    ["2xl", 1440]
  ];
  const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "navigation.css"), "utf8");
  const SCREENS = [
    { id: "a", label: "Overview", href: "/a" },
    { id: "b", label: "Items", href: "/b" }
  ];

  for (const [step, px] of STEPS)
    it(`from ${step} (${px}px): the page row hides and the top-bar strip shows`, async () => {
      for (const placement of ["page", "topbar"] as const) {
        const page = await render(
          <ParentTabs
            label='Screens'
            tabs={SCREENS}
            current='a'
            placement={placement}
            topBarFrom={step}
          />
        );
        expect($("nav")?.className).toContain(`oxy-ui-parent-tabs--${placement}-${step}`);
        await page.unmount();
      }
      const block = css.slice(css.indexOf(`@media (min-width: ${px}px) {`));
      expect(block.length, `no @media at ${px}px`).toBeLessThan(css.length);
      const body = block.slice(0, block.indexOf("\n}\n"));
      expect(body).toMatch(new RegExp(`\\.oxy-ui-parent-tabs--page-${step} \\{\\s*display: none;`));
      expect(body).toMatch(
        new RegExp(`\\.oxy-ui-parent-tabs--topbar-${step} \\{\\s*display: flex;`)
      );
    });

  it("draws nothing different when topBarFrom is left at md", async () => {
    await render(<ParentTabs label='Screens' tabs={SCREENS} current='a' placement='page' />);
    expect($("nav")?.className).toContain("oxy-ui-parent-tabs--page-md");
  });
});
