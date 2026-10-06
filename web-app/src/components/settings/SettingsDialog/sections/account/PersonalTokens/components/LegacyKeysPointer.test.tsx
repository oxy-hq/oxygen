// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import useSettingsDialog from "@/stores/useSettingsDialog";
import LegacyKeysPointer from "./LegacyKeysPointer";

vi.setConfig({ testTimeout: 20000 });

type WorkspaceState = { workspace: { id: string; current_user_role?: string } | null };
let state: WorkspaceState = { workspace: { id: "ws-1", current_user_role: "viewer" } };

// The store is the seam: the line depends on a workspace being loaded, and on nothing else.
vi.mock("@/stores/useCurrentWorkspace", () => ({
  default: (selector: (s: WorkspaceState) => unknown) => selector(state)
}));

afterEach(() => {
  cleanup();
  state = { workspace: { id: "ws-1", current_user_role: "viewer" } };
  useSettingsDialog.setState({ isOpen: false, section: "organization.general" });
});

describe("LegacyKeysPointer", () => {
  it("says where older keys are, and opens that section", async () => {
    const user = userEvent.setup({ delay: null });
    render(<LegacyKeysPointer />);
    expect(screen.getByTestId("account-token-legacy-pointer")).toHaveTextContent(
      "Older keys are under Workspace → Legacy API keys."
    );
    await user.click(screen.getByTestId("account-token-legacy-link"));
    expect(useSettingsDialog.getState()).toMatchObject({
      isOpen: true,
      section: "workspace.legacy_api_keys"
    });
  });

  // A legacy API key belongs to its owner, so the way to it is not an admin's alone.
  it.each(["viewer", "member", "admin", "owner", undefined])(
    "shows to anyone with a workspace, whatever their role there (%s)",
    (role) => {
      state = { workspace: { id: "ws-1", current_user_role: role } };
      render(<LegacyKeysPointer />);
      expect(screen.getByTestId("account-token-legacy-link")).toBeInTheDocument();
    }
  );

  it("stays out of the way with no workspace loaded: the section isn't in the nav then", () => {
    state = { workspace: null };
    render(<LegacyKeysPointer />);
    expect(screen.queryByTestId("account-token-legacy-pointer")).not.toBeInTheDocument();
  });
});
