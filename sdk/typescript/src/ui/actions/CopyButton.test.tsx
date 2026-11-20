// @vitest-environment jsdom
//
// CopyButton, pressed as a person presses it: the text lands on the clipboard and a polite
// status beside the button says "Copied" without moving focus, then clears; a refused Clipboard
// API falls back to the old copy command (focus comes back to the button), and when the browser
// refuses both, the status says so in words.

import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { $, click, render } from "../ui.harness";
import { CopyButton } from "./CopyButton";

const status = () => $('[role="status"]') as HTMLElement;
const button = () => $("button") as HTMLButtonElement;

let writeText: ReturnType<typeof vi.fn>;
beforeEach(() => {
  writeText = vi.fn(async () => undefined);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText }
  });
});

afterEach(() => {
  document.body.innerHTML = "";
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: undefined });
  Reflect.deleteProperty(document, "execCommand");
  vi.useRealTimers();
});

async function press() {
  button().focus();
  await click(button());
  // The clipboard's promise.
  await React.act(async () => {
    await Promise.resolve();
  });
}

describe("CopyButton", () => {
  it("has its live region before anything is copied, so the change is announced", async () => {
    await render(<CopyButton value='abc' />);
    expect(status()).not.toBeNull();
    expect(status().textContent).toBe("");
    expect(button().textContent).toBe("Copy");
  });

  it("copies the value, says Copied beside the button, keeps focus, and clears after a while", async () => {
    vi.useFakeTimers();
    const onCopy = vi.fn();
    await render(<CopyButton value='MSKU0223442' onCopy={onCopy} resetAfter={1500} />);
    await press();
    expect(writeText).toHaveBeenCalledWith("MSKU0223442");
    expect(status().textContent).toBe("Copied");
    expect(document.activeElement).toBe(button());
    expect(onCopy).toHaveBeenCalledWith(true);
    await React.act(async () => {
      vi.advanceTimersByTime(1499);
    });
    expect(status().textContent).toBe("Copied");
    await React.act(async () => {
      vi.advanceTimersByTime(1);
    });
    expect(status().textContent).toBe("");
  });

  it("reads a value given as a function at the press", async () => {
    let n = 1;
    await render(<CopyButton value={() => `row ${n}`} />);
    n = 2;
    await press();
    expect(writeText).toHaveBeenCalledWith("row 2");
  });

  it("falls back to the copy command when the Clipboard API refuses, and gives focus back", async () => {
    writeText.mockRejectedValue(new DOMException("denied", "NotAllowedError"));
    let selected = "";
    const execCommand = vi.fn(() => {
      selected = (document.activeElement as HTMLTextAreaElement).value;
      return true;
    });
    Object.defineProperty(document, "execCommand", { configurable: true, value: execCommand });
    await render(<CopyButton value='abc-123' />);
    await press();
    expect(execCommand).toHaveBeenCalledWith("copy");
    expect(selected).toBe("abc-123");
    expect(document.querySelector("textarea")).toBeNull();
    expect(document.activeElement).toBe(button());
    expect(status().textContent).toBe("Copied");
  });

  it("says so in words when the browser refuses both ways, and keeps it until the next press", async () => {
    vi.useFakeTimers();
    writeText.mockRejectedValue(new DOMException("denied", "NotAllowedError"));
    Object.defineProperty(document, "execCommand", { configurable: true, value: () => false });
    const onCopy = vi.fn();
    await render(<CopyButton value='abc' onCopy={onCopy} failedLabel="Couldn't copy the link." />);
    await press();
    expect(status().textContent).toBe("Couldn't copy the link.");
    expect(onCopy).toHaveBeenCalledWith(false);
    await React.act(async () => {
      vi.advanceTimersByTime(10_000);
    });
    expect(status().textContent).toBe("Couldn't copy the link.");
    expect(document.activeElement).toBe(button());
  });

  it("says so too where there is no Clipboard API and no copy command at all", async () => {
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: undefined });
    Object.defineProperty(document, "execCommand", { configurable: true, value: undefined });
    await render(<CopyButton value='abc' />);
    await press();
    expect(status().textContent).toBe("Couldn't copy. Select it and copy it yourself.");
  });

  it("is a Button: its variant, size and name pass through", async () => {
    await render(<CopyButton value='abc' variant='text' aria-label='Copy the link' />);
    expect(button().className).toContain("oxy-ui-btn--text");
    expect(button().getAttribute("aria-label")).toBe("Copy the link");
  });
});
