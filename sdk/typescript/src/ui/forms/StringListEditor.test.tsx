// @vitest-environment jsdom
//
// The list editor: Enter or Add puts a trimmed entry in (a case-insensitive repeat drops out, a
// refused one says why), each entry's × names it and hands focus on when it goes, Save appears
// once the draft differs, stays focused and busy while it runs, and the draft follows the saved
// list when that changes.

import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { KeepAcrossLayouts } from "../../shell/keptState";
import { $, $$, click, press, render } from "../ui.harness";
import { type } from "./forms.harness";
import { StringListEditor } from "./StringListEditor";

afterEach(() => {
  document.body.innerHTML = "";
});

const entries = () => $$(".oxy-ui-chip").map((c) => c.firstChild?.textContent);
const input = () => $("input") as HTMLInputElement;
const button = (name: string) =>
  $$("button").find((b) => b.textContent === name || b.getAttribute("aria-label") === name) as
    | HTMLButtonElement
    | undefined;
const save = vi.fn(async (_values: string[]) => null as string | null);
const editor = (values: string[], extra: Partial<Parameters<typeof StringListEditor>[0]> = {}) => (
  <StringListEditor
    label='Shipping companies'
    values={values}
    placeholder='Add a company'
    onSave={save}
    {...extra}
  />
);

describe("StringListEditor", () => {
  it("adds a trimmed entry on Enter, and drops a repeat in any case", async () => {
    await render(editor(["Matson"]));
    expect(input().getAttribute("aria-label")).toBe("Add to shipping companies");
    input().focus();
    await type(input(), "  Pasha  ");
    await press("Enter");
    expect(entries()).toEqual(["Matson", "Pasha"]);
    expect(input().value).toBe("");
    await type(input(), "matson");
    await press("Enter");
    expect(entries()).toEqual(["Matson", "Pasha"]);
  });

  it("adds with the Add button, which waits for something to add", async () => {
    await render(editor([]));
    expect(button("Add")?.disabled).toBe(true);
    await type(input(), "Young Brothers");
    expect(button("Add")?.disabled).toBe(false);
    await click(button("Add") ?? null);
    expect(entries()).toEqual(["Young Brothers"]);
  });

  it("says why an entry is refused, and stops saying it once you type again", async () => {
    await render(
      editor([], { validateEntry: (s) => (s.includes("@") ? null : "Enter an email address") })
    );
    input().focus();
    await type(input(), "ops");
    await press("Enter");
    expect($("[role='alert']")?.textContent).toBe("Enter an email address");
    expect(input().getAttribute("aria-invalid")).toBe("true");
    expect(entries()).toEqual([]);
    await type(input(), "ops@");
    expect($("[role='alert']")).toBeNull();
    expect(input().hasAttribute("aria-invalid")).toBe(false);
  });

  it("removes by a × named for the entry, handing focus to the next, the one before, then the input", async () => {
    await render(editor(["Matson", "Pasha", "Young"]));
    const x = button("Remove Pasha from shipping companies");
    x?.focus();
    await click(x ?? null);
    expect(entries()).toEqual(["Matson", "Young"]);
    expect(document.activeElement?.getAttribute("aria-label")).toBe(
      "Remove Young from shipping companies"
    );
    await click(document.activeElement);
    expect(document.activeElement?.getAttribute("aria-label")).toBe(
      "Remove Matson from shipping companies"
    );
    await click(document.activeElement);
    expect(entries()).toEqual([]);
    expect(document.activeElement).toBe(input());
  });

  it("offers Save once the draft differs, busy and focused while it runs, then hands focus back", async () => {
    let finish: (err: string | null) => void = () => {};
    const onSave = vi.fn(() => new Promise<string | null>((resolve) => (finish = resolve)));
    await render(editor(["Matson"], { onSave }));
    expect(button("Save")).toBeUndefined();
    input().focus();
    await type(input(), "Pasha");
    await press("Enter");
    expect(document.body.textContent).toContain("Unsaved changes");
    const saveButton = button("Save") as HTMLButtonElement;
    saveButton.focus();
    await click(saveButton);
    expect(onSave).toHaveBeenCalledWith(["Matson", "Pasha"]);
    expect(saveButton.getAttribute("aria-busy")).toBe("true");
    expect(saveButton.textContent).toBe("Saving…");
    expect(document.activeElement).toBe(saveButton);
    await click(saveButton);
    expect(onSave).toHaveBeenCalledTimes(1);
    await finishWith(() => finish("Could not reach the server"));
    expect($("[role='alert']")?.textContent).toBe("Could not reach the server");
    expect(document.activeElement).toBe(saveButton);
    await click(saveButton);
    await finishWith(() => finish(null));
    expect(document.activeElement).toBe(input());
  });

  it("follows the saved list when it changes", async () => {
    const page = await render(editor(["Matson"]));
    await page.rerender(editor(["Matson", "Pasha"]));
    expect(entries()).toEqual(["Matson", "Pasha"]);
    expect(button("Save")).toBeUndefined();
  });
});

async function finishWith(resolve: () => void) {
  const { act } = await import("react");
  await act(async () => {
    resolve();
  });
}

// Across the swap between an app's phone and desk layouts (each its own tree, so the editor
// remounts), what was typed and not saved stays with `keepOn`, as the reference's `keepOn` kept
// its half-typed entry (oxy-hq/reference-app@81ee8aa `src/components/StringListEditor.tsx:36-44`).
/** The app's split: a different tree on each side, under KeepAcrossLayouts. */
function Layouts({ phone, children }: { phone: boolean; children: ReactNode }) {
  return (
    <KeepAcrossLayouts phone={phone} scope='/settings'>
      {phone ? <section>{children}</section> : <div>{children}</div>}
    </KeepAcrossLayouts>
  );
}

describe("StringListEditor across the phone/desk swap", () => {
  const kept = (values: string[]) => editor(values, { keepOn: "/settings" });

  it("keeps the half-typed entry and the unsaved list with keepOn", async () => {
    const page = await render(<Layouts phone={false}>{kept(["Matson"])}</Layouts>);
    input().focus();
    await type(input(), "Pasha");
    await press("Enter");
    await type(input(), "Cros");
    expect(entries()).toEqual(["Matson", "Pasha"]);
    await page.rerender(<Layouts phone>{kept(["Matson"])}</Layouts>);
    expect($("section input")).not.toBeNull();
    expect(input().value).toBe("Cros");
    expect(entries()).toEqual(["Matson", "Pasha"]);
    expect(document.body.textContent).toContain("Unsaved changes");
  });

  it("still follows the saved list when it changes", async () => {
    const page = await render(<Layouts phone={false}>{kept(["Matson"])}</Layouts>);
    input().focus();
    await type(input(), "Pasha");
    await press("Enter");
    await page.rerender(<Layouts phone={false}>{kept(["Matson", "Crowley"])}</Layouts>);
    expect(entries()).toEqual(["Matson", "Crowley"]);
  });

  it("gives two editors mounted at once, neither on a screen, an entry each", async () => {
    await render(
      <Layouts phone={false}>
        {editor([])}
        {editor([])}
      </Layouts>
    );
    const fields = [...document.querySelectorAll("input")];
    await type(fields[0], "Pasha");
    expect(fields.map((f) => f.value)).toEqual(["Pasha", ""]);
  });
});
