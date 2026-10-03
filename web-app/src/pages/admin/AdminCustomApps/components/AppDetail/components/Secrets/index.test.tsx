// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CustomAppsService } from "@/services/api/customApps";
import type { AppSecrets } from "@/types/apps";

vi.mock("@/services/api/customApps", () => ({
  CustomAppsService: { listSecrets: vi.fn(), setSecret: vi.fn() }
}));

import { Secrets } from "./index";

const secrets: AppSecrets = {
  app_id: "app-1",
  app_slug: "storefront",
  app_name: "Storefront",
  missing_required: 1,
  entries: [
    { key: "STRIPE_KEY", is_set: false, declared: true, required: true, source: "manifest" },
    {
      key: "WEBHOOK_SECRET",
      is_set: true,
      declared: true,
      required: true,
      source: "webhook",
      secret_id: "s-1",
      updated_at: "2026-10-01T00:00:00Z"
    }
  ]
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

/** Renders the panel, then opens the dialog from `key`'s row button. */
const openFromRow = async (key: string, button: "Set" | "Rotate") => {
  vi.mocked(CustomAppsService.listSecrets).mockResolvedValue(secrets);
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Secrets appId='app-1' />
    </QueryClientProvider>
  );
  const row = await screen.findByTestId(`admin-app-secret-${key}`);
  fireEvent.click(within(row).getByRole("button", { name: button }));
  return screen.getByRole("dialog");
};

// The row knows whether its key has a value; the dialog used to assume any key
// opened from a row had one, and offered to "rotate" a secret that was missing.
describe("Secrets panel — the dialog says what the row is", () => {
  it("sets a missing key, rather than rotating it", async () => {
    const dialog = await openFromRow("STRIPE_KEY", "Set");
    expect(within(dialog).getByRole("heading", { name: "Set STRIPE_KEY" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Save" })).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Rotate" })).not.toBeInTheDocument();
    // Still the declared name: typing another would not satisfy the declaration.
    expect(within(dialog).getByLabelText("Key")).toBeDisabled();
  });

  it("rotates a key that has a value", async () => {
    const dialog = await openFromRow("WEBHOOK_SECRET", "Rotate");
    expect(
      within(dialog).getByRole("heading", { name: "Rotate WEBHOOK_SECRET" })
    ).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Rotate" })).toBeInTheDocument();
  });

  it("adds a new key from Add secret", async () => {
    vi.mocked(CustomAppsService.listSecrets).mockResolvedValue(secrets);
    render(
      <QueryClientProvider client={new QueryClient()}>
        <Secrets appId='app-1' />
      </QueryClientProvider>
    );
    fireEvent.click(screen.getByTestId("admin-app-secrets-add"));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Add secret" })).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Key")).toBeEnabled();
  });
});
