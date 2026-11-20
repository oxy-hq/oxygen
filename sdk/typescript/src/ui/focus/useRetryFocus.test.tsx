// @vitest-environment jsdom
//
// Where focus goes after Try again (decisions 14 and 28): it never falls to <body>. The loading
// line takes it while the read runs, the start of the new content takes it when the read works,
// the new Try again takes it when the read fails again, and a person who moved away keeps their
// place. Driven on a bare callout / line / content, as LoadError and Loading draw them.

import { useRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { $, click, render } from "../ui.harness";
import {
  kitButton,
  markRetry,
  useHoldFocusDuringRetry,
  useRetryButtonFocus
} from "./useRetryFocus";

afterEach(() => {
  document.body.innerHTML = "";
});

type Phase = "error" | "loading" | "done";

function Callout({ onRetry, kit = false }: { onRetry: () => void; kit?: boolean }) {
  const ref = useRef<HTMLButtonElement>(null);
  const span = useRef<HTMLSpanElement>(null);
  useRetryButtonFocus(ref);
  useRetryButtonFocus(span, kit ? kitButton : undefined);
  return (
    <div>
      <p role='alert'>Couldn't load the stores.</p>
      {kit ? (
        <button
          type='button'
          onClick={() => {
            markRetry();
            onRetry();
          }}
        >
          <span ref={span}>Try again</span>
        </button>
      ) : (
        <button
          ref={ref}
          type='button'
          onClick={(e) => {
            markRetry(e.currentTarget);
            onRetry();
          }}
        >
          Try again
        </button>
      )}
    </div>
  );
}

function Line() {
  const ref = useRef<HTMLParagraphElement>(null);
  useHoldFocusDuringRetry(ref);
  return (
    <p ref={ref} tabIndex={-1} data-retry-focus='' role='status'>
      Loading the stores…
    </p>
  );
}

function Screen({ phase, onRetry, kit }: { phase: Phase; onRetry: () => void; kit?: boolean }) {
  return (
    <main>
      <button type='button' id='elsewhere'>
        Elsewhere
      </button>
      <section>
        <h2>Stores</h2>
        {phase === "error" && <Callout onRetry={onRetry} kit={kit} />}
        {phase === "loading" && <Line />}
        {phase === "done" && <p id='content'>Four stores.</p>}
      </section>
    </main>
  );
}

const tryAgain = () =>
  [...document.querySelectorAll("button")].find((b) => b.textContent === "Try again") ?? null;

async function pressTryAgain(page: Awaited<ReturnType<typeof render>>, onRetry: () => void) {
  const b = tryAgain();
  b?.focus();
  await click(b);
  expect(onRetry).toHaveBeenCalled();
  await page.rerender(<Screen phase='loading' onRetry={onRetry} />);
}

describe("useRetryFocus", () => {
  it("hands focus on: Try again, then the loading line, then the start of the new content", async () => {
    const onRetry = vi.fn();
    const page = await render(<Screen phase='error' onRetry={onRetry} />);
    await pressTryAgain(page, onRetry);
    expect(document.activeElement).toBe($('[role="status"]'));
    await page.rerender(<Screen phase='done' onRetry={onRetry} />);
    await Promise.resolve();
    expect(document.activeElement).not.toBe(document.body);
    expect(document.activeElement).toBe($("#content"));
    // Lent for as long as it holds focus, never a Tab stop of its own.
    expect($("#content")?.getAttribute("tabindex")).toBe("-1");
    $("#elsewhere")?.focus();
    expect($("#content")?.hasAttribute("tabindex")).toBe(false);
  });

  it("gives focus to the new Try again when the read fails again", async () => {
    const onRetry = vi.fn();
    const page = await render(<Screen phase='error' onRetry={onRetry} />);
    await pressTryAgain(page, onRetry);
    await page.rerender(<Screen phase='error' onRetry={onRetry} />);
    await Promise.resolve();
    expect(document.activeElement).toBe(tryAgain());
  });

  it("goes straight to the new content when the re-read draws no loading line", async () => {
    const onRetry = vi.fn();
    const page = await render(<Screen phase='error' onRetry={onRetry} />);
    const b = tryAgain();
    b?.focus();
    await click(b);
    await page.rerender(<Screen phase='done' onRetry={onRetry} />);
    await Promise.resolve();
    expect(document.activeElement).toBe($("#content"));
  });

  it("leaves a person's place alone: someone who moved on keeps it", async () => {
    const onRetry = vi.fn();
    const page = await render(<Screen phase='error' onRetry={onRetry} />);
    const b = tryAgain();
    b?.focus();
    await click(b);
    $("#elsewhere")?.focus();
    await page.rerender(<Screen phase='loading' onRetry={onRetry} />);
    await page.rerender(<Screen phase='done' onRetry={onRetry} />);
    await Promise.resolve();
    expect(document.activeElement).toBe($("#elsewhere"));
  });

  it("moves nothing on a first load that no Try again started", async () => {
    const onRetry = vi.fn();
    const page = await render(<Screen phase='loading' onRetry={onRetry} />);
    await page.rerender(<Screen phase='done' onRetry={onRetry} />);
    await Promise.resolve();
    expect(document.activeElement).toBe(document.body);
  });

  it("finds a kit button from a span inside it (a PhoneRow takes no ref)", async () => {
    const onRetry = vi.fn();
    const page = await render(<Screen phase='error' onRetry={onRetry} kit />);
    await pressTryAgain(page, onRetry);
    await page.rerender(<Screen phase='error' onRetry={onRetry} kit />);
    await Promise.resolve();
    expect(document.activeElement).toBe(tryAgain());
  });
});
