// @vitest-environment jsdom
//
// PhoneOptionSheet is the kit's PhoneSheet holding a choice of one: a radio group named by the
// title, the chosen row checked (and marked for the eye), a tap choosing, Back and Escape
// dismissing without choosing and returning focus to the opener, and the arrows moving between
// the rows without choosing (a choice here commits and closes).

import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PhoneOptionSheet } from "./PhoneOptionSheet";
import { $, $$, click, press, render } from "./phone.harness";

afterEach(() => {
  document.body.innerHTML = "";
  document.documentElement.style.overflow = "";
});

type Sort = "name" | "sales" | "flags";
const SORTS = [
  { value: "name", label: "Name" },
  { value: "sales", label: "Sales", detail: "Highest first" },
  { value: "flags", label: "Flags" }
] as const;

function Opener({ onChange, onClose }: { onChange?: (v: Sort) => void; onClose?: () => void }) {
  const [open, setOpen] = useState(false);
  const [sort, setSort] = useState<Sort | null>("sales");
  return (
    <>
      <button type='button' data-testid='open' onClick={() => setOpen(true)}>
        Sort
      </button>
      <PhoneOptionSheet
        open={open}
        onClose={() => {
          onClose?.();
          setOpen(false);
        }}
        title='Sort stores'
        backLabel='Stores'
        options={SORTS}
        value={sort}
        onChange={(v) => {
          onChange?.(v);
          setSort(v);
          setOpen(false);
        }}
      >
        <p data-testid='intro'>Pick an order.</p>
      </PhoneOptionSheet>
    </>
  );
}

const opener = () => $('[data-testid="open"]') as HTMLElement;
const sheet = () => $('[data-testid="phone-sheet"]');
const rows = () => $$('[role="radio"]');

async function open() {
  opener().focus();
  await click(opener());
}

describe("PhoneOptionSheet", () => {
  it("renders nothing while closed", async () => {
    await render(<Opener />);
    expect(sheet()).toBeNull();
  });

  it("opens as the kit's sheet, a radio group named by its title, the chosen row checked", async () => {
    await render(<Opener />);
    await open();
    expect(sheet()?.getAttribute("role")).toBe("dialog");
    expect($(".oxy-phone-sheet__back")?.textContent).toBe("Stores");
    expect(document.activeElement).toBe($(".oxy-phone-sheet__back"));
    expect($('[role="radiogroup"]')?.getAttribute("aria-label")).toBe("Sort stores");
    expect(rows().map((r) => r.getAttribute("aria-checked"))).toEqual(["false", "true", "false"]);
    expect(rows().map((r) => r.getAttribute("aria-pressed"))).toEqual([null, null, null]);
    // The mark for the eye on the chosen row only; the detail under its label.
    expect(rows().map((r) => r.querySelector(".oxy-phone-options__check") !== null)).toEqual([
      false,
      true,
      false
    ]);
    expect(rows()[1].querySelector(".oxy-phone-options__detail")?.textContent).toBe(
      "Highest first"
    );
    // What the sheet says before its options, first.
    expect($('[data-testid="intro"]')?.nextElementSibling?.getAttribute("role")).toBe("radiogroup");
  });

  it("chooses the row tapped; the caller closes it and focus returns to the opener", async () => {
    const onChange = vi.fn();
    await render(<Opener onChange={onChange} />);
    await open();
    await click(rows()[2]);
    expect(onChange).toHaveBeenCalledWith("flags");
    expect(sheet()).toBeNull();
    expect(document.activeElement).toBe(opener());
    await open();
    expect(rows()[2].getAttribute("aria-checked")).toBe("true");
  });

  it("dismisses on Escape and on Back without choosing", async () => {
    const onChange = vi.fn();
    const onClose = vi.fn();
    await render(<Opener onChange={onChange} onClose={onClose} />);
    await open();
    await press("Escape");
    expect(sheet()).toBeNull();
    expect(document.activeElement).toBe(opener());
    await open();
    await click($(".oxy-phone-sheet__back"));
    expect(sheet()).toBeNull();
    expect(onClose).toHaveBeenCalledTimes(2);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("is one Tab stop, on the chosen row, and the arrows move without choosing", async () => {
    const onChange = vi.fn();
    await render(<Opener onChange={onChange} />);
    await open();
    expect(rows().map((r) => r.tabIndex)).toEqual([-1, 0, -1]);
    rows()[1].focus();
    await press("ArrowDown");
    expect(document.activeElement).toBe(rows()[2]);
    await press("ArrowDown");
    expect(document.activeElement).toBe(rows()[0]);
    await press("ArrowUp");
    expect(document.activeElement).toBe(rows()[2]);
    await press("Home");
    expect(document.activeElement).toBe(rows()[0]);
    await press("End");
    expect(document.activeElement).toBe(rows()[2]);
    expect(onChange).not.toHaveBeenCalled();
    expect(sheet()).not.toBeNull();
  });

  it("puts the Tab stop on the first row when nothing is chosen", async () => {
    await render(
      <PhoneOptionSheet
        open
        onClose={() => undefined}
        title='Market'
        options={SORTS}
        value={null}
        onChange={() => undefined}
      />
    );
    expect(rows().map((r) => r.tabIndex)).toEqual([0, -1, -1]);
    expect(rows().every((r) => r.getAttribute("aria-checked") === "false")).toBe(true);
  });
});
