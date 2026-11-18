// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CustomApp } from "@/types/apps";

vi.mock("@/hooks/api/customApps/useAppIssues", () => ({ useAppIssues: vi.fn() }));
vi.mock("@/hooks/api/customApps/useAppEvents", () => ({ useAppEventTypes: vi.fn() }));

import { useAppEventTypes } from "@/hooks/api/customApps/useAppEvents";
import { useAppIssues } from "@/hooks/api/customApps/useAppIssues";
import { ConsoleNav } from "./ConsoleNav";
import { CONSOLE_PAGES } from "./consolePages";

const app = { id: "app-1", name: "Shop", org_slug: "local", slug: "shop" } as CustomApp;

const issue = (onLive: boolean) => ({ on_live_build: onLive });

const renderNav = ({
  url = "/admin/apps/local/shop",
  liveIssues = 0,
  dead = 0
}: {
  url?: string;
  liveIssues?: number;
  dead?: number;
} = {}) => {
  vi.mocked(useAppIssues).mockReturnValue({
    data: {
      issues: [...Array(liveIssues)].map(() => issue(true)),
      window_days: 7,
      truncated: false
    }
  } as unknown as ReturnType<typeof useAppIssues>);
  vi.mocked(useAppEventTypes).mockReturnValue({
    data: {
      types: [
        {
          type: "shop.order.placed",
          publisher: "shop",
          event: "order.placed",
          schemaRevision: 1,
          lastPublishedAt: null,
          listeners: [
            {
              listener: "shipping/on-order",
              app: "shipping",
              function: "on-order",
              environment: "production",
              pending: 0,
              retrying: 0,
              dead,
              held: 0,
              oldestPendingAgeSeconds: null,
              lastSuccessAt: null,
              lastFailureAt: null,
              lastFailureOutcome: null,
              inFlight: 0,
              buildId: null,
              reconciledAt: null
            }
          ]
        }
      ]
    }
  } as unknown as ReturnType<typeof useAppEventTypes>);
  render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter initialEntries={[url]}>
        <ConsoleNav app={app} page='events' environment='production' />
      </MemoryRouter>
    </QueryClientProvider>
  );
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("ConsoleNav", () => {
  it("links every page, and marks the current one", () => {
    renderNav();
    for (const page of CONSOLE_PAGES) {
      expect(screen.getByTestId(`apps-console-nav-${page.id}`)).toBeInTheDocument();
    }
    expect(screen.getByTestId("apps-console-nav-events").getAttribute("aria-current")).toBe("page");
    expect(screen.getByTestId("apps-console-nav-overview").getAttribute("aria-current")).toBeNull();
  });

  it("keeps the environment switch in every link and drops a function selection", () => {
    renderNav({ url: "/admin/apps/local/shop/functions?env=staging&fn=checkout" });
    expect(screen.getByTestId("apps-console-nav-data").getAttribute("href")).toBe(
      "/admin/apps/local/shop/data?env=staging"
    );
  });

  it("shows a count only when something is wrong", () => {
    renderNav();
    expect(screen.queryByTestId("apps-console-nav-activity-count")).toBeNull();
    expect(screen.queryByTestId("apps-console-nav-events-count")).toBeNull();
    cleanup();
    renderNav({ liveIssues: 2, dead: 4 });
    expect(screen.getByTestId("apps-console-nav-activity-count").textContent).toBe("2");
    expect(screen.getByTestId("apps-console-nav-events-count").textContent).toBe("4");
  });
});
