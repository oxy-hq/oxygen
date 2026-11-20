// @vitest-environment jsdom
//
// A failed read (decisions 14 and 28): the message is the alert and Try again sits outside it; Try
// again re-reads only the part that failed (its own `onRetry`, or the nearest RereadBoundary); and
// focus never lands on <body> after it: Try again, then the loading line, then the start of the new
// content, or the next Try again. The reference's LoadError drops focus to <body> here (its Try
// again goes with the remount), which the hand-on test fails on.

import { act, useEffect, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RereadBoundary } from "../focus/useReread";
import { ui } from "../recipes";
import { $, $$, click, render } from "../ui.harness";
import { LoadError } from "./LoadError";
import { LoadingLine } from "./Loading";

afterEach(() => {
  document.body.innerHTML = "";
  reads.length = 0;
});

const tryAgain = () =>
  [...document.querySelectorAll("button")].find((b) => b.textContent?.includes("Try again")) ??
  null;

/** Reads the test answers, one per mount, as a hook that fetches in an effect does. */
const reads: ((ok: boolean) => void)[] = [];
function Stores() {
  const [state, setState] = useState<"loading" | "failed" | "done">("loading");
  useEffect(() => {
    let live = true;
    reads.push((ok) => live && setState(ok ? "done" : "failed"));
    return () => {
      live = false;
    };
  }, []);
  if (state === "loading") return <LoadingLine>Loading the stores…</LoadingLine>;
  if (state === "failed")
    return <LoadError title="Couldn't load the stores">The server did not answer.</LoadError>;
  return <p id='content'>Four stores.</p>;
}

function Screen() {
  return (
    <main>
      <h2>Stores</h2>
      <RereadBoundary>
        <Stores />
      </RereadBoundary>
    </main>
  );
}

/** A part outside the boundary, counting its mounts. */
const outside = vi.fn();
function Outside() {
  useEffect(() => outside(), []);
  return <p>Header</p>;
}

const answer = async (ok: boolean) => {
  await act(async () => {
    reads[reads.length - 1](ok);
  });
  await act(async () => {});
};

describe("LoadError", () => {
  it("is an alert holding the claim and the detail, with Try again beside it, outside", async () => {
    await render(
      <LoadError title="Couldn't load templates" onRetry={() => {}}>
        The server did not answer.
      </LoadError>
    );
    const alert = $('[role="alert"]') as HTMLElement;
    expect(alert.textContent).toBe("Couldn't load templatesThe server did not answer.");
    for (const c of ui.callout.split(" ")) expect(alert.classList.contains(c)).toBe(true);
    expect(alert.classList.contains(ui.calloutTone.error)).toBe(true);
    const button = tryAgain() as HTMLButtonElement;
    expect(alert.contains(button)).toBe(false);
    expect(button.querySelector("svg")).not.toBeNull();
    expect(button.className).toBe(ui.btn.secondary);
  });

  it("takes the warning tone and the 44px button when asked", async () => {
    await render(
      <LoadError tone='warning' size='lg' onRetry={() => {}}>
        The latest purchases did not load.
      </LoadError>
    );
    expect($('[role="alert"]')?.classList.contains(ui.calloutTone.warning)).toBe(true);
    expect(tryAgain()?.className).toBe(ui.btn.secondaryLg);
  });

  it("reads again with its own onRetry; offers nothing with noRetry or with nothing to call", async () => {
    const onRetry = vi.fn();
    const page = await render(<LoadError onRetry={onRetry}>Failed.</LoadError>);
    await click(tryAgain());
    expect(onRetry).toHaveBeenCalledTimes(1);
    await page.rerender(
      <LoadError onRetry={onRetry} noRetry>
        Failed.
      </LoadError>
    );
    expect(tryAgain()).toBeNull();
    await page.rerender(<LoadError>Failed.</LoadError>);
    expect(tryAgain()).toBeNull();
  });

  it("while retrying keeps focus, says so, and does not ask twice", async () => {
    const onRetry = vi.fn();
    const page = await render(<LoadError onRetry={onRetry}>Failed.</LoadError>);
    tryAgain()?.focus();
    await page.rerender(
      <LoadError onRetry={onRetry} retrying>
        Failed.
      </LoadError>
    );
    const b = $("button") as HTMLButtonElement;
    expect(document.activeElement).toBe(b);
    expect(b.disabled).toBe(false);
    expect(b.getAttribute("aria-busy")).toBe("true");
    expect(b.textContent).toBe("Trying again…");
    await click(b);
    expect(onRetry).not.toHaveBeenCalled();
  });

  it("re-reads only the boundary it is in", async () => {
    outside.mockClear();
    await render(
      <>
        <Outside />
        <Screen />
      </>
    );
    await answer(false);
    await click(tryAgain());
    expect(reads).toHaveLength(2);
    expect(outside).toHaveBeenCalledTimes(1);
  });

  it("never drops focus to <body> after Try again: the line, then the new content", async () => {
    await render(<Screen />);
    await answer(false);
    const first = tryAgain() as HTMLElement;
    first.focus();
    await click(first);
    expect(document.activeElement).not.toBe(document.body);
    expect(document.activeElement).toBe($('[role="status"]'));
    await answer(true);
    expect(document.activeElement).not.toBe(document.body);
    expect(document.activeElement).toBe($("#content"));
  });

  it("gives focus to the new Try again when the read fails again", async () => {
    await render(<Screen />);
    await answer(false);
    tryAgain()?.focus();
    await click(tryAgain());
    await answer(false);
    expect($$('[role="alert"]')).toHaveLength(1);
    expect(document.activeElement).toBe(tryAgain());
  });
});
