// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type React from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import useSettingsDialog from "@/stores/useSettingsDialog";
import ApiKeys from ".";

vi.setConfig({ testTimeout: 20000 });

let isLocalMode = false;

// The hooks are the seam: this file is about what the section holds, and what it no longer does.
vi.mock("@/hooks/api/userTokens/useWorkspaceTokens", () => ({
  default: () => ({ data: { tokens: [] }, isLoading: false, error: null, refetch: vi.fn() })
}));
vi.mock("@/components/auth/Can", () => ({
  CanWorkspaceAdmin: ({ children }: { children: React.ReactNode }) => <>{children}</>
}));
vi.mock("@/contexts/AuthContext", () => ({ useAuth: () => ({ isLocalMode }) }));
vi.mock("@/stores/useCurrentWorkspace", () => ({
  default: () => ({ workspace: { id: "ws-1234" } })
}));

afterEach(() => {
  cleanup();
  isLocalMode = false;
  useSettingsDialog.setState({ isOpen: false, section: "organization.general" });
});

describe("Workspace → API tokens", () => {
  it("holds the token inventory, the way to your own tokens, and the workspace id", async () => {
    const user = userEvent.setup({ delay: null });
    render(<ApiKeys />);
    const section = screen.getByTestId("settings-api-keys");
    expect(within(section).getByTestId("workspace-tokens-table")).toBeInTheDocument();
    expect(section).toHaveTextContent("ws-1234");

    await user.click(within(section).getByTestId("workspace-tokens-manage-link"));
    expect(useSettingsDialog.getState()).toMatchObject({ isOpen: true, section: "account.tokens" });
  });

  it("lists no legacy API keys and creates nothing", () => {
    render(<ApiKeys />);
    const section = screen.getByTestId("settings-api-keys");
    expect(section).not.toHaveTextContent(/legacy/i);
    expect(screen.queryByTestId("workspace-legacy-keys")).not.toBeInTheDocument();
    expect(screen.queryByTestId("legacy-api-key-row")).not.toBeInTheDocument();
    expect(within(section).queryByRole("button", { name: /create/i })).not.toBeInTheDocument();
  });

  it("offers no account link in local mode, which has no accounts", () => {
    isLocalMode = true;
    render(<ApiKeys />);
    expect(screen.queryByTestId("workspace-tokens-manage-link")).not.toBeInTheDocument();
  });
});
