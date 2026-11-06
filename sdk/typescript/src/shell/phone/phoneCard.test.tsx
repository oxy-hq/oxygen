// @vitest-environment jsdom
//
// Cards and rows replace desk tables: a tap anywhere opens the detail, while
// an action in `trailing` stays its own button (never nested in another).

import { afterEach, describe, expect, it, vi } from "vitest";
import { PhoneCard } from "./PhoneCard";
import { PhoneCardList } from "./PhoneCardList";
import { PhoneFilterChip } from "./PhoneFilterChip";
import { PhoneKpiStrip } from "./PhoneKpiStrip";
import { PhoneRow } from "./PhoneRow";
import { PhoneSection } from "./PhoneSection";
import { PhoneSegmented } from "./PhoneSegmented";
import { $, $$, click, fire, render } from "./phone.harness";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("PhoneCard", () => {
  it("opens on a tap, with its title, status and up to three figures", async () => {
    const onClick = vi.fn();
    const onPost = vi.fn();
    const page = await render(
      <PhoneCardList summary='14 stores · $724,102 · 14/14 posted'>
        <PhoneCard
          title='Almaden'
          subtitle='San Jose'
          status={{ label: "Late", tone: "warn" }}
          figures={[
            { label: "Sales", value: "$52,140" },
            { label: "Fees", value: "$3,210" },
            { label: "Net", value: "$48,930" },
            { label: "Dropped", value: "x" }
          ]}
          trailing={
            <button type='button' data-testid='post' onClick={onPost}>
              Post
            </button>
          }
          onClick={onClick}
        />
      </PhoneCardList>
    );
    expect($(".oxy-phone-cards__summary")?.textContent).toBe("14 stores · $724,102 · 14/14 posted");
    const main = $(".oxy-phone-card__main") as HTMLElement;
    expect(main.tagName).toBe("BUTTON");
    expect(main.textContent).toBe("AlmadenLateSan Jose");
    const pill = $(".oxy-phone-pill") as HTMLElement;
    expect(pill.classList.contains("oxy-phone-tone--warn")).toBe(true);
    expect($$(".oxy-phone-card__figure dt").map((d) => d.textContent)).toEqual([
      "Sales",
      "Fees",
      "Net"
    ]);
    expect($(".oxy-phone-card__chevron")).not.toBeNull();

    await click(main);
    expect(onClick).toHaveBeenCalledTimes(1);
    // The trailing action is its own button, outside the card's.
    const post = $('[data-testid="post"]') as HTMLElement;
    expect(main.contains(post)).toBe(false);
    await click(post);
    expect(onPost).toHaveBeenCalledTimes(1);
    expect(onClick).toHaveBeenCalledTimes(1);
    await page.unmount();
  });

  it("is not a button without onClick", async () => {
    const page = await render(<PhoneCard title='Almaden' />);
    expect($(".oxy-phone-card__main")?.tagName).toBe("DIV");
    expect($(".oxy-phone-card__chevron")).toBeNull();
    expect($(".oxy-phone-card__figures")).toBeNull();
    await page.unmount();
  });
});

describe("PhoneRow", () => {
  it("is a button with a dot and a chevron when clickable", async () => {
    const onClick = vi.fn();
    const page = await render(
      <PhoneSection title='Needs you' action={<span>See all</span>}>
        <PhoneRow
          title='3 deposits unmatched'
          subtitle='Since Monday'
          tone='bad'
          onClick={onClick}
        />
        <PhoneRow title='All clear' trailing='$1.2M' />
      </PhoneSection>
    );
    expect($(".oxy-phone-section__title")?.tagName).toBe("H2");
    expect($(".oxy-phone-section__action")?.textContent).toBe("See all");
    const [first, second] = $$(".oxy-phone-row");
    expect(first.querySelector(".oxy-phone-row__dot")?.classList).toContain("oxy-phone-tone--bad");
    expect(first.querySelector(".oxy-phone-row__chevron")).not.toBeNull();
    await click(first.querySelector("button"));
    expect(onClick).toHaveBeenCalledTimes(1);
    expect(second.querySelector("button")).toBeNull();
    expect(second.querySelector(".oxy-phone-row__dot")).toBeNull();
    expect(second.querySelector(".oxy-phone-row__trailing")?.textContent).toBe("$1.2M");
    await page.unmount();
  });
});

