// @vitest-environment jsdom
//
// The select with an Other: the sentinel first and greyed only while it is the choice, the real
// options, a trailing "Other…" that reveals a free-text box named for what it stands in for, the
// typed text kept by the caller across a trip away and back, and invalid said on both controls.

import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ui } from "../recipes";
import { $, $$, render } from "../ui.harness";
import { choose, type } from "./forms.harness";
import { OTHER, SelectWithOther } from "./SelectWithOther";

afterEach(() => {
  document.body.innerHTML = "";
});

const SHIPPERS = [
  { value: "matson", label: "Matson" },
  { value: "pasha", label: "Pasha" }
];
const LEADING = { value: "__not_specified", label: "Not known" };

function Controlled({ onChange }: { onChange?: (c: string, t: string) => void }) {
  const [choice, setChoice] = useState(LEADING.value);
  const [text, setText] = useState("");
  return (
    <SelectWithOther
      id='shipper'
      describedBy='shipper-msg'
      options={SHIPPERS}
      leading={LEADING}
      choice={choice}
      otherText={text}
      otherPlaceholder='Shipping company name'
      onChange={(c, t) => {
        setChoice(c);
        setText(t);
        onChange?.(c, t);
      }}
    />
  );
}

const select = () => $("select") as HTMLSelectElement;
const other = () => $("input") as HTMLInputElement | null;

describe("SelectWithOther", () => {
  it("lists the sentinel, the options, then Other, on the comfortable select", async () => {
    await render(<Controlled />);
    expect($$("option").map((o) => o.textContent)).toEqual([
      "Not known",
      "Matson",
      "Pasha",
      "Other…"
    ]);
    expect(($$("option")[3] as HTMLOptionElement).value).toBe(OTHER);
    for (const c of ui.selectTouch.split(" ")) expect(select().classList).toContain(c);
    expect(select().getAttribute("aria-describedby")).toBe("shipper-msg");
  });

  it("greys the closed field while the sentinel is the choice, and only then", async () => {
    await render(<Controlled />);
    expect(select().classList).toContain(ui.selectPlaceholder);
    await choose(select(), "matson");
    expect(select().classList.contains(ui.selectPlaceholder)).toBe(false);
    expect(other()).toBeNull();
  });

  it("opens a named free-text box on Other, and keeps the typed text across a trip away", async () => {
    const onChange = vi.fn();
    await render(<Controlled onChange={onChange} />);
    await choose(select(), OTHER);
    expect(onChange).toHaveBeenLastCalledWith(OTHER, "");
    const box = other();
    expect(box?.getAttribute("aria-label")).toBe("Shipping company name");
    expect(box?.placeholder).toBe("Shipping company name");
    expect(box?.getAttribute("aria-describedby")).toBe("shipper-msg");
    await type(box, "Young Brothers");
    expect(onChange).toHaveBeenLastCalledWith(OTHER, "Young Brothers");
    await choose(select(), "pasha");
    expect(other()).toBeNull();
    expect(onChange).toHaveBeenLastCalledWith("pasha", "Young Brothers");
    await choose(select(), OTHER);
    expect(other()?.value).toBe("Young Brothers");
  });

  it("says it is invalid on both controls", async () => {
    await render(
      <SelectWithOther
        options={SHIPPERS}
        leading={LEADING}
        choice={OTHER}
        otherText=''
        otherPlaceholder='Shipping company name'
        onChange={() => {}}
        invalid
      />
    );
    for (const el of [select(), other()]) {
      expect(el?.getAttribute("aria-invalid")).toBe("true");
      expect(el?.classList).toContain(ui.errorInput);
    }
  });
});
