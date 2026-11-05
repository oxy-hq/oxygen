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
vi.mock("./TodayButton", () => ({
  TodayButton: () => <span data-testid='topbar-today-button' />
}));
let online = true;
vi.mock("@/hooks/useOnlineStatus", () => ({ default: () => online }));

afterEach(() => {
  cleanup();
  appsFirst = false;
  online = true;
});

const order = () =>
  Array.from(screen.getByTestId("menubar-status").children).map((el) =>
    el.getAttribute("data-testid")
  );

describe("the top bar", () => {
  it("is main's with the flag off: clock without a date, bell, Ask", () => {
    render(<TopBar />);
    expect(screen.getByTestId("topbar-clock")).toHaveAttribute("data-with-date", "false");
    expect(screen.queryByTestId("menubar-status")).toBeNull();
    expect(screen.getByTestId("workspace-topbar")).not.toHaveAttribute("data-menubar");
  });

  it("is minimal with the flag on: Ask, Today and the bell, no clock, no connection while online", () => {
    appsFirst = true;
    render(<TopBar />);
    expect(screen.getByTestId("workspace-topbar")).toHaveAttribute("data-menubar", "true");
    expect(screen.queryByTestId("topbar-system")).toBeNull();
    expect(screen.queryByTestId("topbar-clock")).toBeNull();
    expect(order()).toEqual([
      "menubar-health",
      "ask-oxygen-button",
      "topbar-today-button",
      "notifications-bell"
    ]);
  });

  it("says nothing about the connection while online, on either bar", () => {
    render(<TopBar />);
    expect(screen.queryByTestId("topbar-system")).toBeNull();
    cleanup();
    appsFirst = true;
    render(<TopBar />);
    expect(screen.queryByTestId("menubar-connection")).toBeNull();
  });

  it("says Offline when the browser is offline, on either bar", () => {
    online = false;
    render(<TopBar />);
    expect(screen.getByTestId("topbar-system")).toHaveTextContent("Offline");
    cleanup();
    appsFirst = true;
    render(<TopBar />);
    expect(screen.getByTestId("menubar-connection")).toHaveAccessibleName("Offline");
    expect(order()).toEqual([
      "menubar-health",
      "menubar-connection",
      "ask-oxygen-button",
      "topbar-today-button",
      "notifications-bell"
    ]);
  });
});
