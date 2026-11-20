// @vitest-environment jsdom
//
// The button: the whole variant x size matrix from one place, presses that land or do not, the
// busy state that keeps focus and refuses a second press (a submit included), the icon-only
// square that keeps its name, the inert span that is no control at all, and the ref.

import { createRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PlusIcon } from "../icons";
import { ui } from "../recipes";
import { $, click, render } from "../ui.harness";
import { Button, type ButtonSize, type ButtonVariant, buttonClass } from "./Button";

afterEach(() => {
  document.body.innerHTML = "";
});

const button = () => $("button") as HTMLButtonElement;

describe("Button", () => {
  it("draws every variant at every size from the recipes, a 32px secondary by default", async () => {
    const want: Record<`${ButtonVariant}-${ButtonSize}`, string> = {
      "primary-sm": ui.btn.primary,
      "secondary-sm": ui.btn.secondary,
      "text-sm": ui.btn.text,
      "primary-lg": ui.btn.primaryLg,
      "secondary-lg": ui.btn.secondaryLg,
      "text-lg": ui.btn.textLg
    };
    for (const variant of ["primary", "secondary", "text"] as const)
      for (const size of ["sm", "lg"] as const) {
        expect(buttonClass(variant, size)).toBe(want[`${variant}-${size}`]);
        const page = await render(
          <Button variant={variant} size={size}>
            Go
          </Button>
        );
        expect(button().className).toBe(want[`${variant}-${size}`]);
        await page.unmount();
      }
    await render(<Button>Save</Button>);
    expect(button().className).toBe(ui.btn.secondary);
    expect(button().type).toBe("button");
  });

  it("presses: a click reaches onClick, and a disabled button stays still", async () => {
    const onClick = vi.fn();
    const page = await render(<Button onClick={onClick}>Post</Button>);
    await click(button());
    expect(onClick).toHaveBeenCalledTimes(1);
    await page.rerender(
      <Button onClick={onClick} disabled>
        Post
      </Button>
    );
    await click(button());
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("while busy keeps focus, says so, shows progress and ignores presses, then answers again", async () => {
    const onClick = vi.fn();
    const page = await render(
      <Button variant='primary' icon={<PlusIcon />} onClick={onClick}>
        Save
      </Button>
    );
    button().focus();
    await page.rerender(
      <Button variant='primary' icon={<PlusIcon />} onClick={onClick} busy busyLabel='Saving…'>
        Save
      </Button>
    );
    const b = button();
    expect(document.activeElement).toBe(b);
    expect(b.disabled).toBe(false);
    expect(b.getAttribute("aria-busy")).toBe("true");
    expect(b.getAttribute("aria-disabled")).toBe("true");
    expect(b.textContent).toBe("Saving…");
    expect(b.querySelectorAll("svg")).toHaveLength(1);
    expect(b.querySelector("svg")?.getAttribute("class")).toContain("oxy-ui-btn__spinner");
    await click(b);
    expect(onClick).not.toHaveBeenCalled();
    await page.rerender(
      <Button variant='primary' icon={<PlusIcon />} onClick={onClick}>
        Save
      </Button>
    );
    expect(button().hasAttribute("aria-busy")).toBe(false);
    expect(button().hasAttribute("aria-disabled")).toBe(false);
    expect(button().textContent).toBe("Save");
    await click(button());
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("while busy does not submit its form a second time", async () => {
    const onSubmit = vi.fn((e: { preventDefault(): void }) => e.preventDefault());
    await render(
      <form onSubmit={onSubmit}>
        <Button type='submit' variant='primary' busy>
          Send
        </Button>
      </form>
    );
    await click(button());
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("icon-only is a square that keeps its accessible name and hides the label", async () => {
    await render(
      <Button iconOnly variant='text' icon={<PlusIcon />} aria-label='Add a line'>
        Add a line
      </Button>
    );
    const b = button();
    expect(b.className.split(" ")).toContain(ui.btn.iconOnly);
    expect(b.getAttribute("aria-label")).toBe("Add a line");
    expect(b.textContent).toBe("");
    expect(b.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
  });

  it("inert draws the box as a span: no button, no focus, no name to announce", async () => {
    const page = await render(
      <Button inert variant='primary'>
        Post to QuickBooks
      </Button>
    );
    expect(page.container.querySelector("button")).toBeNull();
    const span = page.container.querySelector("span") as HTMLSpanElement;
    expect(span.className).toBe(ui.btn.primary);
    expect(span.textContent).toBe("Post to QuickBooks");
    expect(span.tabIndex).toBe(-1);
    expect(span.hasAttribute("role")).toBe(false);
  });

  it("passes its ref to the <button> (React 19 ref as a prop)", async () => {
    const ref = createRef<HTMLButtonElement>();
    await render(<Button ref={ref}>Open</Button>);
    expect(ref.current).toBe(button());
  });
});
