// @vitest-environment jsdom
//
// The on-row ask (decision 43, Store Ops Tablets): Revoke puts the question in the buttons'
// place, Keep it (the safe answer) takes focus, the group is named by the question, Keep it and
// Escape put the question away with focus back on Revoke, Revoke it confirms, and while busy both
// answers keep focus and refuse a second press.

import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ui } from "../recipes";
import { $, click, press, render } from "../ui.harness";
import { ConfirmInline, confirmRowClass } from "./ConfirmInline";
import { declarations } from "./containers.harness";

afterEach(() => {
  document.body.innerHTML = "";
});

const named = (text: string) =>
  [...document.querySelectorAll("button")].find((b) => b.textContent === text) as HTMLElement;

function Row({ busy = false, onConfirm = () => {} }: { busy?: boolean; onConfirm?: () => void }) {
  const [asking, setAsking] = useState(false);
  return (
    <table>
      <tbody>
        <tr id='row' className={asking ? confirmRowClass : undefined}>
          <td>Front counter iPad</td>
          <td>
            <ConfirmInline
              open={asking}
              question="Revoke Front counter iPad? Crew can't sign in on it after this."
              confirmLabel='Revoke it'
              busyLabel='Revoking…'
              busy={busy}
              onConfirm={onConfirm}
              onCancel={() => setAsking(false)}
            >
              <button type='button'>New link</button>
              <button type='button' onClick={() => setAsking(true)}>
                Revoke
              </button>
            </ConfirmInline>
          </td>
        </tr>
      </tbody>
    </table>
  );
}

async function ask() {
  named("Revoke").focus();
  await click(named("Revoke"));
}

describe("ConfirmInline", () => {
  it("asks in the buttons' place, Keep it focused, the question naming the answers' group", async () => {
    await render(<Row />);
    await ask();
    expect(document.activeElement).toBe(named("Keep it"));
    expect($(".oxy-ui-confirm-inline__triggers")?.hidden).toBe(true);
    const group = $('[role="group"]');
    const label = document.getElementById(group?.getAttribute("aria-labelledby") ?? "");
    expect(label?.textContent).toContain("Revoke Front counter iPad?");
    expect(named("Revoke it").className).toBe(ui.btn.primary);
    expect($("#row")?.className).toBe(confirmRowClass);
  });

  it("Keep it puts the question away, focus back on the Revoke that asked", async () => {
    await render(<Row />);
    await ask();
    await click(named("Keep it"));
    expect($('[role="group"]')).toBeNull();
    expect($(".oxy-ui-confirm-inline__triggers")?.hidden).toBe(false);
    expect(document.activeElement).toBe(named("Revoke"));
  });

  it("Escape keeps it too, and goes no further (a panel around the row stays open)", async () => {
    const outer = vi.fn();
    document.addEventListener("keydown", outer);
    await render(<Row />);
    await ask();
    await press("Escape");
    expect(document.activeElement).toBe(named("Revoke"));
    const escapeKey = outer.mock.calls
      .map((c) => c[0] as KeyboardEvent)
      .find((e) => e.key === "Escape");
    expect(escapeKey?.defaultPrevented).toBe(true);
    document.removeEventListener("keydown", outer);
  });

  it("Revoke it confirms; while busy both answers keep focus and refuse presses", async () => {
    const onConfirm = vi.fn();
    const page = await render(<Row onConfirm={onConfirm} />);
    await ask();
    named("Revoke it").focus();
    await click(named("Revoke it"));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    await page.rerender(<Row busy onConfirm={onConfirm} />);
    const working = named("Revoking…");
    expect(document.activeElement).toBe(working);
    expect(working.getAttribute("aria-busy")).toBe("true");
    await click(working);
    await click(named("Keep it"));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect($('[role="group"]')).not.toBeNull();
  });

  it("washes the asking row neutral, over its hover too", () => {
    expect(declarations(".oxy-ui-confirm-row")).toMatch(
      /background-color: var\(--status-neutral-bg\)/
    );
    expect(declarations(".oxy-ui-confirm-row:hover", "hover: hover")).toMatch(
      /background-color: var\(--status-neutral-bg\)/
    );
  });
});
