// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import useSettingsDialog from "@/stores/useSettingsDialog";
import type { ApiKey, ApiKeyListResponse } from "@/types/apiKey";
import LegacyApiKeys from ".";

vi.setConfig({ testTimeout: 20000 });

type QueryState = {
  data?: ApiKeyListResponse;
  isLoading: boolean;
  error: Error | null;
  refetch: () => void;
};
let query: QueryState = { isLoading: true, error: null, refetch: vi.fn() };
let isLocalMode = false;
// The viewer's workspace role. The section is open either way; only Revoke reads it.
let workspaceAdmin = true;
const revoke = vi.fn();

// The hooks are the seam: this file is about what the section shows and offers.
vi.mock("@/hooks/api/apiKeys/useApiKeys", () => ({ default: () => query }));
vi.mock("@/hooks/api/apiKeys/useApiKeyMutations", () => ({
  useRevokeApiKey: () => ({ mutate: revoke, isPending: false }),
  useExtendApiKey: () => ({ mutate: vi.fn(), isPending: false })
}));
vi.mock("@/hooks/api/apiKeys/tokenEndpoints", () => ({
  useWorkspaceApiKeyEndpoints: () => ({})
}));
vi.mock("@/hooks/api/apiKeys/useApiKeyActivity", () => ({
  API_KEY_ACTIVITY_LIMIT: 100,
  default: () => ({ isLoading: true, error: null, refetch: vi.fn() })
}));
// The role hook is mocked and `CanWorkspaceAdmin` is the real one, so the gate under test is the
// component the app uses, not a stand-in for it.
vi.mock("@/hooks/useRole", () => ({ useRole: () => ({ is: { workspaceAdmin } }) }));
vi.mock("@/contexts/AuthContext", () => ({ useAuth: () => ({ isLocalMode }) }));
// jsdom has no canvas; the Activity drawer's chart is not under test here.
vi.mock("@/components/Echarts/EChart", () => ({ default: () => <div data-testid='echart' /> }));

afterEach(() => {
  cleanup();
  revoke.mockReset();
  isLocalMode = false;
  workspaceAdmin = true;
  useSettingsDialog.setState({ isOpen: false, section: "organization.general" });
});

const key = (over: Partial<ApiKey> = {}): ApiKey => ({
  id: "k1",
  name: "CI deploy",
  created_at: "2026-01-01T00:00:00Z",
  expires_at: new Date(Date.now() + 30 * 24 * 60 * 60 * 1000).toISOString(),
  is_active: true,
  masked_key: "oxy_9f2c…a1b2",
  ...over
});

const show = (state: Partial<QueryState>) => {
  query = { isLoading: false, error: null, refetch: vi.fn(), ...state };
  render(<LegacyApiKeys />);
  return userEvent.setup({ delay: null });
};

const list = (...keys: ApiKey[]): Partial<QueryState> => ({
  data: { api_keys: keys, total: keys.length }
});

