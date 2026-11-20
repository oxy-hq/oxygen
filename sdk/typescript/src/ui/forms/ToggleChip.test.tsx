// @vitest-environment jsdom
//
// The toggle chip: a button that says whether it is pressed (`aria-pressed`), flips on a press,
// stays still when disabled, takes the 32px control geometry only when it shares a row, and a
// group named by its caps label.

import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ui } from "../recipes";
import { $, $$, click, render } from "../ui.harness";
import { ToggleChip, ToggleChipGroup } from "./ToggleChip";

afterEach(() => {
  document.body.innerHTML = "";
});

const chip = () => $("button") as HTMLButtonElement;

function Filter() {
  const [on, setOn] = useState(false);
  return (
    <ToggleChip pressed={on} onPressedChange={setOn}>
      Late
    </ToggleChip>
  );
}

describe("ToggleChip", () => {
  it("says whether it is pressed, and flips on each press", async () => {
    await render(<Filter />);
    expect(chip().type).toBe("button");
    expect(chip().getAttribute("aria-pressed")).toBe("false");
    await click(chip());
    expect(chip().getAttribute("aria-pressed")).toBe("true");
    await click(chip());
    expect(chip().getAttribute("aria-pressed")).toBe("false");
  });

  it("asks for the opposite of what it shows, and stays still when disabled", async () => {
    const onPressedChange = vi.fn();
    const page = await render(
      <ToggleChip pressed onPressedChange={onPressedChange}>
        Late
      </ToggleChip>
    );
    await click(chip());
    expect(onPressedChange).toHaveBeenCalledWith(false);
    await page.rerender(
      <ToggleChip pressed onPressedChange={onPressedChange} disabled>
        Late
      </ToggleChip>
    );
    await click(chip());
    expect(onPressedChange).toHaveBeenCalledTimes(1);
  });

  it("is 24px caps alone, and takes the 32px control geometry only when it shares a row", async () => {
    const page = await render(
      <ToggleChip pressed={false} onPressedChange={() => {}}>
        Late
      </ToggleChip>
    );
    expect(chip().classList).toContain(ui.text3xs);
    expect(chip().classList).toContain(ui.focusRing);
    expect(chip().classList.contains(ui.controlSm)).toBe(false);
    await page.rerender(
      <ToggleChip pressed={false} onPressedChange={() => {}} sharesRow>
        Late
      </ToggleChip>
    );
    expect(chip().classList).toContain(ui.controlSm);
    expect(chip().classList).toContain("oxy-ui-toggle-chip--shares-row");
  });

  it("sits in a group named by its caps label, read once", async () => {
    await render(
      <ToggleChipGroup label='Columns'>
        <ToggleChip pressed onPressedChange={() => {}}>
          Cost
        </ToggleChip>
        <ToggleChip pressed={false} onPressedChange={() => {}}>
          Lead time
        </ToggleChip>
      </ToggleChipGroup>
    );
    const group = $("[role='group']") as HTMLElement;
    expect(group.getAttribute("aria-label")).toBe("Columns");
    const label = group.querySelector(".oxy-ui-toggle-chip-group__label") as HTMLElement;
    expect(label.textContent).toBe("Columns");
    expect(label.getAttribute("aria-hidden")).toBe("true");
    expect($$("button[aria-pressed]")).toHaveLength(2);
  });
});
