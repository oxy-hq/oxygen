// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AppEventDead, AppEventListenerHealth, AppEventTypes } from "@/services/api/appEvents";
import { AppEventsService } from "@/services/api/appEvents";
import type { CustomApp } from "@/types/apps";

vi.mock("@/services/api/appEvents", () => ({
  AppEventsService: { types: vi.fn(), dead: vi.fn(), feed: vi.fn() }
}));

import { EventsPage } from "./index";

const app = { id: "app-1", name: "Shop", org_slug: "local", slug: "shop" } as CustomApp;

const listener = (
  appSlug: string,
  fn: string,
  counts: Partial<AppEventListenerHealth> = {}
): AppEventListenerHealth => ({
  listener: `${appSlug}/${fn}`,
  app: appSlug,
  function: fn,
  environment: "production",
  pending: 0,
  retrying: 0,
  dead: 0,
  held: 0,
  oldestPendingAgeSeconds: null,
  lastSuccessAt: null,
  lastFailureAt: null,
  lastFailureOutcome: null,
  inFlight: 0,
  buildId: null,
  reconciledAt: null,
  ...counts
});

const types: AppEventTypes = {
  types: [
    {
      type: "shop.order.placed",
      publisher: "shop",
      event: "order.placed",
      schemaRevision: 2,
      lastPublishedAt: "2026-11-18T07:00:00Z",
      listeners: [
        listener("shipping", "on-order-placed", {
          dead: 3,
          pending: 1,
          oldestPendingAgeSeconds: 120
        })
      ]
    },
    {
      type: "shop.order.viewed",
      publisher: "shop",
      event: "order.viewed",
      schemaRevision: 1,
      lastPublishedAt: null,
      listeners: []
    },
    {
      type: "billing.paid",
      publisher: "billing",
      event: "paid",
      schemaRevision: 1,
      lastPublishedAt: null,
      listeners: [listener("crm", "on-paid", { dead: 50 })]
    }
  ]
};

const dead: AppEventDead = {
  environment: "production",
  truncated: false,
  groups: [
    {
      signature: "error: card declined",
      outcome: "error",
      error: "card declined",
      count: 3,
      deliveries: [
        {
          id: "d-1",
          eventId: "e-1",
          type: "shop.order.placed",
          key: "A-2001",
          listener: "shipping/on-order-placed",
          attemptCount: 5,
          lastError: "card declined\n  at charge()",
          diedAt: "2026-11-18T07:10:00Z",
          attempts: []
        }
      ]
    }
  ]
};

const renderPage = (data: { types?: AppEventTypes; dead?: AppEventDead } = {}) => {
  vi.mocked(AppEventsService.types).mockResolvedValue(data.types ?? types);
  vi.mocked(AppEventsService.dead).mockResolvedValue(data.dead ?? dead);
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <EventsPage app={app} environment='production' scope={{ label: "Production" }} />
    </QueryClientProvider>
  );
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("Events page", () => {
  it("leads with the queue's numbers for this app only", async () => {
    renderPage();
    await screen.findByTestId("admin-app-events-type-shop.order.placed");
    expect(screen.getByTestId("admin-app-events-kpis-types-value").textContent).toBe("2");
    expect(screen.getByTestId("admin-app-events-kpis-pending-value").textContent).toBe("1");
    // 3, not 53: billing.paid is another app's type with another app's listener.
    expect(screen.getByTestId("admin-app-events-kpis-dead-value").textContent).toBe("3");
    expect(screen.queryByTestId("admin-app-events-type-billing.paid")).toBeNull();
  });

  it("shows a type's listeners one click down", async () => {
    renderPage();
    const row = await screen.findByTestId("admin-app-events-type-shop.order.placed");
    expect(screen.queryByTestId("admin-app-events-listeners-shop.order.placed")).toBeNull();
    fireEvent.click(row);
    const listeners = screen.getByTestId("admin-app-events-listeners-shop.order.placed");
    expect(within(listeners).getByTitle("shipping/on-order-placed")).toBeInTheDocument();
    // A listener with dead letters carries the command that queues them again.
    expect(
      within(listeners).getByText(
        "oxyc events redeliver --org local --to shipping/on-order-placed --status dead --dry-run"
      )
    ).toBeInTheDocument();
  });

  it("narrows to types with dead letters", async () => {
    renderPage();
    await screen.findByTestId("admin-app-events-type-shop.order.viewed");
    fireEvent.click(screen.getByTestId("admin-app-events-filter-dead"));
    expect(screen.queryByTestId("admin-app-events-type-shop.order.viewed")).toBeNull();
    expect(screen.getByTestId("admin-app-events-type-shop.order.placed")).toBeInTheDocument();
  });

  it("groups dead letters by cause, the full error one click down", async () => {
    renderPage();
    const group = await screen.findByTestId("admin-app-events-dead-group");
    expect(within(group).getByText("3×")).toBeInTheDocument();
    expect(within(group).queryByText(/at charge\(\)/)).toBeNull();
    fireEvent.click(within(group).getByRole("button"));
    expect(within(group).getByText(/at charge\(\)/)).toBeInTheDocument();
  });

  it("keeps a failed dead-letters read on the page, rather than reading as none died", async () => {
    vi.mocked(AppEventsService.types).mockResolvedValue(types);
    vi.mocked(AppEventsService.dead).mockRejectedValue(new Error("boom"));
    render(
      <QueryClientProvider
        client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
      >
        <EventsPage app={app} environment='production' scope={{ label: "Production" }} />
      </QueryClientProvider>
    );
    const panel = await screen.findByTestId("apps-console-panel-events-dead");
    expect(await within(panel).findByTestId("admin-async-error")).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: /retry/i })).toBeInTheDocument();
  });

  it("says when the app has no events, rather than drawing an empty table", async () => {
    renderPage({
      types: { types: [types.types[2]] },
      dead: { environment: "production", groups: [], truncated: false }
    });
    expect(await screen.findByTestId("admin-app-events-empty")).toBeInTheDocument();
    expect(screen.queryByTestId("admin-app-events-dead")).toBeNull();
  });

  it("does not draw zeros for numbers it could not read", async () => {
    vi.mocked(AppEventsService.types).mockRejectedValue(new Error("boom"));
    vi.mocked(AppEventsService.dead).mockResolvedValue(dead);
    render(
      <QueryClientProvider
        client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
      >
        <EventsPage app={app} environment='production' scope={{ label: "Production" }} />
      </QueryClientProvider>
    );
    expect(await screen.findByTestId("admin-async-error")).toBeInTheDocument();
    expect(screen.getByTestId("admin-app-events-kpis-dead-value").textContent).toBe("—");
  });
});
