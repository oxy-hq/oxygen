// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { toast } from "sonner";
import { afterEach, describe, expect, it, vi } from "vitest";
import queryKeys from "@/hooks/api/queryKey";
import { openSecureWindow } from "@/utils/githubAppInstall";
import AddViaInstallationDialog from "./AddViaInstallationDialog";

const account = vi.hoisted(() => ({ connected: false }));

vi.mock("@/hooks/api/github/useGitHubAccount", () => ({
  useGitHubAccount: () => ({ data: { connected: account.connected } })
}));
vi.mock("@/hooks/api/github/useUserInstallations", () => ({
  useUserInstallations: () => ({ data: [], isLoading: false, isError: false })
}));
vi.mock("@/hooks/api/github/useCreateInstallationNamespace", () => ({
  useCreateInstallationNamespace: () => ({ mutateAsync: vi.fn(), isPending: false })
}));
vi.mock("@/hooks/api/github/useConnectGitHubAccount", () => ({
  useConnectGitHubAccount: () => ({ mutate: vi.fn(), isPending: false })
}));
vi.mock("@/services/api", () => ({
  GitHubApiService: {
    getNewInstallationUrl: vi.fn(async () => "https://github.com/apps/oxy/installations/new")
  }
}));
vi.mock("@/utils/githubAppInstall", () => ({
  openSecureWindow: vi.fn(() => ({ closed: false }))
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn() } }));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  account.connected = false;
});

function renderDialog() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidate = vi.spyOn(qc, "invalidateQueries");
  render(
    <QueryClientProvider client={qc}>
      <AddViaInstallationDialog
        orgId='org-1'
        open={true}
        onClose={() => {}}
        onConnected={() => {}}
      />
    </QueryClientProvider>
  );
  return { invalidate };
}

/** Start an install, then have the popup report `message` back. */
async function installAndPopupSays(message: Record<string, unknown>) {
  fireEvent.click(screen.getByRole("button", { name: /install github app/i }));
  await waitFor(() => expect(openSecureWindow).toHaveBeenCalled());
  window.dispatchEvent(
    new MessageEvent("message", { origin: window.location.origin, data: message })
  );
}

describe("AddViaInstallationDialog", () => {
  it("shows Connect GitHub step when account not connected", () => {
    renderDialog();
    expect(screen.getByRole("button", { name: /connect github/i })).toBeInTheDocument();
  });

  it("asks for a reconnect when the install callback says the GitHub account is unusable", async () => {
    account.connected = true;
    const { invalidate } = renderDialog();

    await installAndPopupSays({
      type: "github-callback-error",
      code: "github_account_not_connected",
      reason: "Connect your GitHub account to choose one of its installations."
    });

    await waitFor(() =>
      expect(invalidate).toHaveBeenCalledWith({ queryKey: queryKeys.github.account })
    );
    expect(toast.error).toHaveBeenCalledWith(expect.stringMatching(/reconnect your github/i));
  });

  it("shows the install callback's refusal in its own words", async () => {
    account.connected = true;
    renderDialog();
    const reason = "That GitHub installation isn't one your GitHub account can access.";

    await installAndPopupSays({ type: "github-callback-error", reason });

    await waitFor(() => expect(toast.error).toHaveBeenCalledWith(reason));
  });
});
