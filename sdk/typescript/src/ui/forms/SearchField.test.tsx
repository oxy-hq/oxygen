// @vitest-environment jsdom
//
// The search field: a named `type="search"` field with its magnifier, a clear button that shows
// only with text and gives focus back to the field, Escape that clears text and otherwise goes
// on up to a dialog, the toolbar and entry sizes, and the one filter rule, `matchesQuery`.

import { createRef, useState } from "react";
import { afterAll, afterEach, describe, expect, it, vi } from "vitest";
import { paintUi, unpaintUi } from "../paint.harness";
import { ui } from "../recipes";
import { $, click, press, render } from "../ui.harness";
import { type } from "./forms.harness";
import { matchesQuery, SearchField } from "./SearchField";

afterEach(() => {
  document.body.innerHTML = "";
});

const field = () => $("input") as HTMLInputElement;
const clearButton = () => $("button") as HTMLButtonElement | null;

function Search({ onChange }: { onChange?: (v: string) => void }) {
  const [q, setQ] = useState("");
  return (
    <SearchField
      label='Search SKU or item'
      value={q}
      onChange={(v) => {
        setQ(v);
        onChange?.(v);
      }}
    />
  );
}

describe("SearchField", () => {
  it("is a named search field with its magnifier, the compact field by default", async () => {
    await render(<Search />);
    expect(field().type).toBe("search");
    expect(field().getAttribute("aria-label")).toBe("Search SKU or item");
    expect(field().placeholder).toBe("Search SKU or item");
    for (const c of ui.inputSm.split(" ")) expect(field().classList).toContain(c);
    expect($(".oxy-ui-search-field__glass")?.getAttribute("aria-hidden")).toBe("true");
    expect(clearButton()).toBeNull();
  });

  it("reports what is typed, and clears with a × that hands focus back to the field", async () => {
    const onChange = vi.fn();
    await render(<Search onChange={onChange} />);
    await type(field(), "cf");
    expect(onChange).toHaveBeenLastCalledWith("cf");
    const x = clearButton();
    expect(x?.getAttribute("aria-label")).toBe("Clear search");
    expect(x?.type).toBe("button");
    x?.focus();
    await click(x);
    expect(onChange).toHaveBeenLastCalledWith("");
    expect(field().value).toBe("");
    expect(document.activeElement).toBe(field());
    expect(clearButton()).toBeNull();
  });

  it("clears on Escape and keeps that Escape, but lets an empty field's Escape go on up", async () => {
    const outer = vi.fn();
    window.addEventListener("keydown", outer);
    try {
      await render(<Search />);
      field().focus();
      await type(field(), "cf");
      await press("Escape");
      expect(field().value).toBe("");
      expect(outer).not.toHaveBeenCalled();
      await press("Escape");
      expect(outer).toHaveBeenCalledTimes(1);
    } finally {
      window.removeEventListener("keydown", outer);
    }
  });

  it("draws the entry size and the quiet edge on request, and passes its ref to the field", async () => {
    const ref = createRef<HTMLInputElement>();
    await render(
      <SearchField label='Find' value='' onChange={() => {}} size='lg' quiet ref={ref} />
    );
    expect(field().classList).toContain(ui.input);
    expect(field().classList.contains("oxy-ui-field--sm")).toBe(false);
    expect(field().classList).toContain("oxy-ui-field--quiet");
    expect(ref.current).toBe(field());
  });
});

describe("matchesQuery", () => {
  it("trims, lowercases and needs every word", () => {
    expect(matchesQuery("Cascade Freight", "  cascade  ")).toBe(true);
    expect(matchesQuery("Cascade Freight", "FREIGHT cas")).toBe(true);
    expect(matchesQuery("Cascade Freight", "cascade matson")).toBe(false);
  });

  it("reads several fields as one, skipping the empty ones", () => {
    expect(matchesQuery(["Cascade Freight", null, "CF-201"], "cf-201 freight")).toBe(true);
    expect(matchesQuery([undefined, "Pasha"], "pasha")).toBe(true);
  });

  it("matches everything for an empty query, and nothing for an empty text", () => {
    expect(matchesQuery("anything", "   ")).toBe(true);
    expect(matchesQuery(null, "")).toBe(true);
    expect(matchesQuery(null, "a")).toBe(false);
    expect(matchesQuery("", "a")).toBe(false);
  });
});

// Where the magnifier sits, with ui.css painted on: the reference's own finds. The compact one
// (oxy-hq/reference-app@81ee8aa `src/components/ReorderSheet.tsx:164-177`) has it at the END,
// `right-3`, the text clear of it by `pr-9`; the 44px ones (`WarehouseReports.tsx:150-161`,
// `ItemPickerSheet.tsx:102-115`) at the start, `left-4`, the text after it at `pl-11`. The compact
// one was drawn with the magnifier at the start once (SDK-GAPS 5).
describe("SearchField, painted", () => {
  afterAll(() => unpaintUi());
  const style = (sel: string) => getComputedStyle($(sel) as HTMLElement);
  const glass = () => style(".oxy-ui-search-field__glass");
  const input = () => style(".oxy-ui-search-field__input");

  it("puts the compact find's magnifier at the end, 12px in, the text 12px to 36px clear", async () => {
    paintUi();
    await render(<SearchField label='Search SKU or item' value='' onChange={() => {}} />);
    expect(glass().right).toBe("12px");
    expect(glass().left).not.toBe("12px");
    expect([input().paddingLeft, input().paddingRight]).toEqual(["12px", "36px"]);
  });

  it("puts the × in the compact find's magnifier place while it holds text", async () => {
    paintUi();
    await render(<SearchField label='Search SKU or item' value='cf' onChange={() => {}} />);
    expect(glass().display).toBe("none");
    // 24px wide, 8px in: centred 20px from the end, where the 16px magnifier's centre was.
    expect([
      style(".oxy-ui-search-field__clear").right,
      style(".oxy-ui-search-field__clear").width
    ]).toEqual(["8px", "24px"]);
    expect(input().paddingRight).toBe("36px");
  });

  it("puts the 44px field's magnifier at the start, 16px in, its text at 44px", async () => {
    paintUi();
    await render(<SearchField size='lg' label='Search reports' value='' onChange={() => {}} />);
    expect(glass().left).toBe("16px");
    expect(glass().right).toBe("auto");
    expect(input().paddingLeft).toBe("44px");
  });
});
