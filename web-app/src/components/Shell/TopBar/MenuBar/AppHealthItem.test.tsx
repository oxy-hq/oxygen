// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { HqSignal } from "@/pages/launcher/components/useHqAlerts";
import { AppHealthItem } from "./AppHealthItem";

let signals: HqSignal[] = [];
vi.mock("@/pages/launcher/components/useHqAlerts", () => ({ useHqAlerts: () => signals }));

const signal = (id: string, severity: HqSignal["severity"]): HqSignal => ({
  id,
  severity,
  category: severity === "critical" ? "App down" : "App degraded",
  title: `${id} is not well`,
  destLabel: id,
  href: `/customer-apps/acme/${id}/`
});

const renderItem = () =>
  render(
    <MemoryRouter>
      <AppHealthItem />
    </MemoryRouter>
  );

afterEach(() => {
  cleanup();
  signals = [];
});

describe("the menu bar's app-health item", () => {
  it("is not there while no app is down or degraded — no green tick for an absence", () => {
    renderItem();
    expect(screen.queryByTestId("menubar-health")).toBeNull();
  });

  it("leaves an unmeasured app to the home's attention list", () => {
    signals = [signal("quiet", "info")];
    renderItem();
    expect(screen.queryByTestId("menubar-health")).toBeNull();
  });

  it("counts the apps that need attention and lists them, each linking to its app", () => {
    signals = [signal("pos", "critical"), signal("labor", "warning"), signal("quiet", "info")];
    renderItem();
    const item = screen.getByTestId("menubar-health");
    expect(item).toHaveTextContent("2");
    expect(item).toHaveAccessibleName("2 apps need attention");
    fireEvent.click(item);
    expect(screen.getByTestId("menubar-health-row-pos")).toHaveAttribute(
      "href",
      "/customer-apps/acme/pos/"
    );
    expect(screen.getByTestId("menubar-health-row-labor")).toBeInTheDocument();
    expect(screen.queryByTestId("menubar-health-row-quiet")).toBeNull();
  });
});
