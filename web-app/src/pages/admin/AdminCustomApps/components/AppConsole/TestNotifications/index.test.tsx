// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { AxiosError, AxiosHeaders } from "axios";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CustomAppsService } from "@/services/api/customApps";
import type { AppNotificationKinds, CustomApp } from "@/types/apps";

vi.mock("@/services/api/customApps", () => ({
  CustomAppsService: { listNotificationKinds: vi.fn(), sendTestNotification: vi.fn() }
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { toast } from "sonner";
import { TestNotifications } from "./index";
import { testNotificationError } from "./testNotificationError";

const app = { id: "app-1", name: "Store Ops", org_slug: "acme", slug: "store-ops" } as CustomApp;

const kinds = (reachable: boolean): AppNotificationKinds => ({
  reachable,
  kinds: [
    {
      kind: "check_failed",
      label: "Check failed",
      description: null,
      level: "time_sensitive",
      category: ["General"],
      mutable: true
    },
    {
      kind: "fire_alarm",
      label: "Fire alarm",
      description: null,
      level: "critical",
      category: ["General"],
      mutable: false
    }
  ]
});

const renderPanel = (data: AppNotificationKinds) => {
  vi.mocked(CustomAppsService.listNotificationKinds).mockResolvedValue(data);
  render(
    <QueryClientProvider client={new QueryClient()}>
      <TestNotifications app={app} />
    </QueryClientProvider>
  );
};

const axiosError = (status: number, data: unknown) => {
  const config = { headers: new AxiosHeaders() };
  return new AxiosError("Request failed", String(status), config, null, {
    status,
    statusText: "",
    headers: {},
    config,
    data
  });
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("Test notifications panel", () => {
  it("lists each kind and sends the one clicked to the app", async () => {
    vi.mocked(CustomAppsService.sendTestNotification).mockResolvedValue({
      id: "send-1",
      kind: "check_failed"
    });
    renderPanel(kinds(true));
    const row = await screen.findByTestId("admin-app-notification-test-kind-check_failed");
    expect(within(row).getByText("Check failed")).toBeInTheDocument();
    expect(row).toHaveTextContent("time-sensitive");
    fireEvent.click(within(row).getByRole("button", { name: "Send to me" }));
    await waitFor(() =>
      expect(CustomAppsService.sendTestNotification).toHaveBeenCalledWith("app-1", "check_failed")
    );
    await waitFor(() => expect(toast.success).toHaveBeenCalled());
  });

  it("says a critical kind ignores mutes, and no other", async () => {
    renderPanel(kinds(true));
    const critical = await screen.findByTestId("admin-app-notification-test-kind-fire_alarm");
    const muteable = screen.getByTestId("admin-app-notification-test-kind-check_failed");
    expect(critical).toHaveTextContent("ignores mutes");
    expect(muteable).not.toHaveTextContent("ignores mutes");
  });

  it("turns the buttons off and says why when the app can't notify you", async () => {
    renderPanel(kinds(false));
    expect(
      await screen.findByTestId("admin-app-notification-test-unreachable")
    ).toBeInTheDocument();
    for (const button of screen.getAllByRole("button", { name: "Send to me" })) {
      expect(button).toBeDisabled();
    }
  });

  it("names where to declare kinds when the app has none", async () => {
    renderPanel({ reachable: true, kinds: [] });
    expect(await screen.findByText("No notification kinds declared.")).toBeInTheDocument();
    expect(screen.getByText(/notifications\.kinds/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Send to me" })).not.toBeInTheDocument();
  });

  it("shows the server's reason when a send is refused", async () => {
    vi.mocked(CustomAppsService.sendTestNotification).mockRejectedValue(
      axiosError(429, { error: "rate_limited", retry_after: 7 })
    );
    renderPanel(kinds(true));
    const row = await screen.findByTestId("admin-app-notification-test-kind-check_failed");
    fireEvent.click(within(row).getByRole("button", { name: "Send to me" }));
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("You just sent one. Try again in 7 seconds.")
    );
  });
});

describe("testNotificationError", () => {
  it("turns each refusal code into what to do about it", () => {
    expect(testNotificationError(axiosError(409, { error: "not_in_audience" }))).toMatch(
      /aren't a member of its org/
    );
    expect(testNotificationError(axiosError(400, { error: "unknown_kind" }))).toMatch(/Reload/);
    expect(testNotificationError(axiosError(409, { error: "no_store" }))).toMatch(
      /no notification store/
    );
    expect(testNotificationError(axiosError(404, ""))).toMatch(/not found/);
    expect(testNotificationError(new Error("Network Error"))).toBe("Network Error");
  });
});
