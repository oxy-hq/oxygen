// @vitest-environment jsdom
//
// Decision 16 for the figures and forms groups: on a touch screen every control they draw reaches
// 44px and every text field reads 16px (iOS zooms the page on focus under 16px). The fields and
// selects get both from base.css's touch floor by carrying the field and select recipes, so this
// checks each piece's text controls carry one; the controls the groups draw themselves get theirs
// from a coarse block in their own CSS, which must honour the shell's opt-out as base.css does.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { $$, render } from "../ui.harness";
import { AttachmentsField, SearchField, SelectWithOther, StringListEditor } from "./index";
import { OTHER } from "./SelectWithOther";

afterEach(() => {
  document.body.innerHTML = "";
});

const here = dirname(fileURLToPath(import.meta.url));
const read = (path: string) =>
  readFileSync(join(here, path), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const PREFIX = ':root:not([data-oxy-touch-targets="off"])';

/** The coarse-pointer block of a stylesheet. */
function coarse(css: string): string {
  const at = css.indexOf("@media (pointer: coarse)");
  if (at < 0) return "";
  let depth = 0;
  for (let i = css.indexOf("{", at); i < css.length; i += 1) {
    if (css[i] === "{") depth += 1;
    if (css[i] === "}" && --depth === 0) return css.slice(at, i + 1);
  }
  return "";
}

// base.css's own rule (coarse: `.oxy-ui-field` and `.oxy-ui-select` at 16px) is uiCss.test.ts's
// to hold; what is this group's is that its controls carry those classes.
describe("text fields read 16px on a touch screen", () => {
  it("every text control the forms draw carries one of those recipes", async () => {
    await render(
      <>
        <SearchField label='Find' value='x' onChange={() => {}} />
        <SelectWithOther
          options={[]}
          leading={{ value: "", label: "Select…" }}
          choice={OTHER}
          otherText=''
          otherPlaceholder='Other name'
          onChange={() => {}}
        />
        <StringListEditor label='Codes' values={[]} placeholder='Add' onSave={async () => null} />
        <AttachmentsField files={[]} onAdd={() => {}} onRemove={() => {}} onRetry={() => {}} />
      </>
    );
    const controls = $$("input:not([type='file']), select, textarea");
    expect(controls.length).toBe(4);
    for (const c of controls)
      expect(
        c.classList.contains("oxy-ui-field") || c.classList.contains("oxy-ui-select"),
        c.outerHTML
      ).toBe(true);
  });
});

describe("the groups' own controls reach 44px on a touch screen", () => {
  for (const [file, want] of [
    ["forms.css", [".oxy-ui-toggle-chip", ".oxy-ui-search-field__clear::after"]],
    ["../figures/figures.css", [".oxy-ui-chip__remove::after", ".oxy-ui-alert-strip__door"]]
  ] as const)
    it(`${file}: ${want.join(", ")}, each rule behind the shell's opt-out`, () => {
      const block = coarse(read(file));
      const rules = [...block.matchAll(/([^{}]+)\{([^{}]*)\}/g)].slice(0).map((m) => ({
        selector: m[1].trim(),
        body: m[2]
      }));
      const inner = rules.filter((r) => !r.selector.startsWith("@media"));
      expect(inner.length).toBeGreaterThan(0);
      for (const r of inner) expect(r.selector.startsWith(PREFIX), r.selector).toBe(true);
      for (const sel of want) {
        const r = inner.find((x) => x.selector === `${PREFIX} ${sel}`);
        expect(r, sel).toBeDefined();
        expect(r?.body).toMatch(
          /min-height: var\(--control-h-lg\)|inset: min\(0px, calc\(\(100% - var\(--control-h-lg\)\) \/ 2\)\)/
        );
      }
    });
});
