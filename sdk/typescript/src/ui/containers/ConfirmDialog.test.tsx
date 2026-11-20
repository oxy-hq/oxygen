// @vitest-environment jsdom
//
// The centred confirm (decision 43): the safe answer takes focus first, on the desk (Cancel) and
// on a phone (the sheet's Back, named with the cancel word); the confirm is the primary; Escape
// cancels; and while the action runs both answers keep focus and refuse a second press, so focus
// never drops to <body> (a `disabled` answer would drop it).

import { afterEach, describe, expect, it, vi } from "vitest";
import { ui } from "../recipes";
import { $, click, press, render } from "../ui.harness";
import { ConfirmDialog } from "./ConfirmDialog";
import { stubMedia } from "./containers.harness";

afterEach(() => {
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
});

const named = (text: string) =>
  [...document.querySelectorAll("button")].find((b) => b.textContent === text) as HTMLElement;

function Ask(props: { busy?: boolean; onConfirm?: () => void; onCancel?: () => void }) {
  return (
    <ConfirmDialog
      title='Delete RR-41?'
      confirmLabel='Delete'
      busyLabel='Deleting…'
      busy={props.busy}
      onConfirm={props.onConfirm ?? (() => {})}
      onCancel={props.onCancel ?? (() => {})}
    >
      <p>The report and its photos go for good.</p>
    </ConfirmDialog>
  );
}

describe("ConfirmDialog on the desk", () => {
  it("starts on the safe answer, the confirm the primary", async () => {
    await render(<Ask />);
    expect(document.activeElement).toBe(named("Cancel"));
    expect(named("Delete").className).toBe(ui.btn.primary);
    expect(named("Cancel").className).toBe(ui.btn.secondary);
    expect($("dialog")?.textContent).toContain("Delete RR-41?");
  });

  it("answers: Delete confirms, Cancel and Escape cancel", async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    await render(<Ask onConfirm={onConfirm} onCancel={onCancel} />);
    await click(named("Delete"));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    await click(named("Cancel"));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("cancels on Escape", async () => {
    stubMedia({ reduce: true });
    const onCancel = vi.fn();
    await render(<Ask onCancel={onCancel} />);
    await press("Escape");
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("while busy keeps focus on the answer pressed, says so, and refuses a second press", async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const page = await render(<Ask onConfirm={onConfirm} onCancel={onCancel} />);
    named("Delete").focus();
    await click(named("Delete"));
    await page.rerender(<Ask busy onConfirm={onConfirm} onCancel={onCancel} />);
    const confirm = named("Deleting…");
    expect(document.activeElement).toBe(confirm);
    expect((confirm as HTMLButtonElement).disabled).toBe(false);
    expect(confirm.getAttribute("aria-busy")).toBe("true");
    expect(named("Cancel").getAttribute("aria-disabled")).toBe("true");
    await click(confirm);
    await click(named("Cancel"));
    await press("Escape");
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onCancel).not.toHaveBeenCalled();
    expect(document.activeElement).not.toBe(document.body);
  });
});

describe("ConfirmDialog on a phone", () => {
  it("is the kit's sheet, Back named with the cancel word and focused first", async () => {
    stubMedia({ phone: true });
    const onCancel = vi.fn();
    await render(<Ask onCancel={onCancel} />);
    expect($("dialog")).toBeNull();
    const back = $(".oxy-phone-sheet__back");
    expect(back?.textContent).toBe("Cancel");
    expect(document.activeElement).toBe(back);
    await click(back);
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("while busy says so and refuses presses, without disabling the answers", async () => {
    stubMedia({ phone: true });
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    await render(<Ask busy onConfirm={onConfirm} onCancel={onCancel} />);
    const confirm = named("Deleting…");
    expect((confirm as HTMLButtonElement).disabled).toBe(false);
    await click(confirm);
    await click(named("Cancel"));
    await click($(".oxy-phone-sheet__back"));
    expect(onConfirm).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
  });
});