describe("Workspace → Legacy API keys", () => {
  it("says what a legacy API key reaches, and sends new credentials to API tokens", async () => {
    const user = show(list(key()));
    const notice = screen.getByTestId("legacy-api-keys-notice");
    expect(notice).toHaveTextContent(
      "A legacy API key reaches everything its owner can, and can't be limited to workspaces."
    );
    expect(notice).toHaveTextContent("For anything new, use an API token.");

    await user.click(within(notice).getByRole("button", { name: "Create an API token" }));
    expect(useSettingsDialog.getState()).toMatchObject({ isOpen: true, section: "account.tokens" });
  });

  it("badges every row Legacy", () => {
    show(list(key(), key({ id: "k2", name: "old laptop", expires_at: undefined })));
    const rows = screen.getAllByTestId("legacy-api-key-row");
    expect(rows).toHaveLength(2);
    for (const row of rows) {
      expect(within(row).getByTestId("legacy-badge")).toHaveTextContent("Legacy");
    }
  });

  it("offers Activity, Extend and Revoke on a live key", () => {
    show(list(key()));
    const row = screen.getByTestId("legacy-api-key-row");
    expect(within(row).getByTestId("legacy-api-key-activity-button")).toBeInTheDocument();
    expect(within(row).getByTestId("api-key-extend-button")).toBeInTheDocument();
    expect(within(row).getByTestId("legacy-api-key-revoke-button")).toBeEnabled();
  });

  it("has no way to create a legacy API key", () => {
    show(list(key()));
    const section = screen.getByTestId("settings-legacy-api-keys");
    expect(screen.queryByTestId("api-key-create-button")).not.toBeInTheDocument();
    // The one "create" on the page makes an API token, not a key.
    const creates = within(section).getAllByRole("button", { name: /create/i });
    expect(creates.map((button) => button.textContent)).toEqual(["Create an API token"]);
  });

  it("asks before revoking, then revokes through the legacy route", async () => {
    const user = show(list(key()));
    await user.click(screen.getByTestId("legacy-api-key-revoke-button"));
    expect(revoke).not.toHaveBeenCalled();
    expect(await screen.findByRole("alertdialog")).toHaveTextContent(
      "Anything using this legacy API key stops working at once"
    );
    await user.click(screen.getByTestId("legacy-api-key-revoke-confirm"));
    expect(revoke).toHaveBeenCalledWith("k1");
  });

  it("keeps Activity on a revoked key, and nothing that would change it", () => {
    show(list(key({ is_active: false })));
    const row = screen.getByTestId("legacy-api-key-row");
    expect(row).toHaveTextContent("Revoked");
    expect(within(row).getByTestId("legacy-api-key-activity-button")).toBeEnabled();
    expect(within(row).queryByTestId("api-key-extend-button")).not.toBeInTheDocument();
    expect(within(row).getByTestId("legacy-api-key-revoke-button")).toBeDisabled();
  });

  it("says so calmly when there are none", () => {
    show(list());
    expect(screen.getByText("No legacy API keys")).toBeInTheDocument();
    // The notice still points the way to API tokens.
    expect(screen.getByTestId("legacy-api-keys-create-token-link")).toBeInTheDocument();
  });

  it("shows a spinner, not an empty table, while loading", () => {
    show({ isLoading: true });
    expect(screen.queryByText("No legacy API keys")).not.toBeInTheDocument();
    expect(screen.queryByTestId("legacy-api-key-row")).not.toBeInTheDocument();
  });

  it("offers a retry when the list fails to load", async () => {
    const refetch = vi.fn();
    const user = show({ error: new Error("Network Error"), refetch });
    expect(screen.getByText("Couldn't load the legacy API keys.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /try again/i }));
    expect(refetch).toHaveBeenCalled();
  });

  it("offers no account link in local mode, which has no accounts", () => {
    isLocalMode = true;
    show(list(key()));
    expect(screen.getByTestId("legacy-api-keys-notice")).toBeInTheDocument();
    expect(screen.queryByTestId("legacy-api-keys-create-token-link")).not.toBeInTheDocument();
  });

  // A legacy API key belongs to its owner. The server lists it, extends it and shows its activity
  // to whoever owns it; only revoke needs the workspace admin role.
  describe("for a key's owner who is not a workspace admin", () => {
    const REASON = "Revoking a legacy API key needs a workspace admin role";

    it("opens the section in full: no denial, the notice and their keys", () => {
      workspaceAdmin = false;
      show(list(key(), key({ id: "k2", name: "old laptop" })));
      const section = screen.getByTestId("settings-legacy-api-keys");
      expect(section).not.toHaveTextContent(/You need workspace admin access/);
      expect(screen.getByTestId("legacy-api-keys-notice")).toBeInTheDocument();
      expect(screen.getByTestId("legacy-api-keys-create-token-link")).toBeEnabled();
      const rows = screen.getAllByTestId("legacy-api-key-row");
      expect(rows).toHaveLength(2);
      for (const row of rows) expect(within(row).getByTestId("legacy-badge")).toBeInTheDocument();
    });

    it("offers Extend on a live key and on an expired one", async () => {
      workspaceAdmin = false;
      const user = show(
        list(key(), key({ id: "k2", name: "lapsed", expires_at: "2020-01-01T00:00:00Z" }))
      );
      const [live, lapsed] = screen.getAllByTestId("legacy-api-key-row");
      const extend = within(live).getByTestId("api-key-extend-button");
      expect(extend).toBeEnabled();
      expect(within(lapsed).getByTestId("api-key-expired-extend-button")).toBeEnabled();

      await user.click(extend);
      expect(await screen.findByTestId("api-key-extend-popover")).toHaveTextContent(
        "Extend CI deploy"
      );
    });

    it("opens Activity", async () => {
      workspaceAdmin = false;
      const user = show(list(key()));
      const activity = screen.getByTestId("legacy-api-key-activity-button");
      expect(activity).toBeEnabled();
      await user.click(activity);
      expect(await screen.findByTestId("api-key-activity-drawer")).toHaveTextContent("CI deploy");
    });

    it("shows Revoke disabled, and says it needs a workspace admin role", async () => {
      workspaceAdmin = false;
      const user = show(list(key()));
      const button = screen.getByTestId("legacy-api-key-revoke-button");
      expect(button).toBeDisabled();
      expect(button).toHaveAccessibleName(`Revoke CI deploy: ${REASON}`);

      // Nothing to click through to: no confirm, no request.
      await user.click(button);
      expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
      expect(revoke).not.toHaveBeenCalled();

      // A disabled button takes no focus or hover, so the tooltip's trigger is a focusable
      // wrapper: the reason is reachable by keyboard as well as by mouse.
      expect(button.parentElement).toHaveAttribute("tabindex", "0");
    });

    it("does not blame the role for a key that is already revoked", () => {
      workspaceAdmin = false;
      show(list(key({ is_active: false })));
      const button = screen.getByTestId("legacy-api-key-revoke-button");
      expect(button).toBeDisabled();
      expect(button).toHaveAccessibleName("Revoke CI deploy");
    });

    it("does not promise a revoke in the section's description", () => {
      workspaceAdmin = false;
      show(list(key()));
      const section = screen.getByTestId("settings-legacy-api-keys");
      expect(section).toHaveTextContent("Extend one, or see what it has done.");
      expect(section).not.toHaveTextContent("or revoke it");
    });
  });

  it("gives a workspace admin Revoke with no admin caveat", () => {
    show(list(key()));
    const button = screen.getByTestId("legacy-api-key-revoke-button");
    expect(button).toBeEnabled();
    expect(button).toHaveAccessibleName("Revoke CI deploy");
    expect(screen.getByTestId("settings-legacy-api-keys")).toHaveTextContent("or revoke it");
  });
});
