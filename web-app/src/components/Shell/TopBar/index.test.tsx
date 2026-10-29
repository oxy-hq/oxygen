// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TopBar } from "./index";

/** With the apps-first experience the top bar is the home's menu bar; without it, main's. */

let appsFirst = false;
vi.mock("@/hooks/useAppsFirstExperience", () => ({ default: () => appsFirst }));
vi.mock("./Breadcrumb", () => ({ Breadcrumb: () => null }));
vi.mock("./NotificationsBell", () => ({
  NotificationsBell: () => <span data-testid='notifications-bell' />
}));
vi.mock("./WorkspaceClock", () => ({
  WorkspaceClock: ({ withDate }: { withDate?: boolean }) => (
    <span data-testid='topbar-clock' data-with-date={String(!!withDate)} />
  )
}));
vi.mock("./MenuBar/AppHealthItem", () => ({
  AppHealthItem: () => <span data-testid='menubar-health' />
}));

afterEach(() => {
  cleanup();
  appsFirst = false;
});

const order = () =>
  Array.from(screen.getByTestId("menubar-status").children).map((el) =>
    el.getAttribute("data-testid")
  );

describe("the top bar", () => {
  it("is main's with the flag off: Sys text, clock without a date, bell, Ask", () => {
    render(<TopBar />);
    expect(screen.getByTestId("topbar-system")).toBeInTheDocument();
    expect(screen.getByTestId("topbar-clock")).toHaveAttribute("data-with-date", "false");
    expect(screen.queryByTestId("menubar-status")).toBeNull();
    expect(screen.getByTestId("workspace-topbar")).not.toHaveAttribute("data-menubar");
  });

  it("is a menu bar with the flag on, the day and time last", () => {
    appsFirst = true;
    render(<TopBar />);
    expect(screen.getByTestId("workspace-topbar")).toHaveAttribute("data-menubar", "true");
    expect(screen.queryByTestId("topbar-system")).toBeNull();
    expect(order()).toEqual([
      "menubar-health",
      "menubar-connection",
      "ask-oxygen-button",
      "notifications-bell",
      "topbar-clock"
    ]);
    expect(screen.getByTestId("topbar-clock")).toHaveAttribute("data-with-date", "true");
  });
});
