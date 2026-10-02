// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import DataDisplay from "./DataDisplay";
import SqlDisplay from "./SqlDisplay";

// The browser refuses a clipboard write when the page is not focused or the permission
// is denied. The copy buttons here awaited it with no catch, so a refusal surfaced as an
// unhandled rejection. They must swallow it, and only show the check on a real copy.

vi.mock("@/hooks/usePrismTheme", () => ({ default: () => ({}) }));
vi.mock("react-syntax-highlighter", () => ({
  Prism: ({ children }: { children: ReactNode }) => <pre>{children}</pre>
}));

const writeText = vi.fn<(text: string) => Promise<void>>();
const unhandled = vi.fn();

beforeEach(() => {
  writeText.mockReset();
  unhandled.mockReset();
  // After `userEvent.setup()`, which installs a clipboard stub of its own.
  process.on("unhandledRejection", unhandled);
  vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  process.off("unhandledRejection", unhandled);
  cleanup();
});

/** Lets a rejection that nothing caught reach the process-level handler. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

const cases = [
  { name: "SqlDisplay", ui: <SqlDisplay sql='select 1' />, copies: "select 1" },
  {
    name: "DataDisplay",
    ui: <DataDisplay value='plain text' label='Output' />,
    copies: "plain text"
  }
];

describe.each(cases)("$name copy button", ({ ui, copies }) => {
  const setup = () => {
    const user = userEvent.setup();
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { container } = render(ui);
    return { user, container };
  };

  it("shows the check once the text is copied", async () => {
    writeText.mockResolvedValue(undefined);
    const { user, container } = setup();

    await user.click(screen.getByRole("button"));

    expect(writeText).toHaveBeenCalledWith(copies);
    expect(container.querySelector(".lucide-check")).not.toBeNull();
  });

  it("swallows a refused write and does not show the check", async () => {
    writeText.mockRejectedValue(new DOMException("Document is not focused.", "NotAllowedError"));
    const { user, container } = setup();

    await user.click(screen.getByRole("button"));
    await settle();

    expect(writeText).toHaveBeenCalledTimes(1);
    expect(unhandled).not.toHaveBeenCalled();
    expect(container.querySelector(".lucide-check")).toBeNull();
    expect(container.querySelector(".lucide-copy")).not.toBeNull();
  });
});
