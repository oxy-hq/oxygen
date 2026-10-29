// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WorkspaceClock } from "./WorkspaceClock";

vi.mock("@/hooks/api/agents/useWorkspaceTimezone", () => ({
  default: () => "America/Los_Angeles"
}));

beforeEach(() => {
  // Pinned: the clock reads `new Date()`, so a real one would make this flaky.
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date("2026-10-29T22:42:00Z"));
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("the workspace clock", () => {
  it("shows the time and zone in the workspace's timezone", () => {
    render(<WorkspaceClock />);
    expect(screen.getByTestId("topbar-clock")).toHaveTextContent(/^3:42\sPM PDT$/);
  });

  it("adds the day and date in the menu bar's form, the zone left to the tooltip", () => {
    render(<WorkspaceClock withDate />);
    expect(screen.getByTestId("topbar-clock")).toHaveTextContent(/^Thu, Oct 29, 3:42\sPM$/);
  });
});
