// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SecretService } from "@/services/secretService";
import type { AppSecretNeeds } from "@/types/sharedSecrets";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "proj-1" }, branchName: "main" })
}));
vi.mock("@/services/secretService", () => ({ SecretService: { shareWithApps: vi.fn() } }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { ShareSecretDialog } from "./ShareSecretDialog";

const shareWithApps = vi.mocked(SecretService.shareWithApps);

const app = (id: string, name: string, missing: string[] = []): AppSecretNeeds => ({
  app_id: id,
  app_name: name,
  app_slug: name.toLowerCase().replace(/ /g, "-"),
  missing: missing.map((key) => ({ key, required: false }))
});

const apps = [app("pulse", "Platform Pulse", ["SLACK_BOT_TOKEN"]), app("ops", "Ops Bot")];

const open = (appIds: string[], ownValue = new Set<string>()) => {
  const onOpenChange = vi.fn();
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { mutations: { retry: false } } })}
    >
      <ShareSecretDialog
        secret={{ id: "s-slack", name: "SLACK_BOT_TOKEN", appIds }}
        apps={apps}
        ownValue={ownValue}
        onOpenChange={onOpenChange}
      />
    </QueryClientProvider>
  );
  return onOpenChange;
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("ShareSecretDialog", () => {
  it("starts from the apps it is shared with, and saves nothing unchanged", () => {
    open(["ops"]);
    expect(screen.getByLabelText("Ops Bot")).toBeChecked();
    expect(screen.getByLabelText("Platform Pulse")).not.toBeChecked();
    expect(screen.getByTestId("share-secret-save")).toBeDisabled();
  });

  it("says which app needs the key and which keeps its own value", () => {
    open([], new Set(["ops"]));
    expect(screen.getByText("Needs it")).toBeInTheDocument();
    expect(screen.getByText("Has its own value")).toBeInTheDocument();
  });

  it("sends the whole set, so an unticked app stops reading it", async () => {
    shareWithApps.mockResolvedValue({ secret_id: "s-slack", app_ids: ["pulse"] });
    const onOpenChange = open(["ops"]);

    fireEvent.click(screen.getByLabelText("Ops Bot"));
    fireEvent.click(screen.getByLabelText("Platform Pulse"));
    fireEvent.click(screen.getByTestId("share-secret-save"));

    await waitFor(() => expect(shareWithApps).toHaveBeenCalledWith("proj-1", "s-slack", ["pulse"]));
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });
});
