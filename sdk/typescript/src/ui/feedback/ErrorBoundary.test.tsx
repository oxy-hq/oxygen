// @vitest-environment jsdom
//
// A render that throws loses only the part inside the boundary: it draws its fallback (by default
// a LoadError), reports the error, and starts over on Try again (focus handed on to what renders,
// never dropped to <body>), when its reset keys change, or through a custom fallback's `reset`.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { $, click, render } from "../ui.harness";
import { ErrorBoundary } from "./ErrorBoundary";

let broken = true;
function Part() {
  if (broken) throw new Error("bad row");
  return <p id='part'>The part.</p>;
}

beforeEach(() => {
  broken = true;
  // React reports a caught error on the console; the test reads it through `onError`.
  vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const tryAgain = () =>
  [...document.querySelectorAll("button")].find((b) => b.textContent === "Try again") ?? null;

describe("ErrorBoundary", () => {
  it("renders its children while nothing throws", async () => {
    broken = false;
    await render(
      <ErrorBoundary>
        <Part />
      </ErrorBoundary>
    );
    expect($("#part")).not.toBeNull();
  });

  it("draws a LoadError in place of what threw, the rest of the page kept, and reports it", async () => {
    const onError = vi.fn();
    await render(
      <main>
        <h1 id='title'>Stores</h1>
        <ErrorBoundary onError={onError}>
          <Part />
        </ErrorBoundary>
      </main>
    );
    expect($("#title")).not.toBeNull();
    expect($('[role="alert"]')?.textContent).toContain("This part stopped working");
    expect(onError).toHaveBeenCalledTimes(1);
    expect((onError.mock.calls[0][0] as Error).message).toBe("bad row");
  });

  it("starts over on Try again, focus handed on to what renders", async () => {
    await render(
      <main>
        <ErrorBoundary>
          <Part />
        </ErrorBoundary>
      </main>
    );
    broken = false;
    const b = tryAgain() as HTMLElement;
    b.focus();
    await click(b);
    await Promise.resolve();
    expect($("#part")).not.toBeNull();
    expect(document.activeElement).not.toBe(document.body);
    expect(document.activeElement).toBe($("#part"));
  });

  it("starts over when its reset keys change", async () => {
    const page = await render(
      <ErrorBoundary resetKeys={["a"]}>
        <Part />
      </ErrorBoundary>
    );
    broken = false;
    await page.rerender(
      <ErrorBoundary resetKeys={["a"]}>
        <Part />
      </ErrorBoundary>
    );
    expect($("#part")).toBeNull();
    await page.rerender(
      <ErrorBoundary resetKeys={["b"]}>
        <Part />
      </ErrorBoundary>
    );
    expect($("#part")).not.toBeNull();
  });

  it("hands a custom fallback the error and its reset", async () => {
    await render(
      <ErrorBoundary
        fallback={(error, reset) => (
          <button type='button' onClick={reset}>
            {`Reload (${(error as Error).message})`}
          </button>
        )}
      >
        <Part />
      </ErrorBoundary>
    );
    expect($("button")?.textContent).toBe("Reload (bad row)");
    broken = false;
    await click($("button"));
    expect($("#part")).not.toBeNull();
  });
});
