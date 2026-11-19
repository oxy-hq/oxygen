// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { OxygenAccess } from "@/types/oxygenAccess";
import { OxygenAccessApproval } from "./OxygenAccessApproval";

const approve = vi.fn();
const revoke = vi.fn();
let current: OxygenAccess;

// The hooks are the seam: this file is about what an app's request renders,
// and which request each button sends.
vi.mock("@/hooks/api/oxygenAccess", () => ({
  useOxygenAccess: () => ({
    data: current,
    isPending: false,
    error: null,
    refetch: vi.fn()
  }),
  useApproveOxygenAccess: () => ({ mutate: approve, isPending: false }),
  useRevokeOxygenAccess: () => ({ mutate: revoke, isPending: false })
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

const app = (over: Partial<OxygenAccess> = {}): OxygenAccess => ({
  app_id: "app-1",
  app_slug: "store-ops",
  app_name: "Store Ops",
  live_build: {
    build_id: "b1",
    published_at: "2026-11-18T10:00:00Z",
    published_by: "Michael",
    published_via: null
  },
  actions: [
    {
      action: "frontline_device_manage",
      description: "Manage the organization's kiosk devices.",
      functions: ["tablets"],
      status: "pending",
      approved_at: null,
      approved_by: null,
      targets: []
    }
  ],
  token_id: null,
  service_account: null,
  last_used_at: null,
  ...over
});

afterEach(() => {
  cleanup();
  approve.mockReset();
  revoke.mockReset();
});

describe("OxygenAccessApproval", () => {
  it("says what is being trusted, and approves what is waiting", async () => {
    current = app();
    render(<OxygenAccessApproval orgId='org-1' appId='app-1' />);
    expect(screen.getByText(/Oxygen checks the app, not the person using it/)).toBeTruthy();
    expect(screen.getByText(/published .* by Michael/)).toBeTruthy();
    expect(screen.getByText("Waiting for approval")).toBeTruthy();
    expect(screen.queryByTestId("oxygen-access-revoke")).toBeNull();

    await userEvent.click(screen.getByTestId("oxygen-access-approve"));
    expect(approve).toHaveBeenCalledWith(
      { orgId: "org-1", appId: "app-1", actions: ["frontline_device_manage"] },
      expect.anything()
    );
  });

  it("offers a revoke once approved, and no approve with nothing waiting", async () => {
    current = app({
      token_id: "tok-1",
      service_account: "app-store-ops",
      actions: [
        {
          action: "frontline_device_manage",
          description: "Manage the organization's kiosk devices.",
          functions: ["tablets"],
          status: "approved",
          approved_at: "2026-11-18T11:00:00Z",
          approved_by: "Scott",
          targets: []
        }
      ]
    });
    render(<OxygenAccessApproval orgId='org-1' appId='app-1' />);
    expect(screen.getByText("Approved")).toBeTruthy();
    expect(screen.getByText(/Acts as the service account app-store-ops/)).toBeTruthy();
    expect(screen.queryByTestId("oxygen-access-approve")).toBeNull();

    await userEvent.click(screen.getByTestId("oxygen-access-revoke"));
    expect(revoke).toHaveBeenCalledWith({ orgId: "org-1", appId: "app-1" }, expect.anything());
  });

  it("shows an upload's pipelines, each with its own status", () => {
    current = app({
      app_name: "Bookkeeping",
      actions: [
        {
          action: "source_upload_report",
          description: "Upload source reports into the landing zones of the pipelines named below.",
          functions: ["ubereats-report-webhook"],
          status: "pending",
          approved_at: null,
          approved_by: null,
          targets: [
            { name: "toast_pos", status: "approved", approved_at: null, approved_by: null },
            {
              name: "ubereats_transactions",
              status: "pending",
              approved_at: null,
              approved_by: null
            }
          ]
        }
      ]
    });
    render(<OxygenAccessApproval orgId='org-1' appId='app-1' />);
    const targets = screen.getByTestId("oxygen-access-targets");
    expect(targets.textContent).toContain("toast_pos");
    expect(targets.textContent).toContain("ubereats_transactions");
    expect(screen.getAllByText("Waiting for approval")).toHaveLength(2);
    expect(screen.getByTestId("oxygen-access-approve")).toBeTruthy();
  });
});
