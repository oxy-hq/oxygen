// @vitest-environment jsdom
//
// Try again on its own (2.31.0): it reads again with its own onRetry or the nearest
// RereadBoundary's, draws nothing with neither, hands focus on to the loading line as LoadError's
// does, while retrying keeps focus and refuses a second press, and carries `data-oxy-try-again`
// (LoadError's too) for an app to aim at. LoadingBar's progress defaults to one read.

import { act, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RereadBoundary } from "../focus/useReread";
import { ui } from "../recipes";
import { $, $$, click, render } from "../ui.harness";
import { LoadError, TryAgainButton } from "./LoadError";
import { LoadingBar, LoadingLine } from "./Loading";

afterEach(() => {
  document.body.innerHTML = "";
});

const button = () => $("button") as HTMLButtonElement;

/** Counts its mounts: a re-read mounts the part again. */
let mounts = 0;
function Part() {
  mounts += 1;
  return <TryAgainButton />;
}

/** A read that worked and found nothing, with Try again beside the empty list; the test settles
 *  each re-read with `settle()`, empty again. */
let settle: () => void = () => {};
function Empty() {
  const [state, setState] = useState<"empty" | "loading">("empty");
  settle = () => setState("empty");
  return state === "loading" ? (
    <LoadingLine>Loading the items…</LoadingLine>
  ) : (
    <TryAgainButton onRetry={() => setState("loading")} />
  );
}

/** A re-read with no loading line between: Try again goes, and the list takes its place. */
function Found() {
  const [found, setFound] = useState(false);
  return found ? (
    <ul>
      <li>3 items</li>
    </ul>
  ) : (
    <TryAgainButton onRetry={() => setFound(true)} />
  );
}

describe("TryAgainButton", () => {
  it("reads again with its own onRetry, as a 32px secondary named Try again", async () => {
    const onRetry = vi.fn();
    await render(<TryAgainButton onRetry={onRetry} />);
    expect(button().textContent).toBe("Try again");
    expect(button().className).toContain(ui.btn.secondary);
    expect(button().hasAttribute("data-oxy-try-again")).toBe(true);
    await click(button());
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("takes the boundary's re-read by default, and draws nothing with nothing to call", async () => {
    mounts = 0;
    await render(
      <RereadBoundary>
        <Part />
      </RereadBoundary>
    );
    expect(mounts).toBe(1);
    await click(button());
    expect(mounts).toBe(2);
    document.body.innerHTML = "";
    await render(<TryAgainButton />);
    expect($$("button")).toHaveLength(0);
  });

  it("hands focus on to the loading line and to the next Try again, and while retrying keeps focus and refuses a press", async () => {
    await render(<Empty />);
    button().focus();
    await click(button());
    expect(document.activeElement?.textContent).toBe("Loading the items…");
    // Still nothing: the new Try again takes focus, never <body>.
    await act(async () => settle());
    expect(document.activeElement).toBe(button());
    document.body.innerHTML = "";
    const onRetry = vi.fn();
    await render(<TryAgainButton onRetry={onRetry} retrying size='lg' label='Reload' />);
    button().focus();
    await click(button());
    expect(onRetry).not.toHaveBeenCalled();
    expect(button().getAttribute("aria-busy")).toBe("true");
    expect(document.activeElement).toBe(button());
    expect(button().className).toContain(ui.btn.secondaryLg);
  });

  it("gives focus to what took its place when it goes with no loading line between", async () => {
    await render(<Found />);
    button().focus();
    await click(button());
    expect(document.activeElement).not.toBe(document.body);
    expect(document.activeElement?.textContent).toContain("3 items");
  });

  it("is LoadError's Try again too: the same mark on it", async () => {
    await render(<LoadError onRetry={() => {}}>The counts did not load.</LoadError>);
    expect(button().hasAttribute("data-oxy-try-again")).toBe(true);
    // The look LoadError's lg test pins is unchanged: the recipe alone.
    expect(button().className).toBe(ui.btn.secondary);
  });
});

describe("LoadingBar's progress", () => {
  it("defaults to one read, not back yet: a sliver and its honest count", async () => {
    await act(async () => {
      await render(<LoadingBar label='Loading the counts…' />);
    });
    const bar = $('[role="status"]') as HTMLElement;
    expect(bar.getAttribute("aria-label")).toBe("Loading the counts… — 0 of 1 parts ready");
    expect(($(".oxy-ui-loading-bar__fill") as HTMLElement).style.width).toBe("4%");
  });
});
