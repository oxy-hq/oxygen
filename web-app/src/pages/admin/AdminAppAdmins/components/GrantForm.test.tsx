// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DelegationBound } from "@/hooks/api/access/useDelegationBound";

vi.mock("@/hooks/api/access/useAppAdmins", () => ({
  ROLE_LABELS: { app_operator: "App Operator", global_admin: "Global Admin" },
  useAppAdmins: () => ({ data: [] }),
  useCreateAppAdmin: () => ({ mutate: vi.fn(), isPending: false })
}));
vi.mock("@/hooks/api/adminTenants", () => ({
  useDrainedAdminOrgs: () => ({
    orgs: [],
    isLoading: false,
    isDraining: false,
    isIncomplete: false,
    error: null,
    refetch: vi.fn()
  })
}));

import { GrantForm } from "./GrantForm";

const bound = (issuableRoles: DelegationBound["issuableRoles"]): DelegationBound => ({
  canGrant: true,
  issuableRoles,
  scopeAll: true,
  scopeOrgIds: []
});

afterEach(cleanup);

describe("GrantForm", () => {
  // What a role grants is the consequence of pressing Grant, so it is on the form, not
  // behind a hover: this is the most privileged form in the admin console.
  it("says on the form what the chosen role grants", () => {
    render(<GrantForm onCancel={vi.fn()} bound={bound(["app_operator", "global_admin"])} />);
    const blurb = screen.getByTestId("admin-app-admins-role-blurb");
    expect(blurb).toBeVisible();
    expect(blurb.textContent).toMatch(/Ships and develops custom apps/);
  });

  it("explains a missing Global Admin option behind the ⓘ, only when it is missing", () => {
    render(<GrantForm onCancel={vi.fn()} bound={bound(["app_operator"])} />);
    expect(screen.getByTestId("admin-app-admins-role-note")).toBeInTheDocument();
    cleanup();
    render(<GrantForm onCancel={vi.fn()} bound={bound(["app_operator", "global_admin"])} />);
    expect(screen.queryByTestId("admin-app-admins-role-note")).toBeNull();
  });
});
