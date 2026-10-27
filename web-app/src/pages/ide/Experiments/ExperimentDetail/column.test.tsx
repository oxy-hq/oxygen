// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { COLUMN } from "./column";
import PendingView from "./components/ResultPane/components/PendingView";
import SealedView from "./components/ResultPane/components/SealedView";

afterEach(cleanup);

/** Every state's body shares the Definition's column, so their left edges line up. */
describe("the detail column", () => {
  it("is the same width as the Definition card", () => {
    expect(COLUMN).toContain("max-w-4xl");
    expect(COLUMN).toContain("mx-auto");
  });

  it.each([
    ["pending", () => render(<PendingView state='aborted' />), "experiment-report-pending-view"],
    ["sealed", () => render(<SealedView state='live' />), "experiment-sealed"]
  ])("puts the %s view in that column", (_, mount, testId) => {
    mount();
    const root = screen.getByTestId(testId);
    for (const c of COLUMN.split(" ")) expect(root).toHaveClass(c);
    expect(root.className).not.toMatch(/max-w-3xl/);
  });
});
