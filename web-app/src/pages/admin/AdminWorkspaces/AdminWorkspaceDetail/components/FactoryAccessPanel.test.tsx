// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AdminFactoryAccess } from "@/services/api/adminTenants";
import FactoryAccessPanel from "./FactoryAccessPanel";

const mutate = vi.fn();
let access: AdminFactoryAccess;

vi.mock("@/hooks/api/adminTenants/useAdminWorkspaces", () => ({
  useAdminFactoryAccess: () => ({
    isPending: false,
    isError: false,
    data: access,
    refetch: () => undefined
  }),
  useSetAdminFactoryAccess: () => ({ mutate, isPending: false, variables: undefined })
}));

const member = (over: Partial<AdminFactoryAccess["members"][number]>) => ({
  user_id: "u-1",
  email: "ada@acme.test",
  name: "Ada",
  org_role: "member",
  granted: false,
  granted_by: null,
  granted_at: null,
  ...over
});

beforeEach(() => {
  mutate.mockReset();
});

afterEach(cleanup);

describe("FactoryAccessPanel", () => {
  it("grants a member with the toggle, and revokes a granted one", () => {
    access = {
      enforced: true,
      members: [
        member({ user_id: "u-granted", email: "grace@acme.test", name: "Grace", granted: true }),
        member({ user_id: "u-plain", email: "ada@acme.test" })
      ]
    };
    render(<FactoryAccessPanel workspaceId='ws-1' />);

    fireEvent.click(screen.getByTestId("factory-access-switch-u-plain"));
    expect(mutate).toHaveBeenLastCalledWith({ userId: "u-plain", granted: true });

    fireEvent.click(screen.getByTestId("factory-access-switch-u-granted"));
    expect(mutate).toHaveBeenLastCalledWith({ userId: "u-granted", granted: false });
  });

  it("says whether enforcement is on", () => {
    access = { enforced: false, members: [member({})] };
    render(<FactoryAccessPanel workspaceId='ws-1' />);
    expect(screen.getByText(/Enforcement is off/)).toBeInTheDocument();
    cleanup();

    access = { enforced: true, members: [member({})] };
    render(<FactoryAccessPanel workspaceId='ws-1' />);
    expect(screen.getByText(/Enforcement is on/)).toBeInTheDocument();
  });

  /// A grant whose holder left the org opens nothing, but must stay removable.
  it("lists a grant whose holder left the org, and lets it be removed but not re-added", () => {
    access = {
      enforced: true,
      members: [member({ user_id: "u-left", org_role: null, granted: true })]
    };
    render(<FactoryAccessPanel workspaceId='ws-1' />);
    expect(screen.getByText("Left the org")).toBeInTheDocument();
    const toggle = screen.getByTestId("factory-access-switch-u-left");
    expect(toggle).not.toBeDisabled();
    fireEvent.click(toggle);
    expect(mutate).toHaveBeenLastCalledWith({ userId: "u-left", granted: false });
  });

  /// Owners and admins of the org reach the Factory without a grant, and the panel
  /// says so on their rows and in the summary.
  it("marks owners and admins as having access by role", () => {
    access = {
      enforced: true,
      members: [
        member({ user_id: "u-owner", org_role: "owner" }),
        member({ user_id: "u-admin", org_role: "admin" }),
        member({ user_id: "u-plain" })
      ]
    };
    render(<FactoryAccessPanel workspaceId='ws-1' />);
    expect(screen.getByText(/Owners and admins already have access/)).toBeInTheDocument();
    expect(screen.getByTestId("factory-access-row-u-owner").textContent).toContain(
      "Has access by role"
    );
    expect(screen.getByTestId("factory-access-row-u-admin").textContent).toContain(
      "Has access by role"
    );
    expect(screen.getByTestId("factory-access-row-u-plain").textContent).not.toContain(
      "Has access by role"
    );
  });

  it("links to the workspace's Factory when the org is known, and not otherwise", () => {
    access = { enforced: true, members: [member({})] };
    render(
      <MemoryRouter>
        <FactoryAccessPanel workspaceId='ws-1' orgSlug='acme' />
      </MemoryRouter>
    );
    expect(screen.getByTestId("factory-access-open-factory")).toHaveAttribute(
      "href",
      "/acme/workspaces/ws-1/ide"
    );
    cleanup();

    render(<FactoryAccessPanel workspaceId='ws-1' orgSlug={null} />);
    expect(screen.queryByTestId("factory-access-open-factory")).not.toBeInTheDocument();
  });
});