describe("PhoneKpiStrip", () => {
  it("renders a tile per item, a button when it has onClick", async () => {
    const onClick = vi.fn();
    const page = await render(
      <PhoneKpiStrip
        items={[
          { label: "Net sales", value: "$25.7M", delta: "+25.7%", tone: "good", onClick },
          { label: "Labor", value: "23.5%", delta: "+1.4pts" },
          { label: "Stores", value: "24" }
        ]}
      />
    );
    const tiles = $$(".oxy-phone-kpi");
    expect(tiles.map((t) => t.tagName)).toEqual(["BUTTON", "DIV", "DIV"]);
    expect(tiles[0].querySelector(".oxy-phone-kpi__delta")?.classList).toContain(
      "oxy-phone-tone--good"
    );
    // No tone given: neutral, never red by default.
    expect(tiles[1].querySelector(".oxy-phone-kpi__delta")?.classList).toContain(
      "oxy-phone-tone--neutral"
    );
    expect(tiles[2].querySelector(".oxy-phone-kpi__delta")).toBeNull();
    await click(tiles[0]);
    expect(onClick).toHaveBeenCalledTimes(1);
    await page.unmount();
  });
});

describe("PhoneSegmented", () => {
  it("is a radio group: a tap or an arrow key moves the choice", async () => {
    const onChange = vi.fn();
    const options = [
      { value: "list", label: "List" },
      { value: "map", label: "Map" }
    ];
    const page = await render(
      <PhoneSegmented options={options} value='list' onChange={onChange} ariaLabel='View' />
    );
    const group = $('[role="radiogroup"]') as HTMLElement;
    expect(group.getAttribute("aria-label")).toBe("View");
    const radios = $$('[role="radio"]');
    expect(radios.map((r) => r.getAttribute("aria-checked"))).toEqual(["true", "false"]);
    expect(radios.map((r) => r.tabIndex)).toEqual([0, -1]);
    await click(radios[1]);
    expect(onChange).toHaveBeenLastCalledWith("map");
    await fire(radios[0], new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
    expect(onChange).toHaveBeenLastCalledWith("map"); // wraps from the first
    await page.unmount();
  });
  it("stays reachable by keyboard while its value matches no option", async () => {
    const onChange = vi.fn();
    const options = [
      { value: "list", label: "List" },
      { value: "map", label: "Day" },
      { value: "grid", label: "Grid" }
    ];
    const page = await render(<PhoneSegmented options={options} value='' onChange={onChange} />);
    const radios = $$('[role="radio"]');
    expect(radios.map((r) => r.tabIndex)).toEqual([0, -1, -1]);
    await fire(radios[0], new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    expect(onChange).toHaveBeenLastCalledWith("list");
    await fire(radios[0], new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
    expect(onChange).toHaveBeenLastCalledWith("grid");
    await page.unmount();
  });
});

describe("PhoneFilterChip", () => {
  it("opens the filters and shows how many are set", async () => {
    const onClick = vi.fn();
    const page = await render(
      <PhoneFilterChip label='Oct 2026 · All companies' count={2} onClick={onClick} />
    );
    const chip = $(".oxy-phone-chip") as HTMLElement;
    expect(chip.getAttribute("aria-haspopup")).toBe("dialog");
    expect(chip.textContent).toBe("Oct 2026 · All companies2 filters set");
    await click(chip);
    expect(onClick).toHaveBeenCalledTimes(1);
    await page.rerender(<PhoneFilterChip label='All' count={0} onClick={onClick} />);
    expect($(".oxy-phone-chip__count")).toBeNull();
    await page.unmount();
  });
});
