// @vitest-environment jsdom
//
// The field: a label that names its control, then the control, then the error OR the hint under
// one id, wired onto the control by the field itself (`aria-describedby` while there is a
// message, `aria-invalid` while there is an error), keeping what the control already says.

import { afterEach, describe, expect, it } from "vitest";
import { ui } from "../recipes";
import { $, render } from "../ui.harness";
import { describedBy, Field } from "./Field";

afterEach(() => {
  document.body.innerHTML = "";
});

const input = () => $("input") as HTMLInputElement;
const described = (el: Element) =>
  (el.getAttribute("aria-describedby") ?? "")
    .split(/\s+/)
    .filter(Boolean)
    .map((id) => document.getElementById(id)?.textContent);

describe("Field", () => {
  it("names its control with a label in the label role", async () => {
    await render(
      <Field label='Vendor' htmlFor='vendor'>
        <input id='vendor' className={ui.input} />
      </Field>
    );
    const label = $("label") as HTMLLabelElement;
    expect(label.htmlFor).toBe("vendor");
    expect(label.className).toBe(ui.label);
    expect(input().labels?.[0]?.textContent).toBe("Vendor");
    expect(input().hasAttribute("aria-describedby")).toBe(false);
    expect(input().hasAttribute("aria-invalid")).toBe(false);
  });

  it("describes the control by its hint while nothing is wrong", async () => {
    await render(
      <Field label='Vendor' htmlFor='vendor' hint='As on the packing slip'>
        <input className={ui.input} />
      </Field>
    );
    expect(input().id).toBe("vendor");
    expect(input().getAttribute("aria-describedby")).toBe(describedBy("vendor"));
    expect(described(input())).toEqual(["As on the packing slip"]);
    expect(input().hasAttribute("aria-invalid")).toBe(false);
    expect($("[role='alert']")).toBeNull();
  });

  it("swaps the hint for the error, announced, and marks the control invalid", async () => {
    await render(
      <Field label='Vendor' htmlFor='vendor' hint='As on the packing slip' error='Pick a vendor'>
        <input className={ui.input} />
      </Field>
    );
    const alert = $("[role='alert']") as HTMLElement;
    expect(alert.textContent).toBe("Pick a vendor");
    expect(alert.id).toBe(describedBy("vendor"));
    expect(described(input())).toEqual(["Pick a vendor"]);
    expect(input().getAttribute("aria-invalid")).toBe("true");
    expect(document.body.textContent).not.toContain("As on the packing slip");
  });

  it("keeps what the control already names and says", async () => {
    await render(
      <>
        <p id='rules'>Letters only</p>
        <Field label='Code' id='code' error='Too long'>
          <input id='code' aria-describedby='rules' className={ui.input} />
        </Field>
      </>
    );
    expect(described(input())).toEqual(["Letters only", "Too long"]);
    expect(($("label") as HTMLLabelElement).htmlFor).toBe("code");
  });

  it("hands its wiring to a function child, for a component control", async () => {
    const page = await render(
      <Field label='Count' htmlFor='count' error='Required'>
        {(control) => <input {...control} className={ui.input} />}
      </Field>
    );
    expect(input().id).toBe("count");
    expect(input().getAttribute("aria-invalid")).toBe("true");
    expect(described(input())).toEqual(["Required"]);
    await page.rerender(
      <Field label='Count' htmlFor='count'>
        {(control) => <input {...control} className={ui.input} />}
      </Field>
    );
    expect(input().hasAttribute("aria-describedby")).toBe(false);
    expect(input().hasAttribute("aria-invalid")).toBe(false);
  });

  it("wires only a labelable control: a wrapper round an input and its unit is left alone (2.31.0)", async () => {
    await render(
      <Field id='in-cap' label='Weekly cap' hint='Most a store may order in a week.'>
        <div className='with-unit'>
          <input id='in-cap' aria-describedby={describedBy("in-cap")} />
          <span>cases</span>
        </div>
      </Field>
    );
    const wrap = $(".with-unit") as HTMLElement;
    expect(wrap.hasAttribute("id")).toBe(false);
    expect(wrap.hasAttribute("aria-describedby")).toBe(false);
    expect(document.querySelectorAll("#in-cap")).toHaveLength(1);
    // The label names the input the caller wired by hand.
    expect(($("label") as HTMLLabelElement).control).toBe(input());
  });

  it("labels a composite control with a span and leaves it unwired", async () => {
    await render(
      <Field label='People' hint='Everyone on the dock'>
        <div data-picker=''>
          <input aria-label='Add a person' />
        </div>
      </Field>
    );
    expect($("label")).toBeNull();
    expect($(".oxy-ui-form-field__label-row span")?.textContent).toBe("People");
    expect(input().hasAttribute("aria-describedby")).toBe(false);
    expect($(".oxy-ui-note")?.hasAttribute("id")).toBe(false);
  });
});
