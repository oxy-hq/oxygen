// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { OltpConnectionInfo, OltpConnectionTest } from "@/services/api/oltp";
import { OltpTenantPanel } from ".";

afterEach(cleanup);

const STATUS: OltpConnectionInfo = {
  is_provisioned: true,
  host: "ep-x.neon.tech",
  database: "neondb",
  provider: "neon",
  project_name: "oxy-org-a",
  console_url: null,
  region: "aws-us-west-2",
  status: "active",
  analyst_role: "oxy_analyst_ro",
  analyst_ready: true,
  platform_schema_version: 1,
  expected_platform_schema_version: 1,
  schemas: []
};

const ANSWERED: OltpConnectionTest = {
  ok: true,
  stage: null,
  error: null,
  role: "oxy_analyst_ro",
  host: "ep-x.neon.tech",
  elapsed_ms: 87
};

const idle = () => ({ data: undefined, isPending: false, mutate: vi.fn() });

// The hooks are the seam. The probe keeps its result in component state, as
// a `useMutation` does: that is the property under test.
vi.mock("@/hooks/api/oltp/useAdminOltp", () => ({
  useAdminOltpStatus: () => ({ data: STATUS, isPending: false, isError: false, error: null }),
  useProvisionOltp: () => idle(),
  useOltpCredentials: () => idle(),
  useSetOltpVisibility: () => idle(),
  useDeprovisionOltp: () => idle(),
  useTestOltpConnection: () => {
    const [data, setData] = useState<OltpConnectionTest | undefined>(undefined);
    return { data, isPending: false, mutate: () => setData(ANSWERED) };
  }
}));

describe("OLTP tenant panel", () => {
  it("drops one org's connection test when another org is selected", () => {
    const { rerender } = render(<OltpTenantPanel orgId='org-a' />);
    fireEvent.click(screen.getByTestId("admin-org-oltp-test-run"));
    expect(screen.getByTestId("admin-org-oltp-test-verdict")).toBeTruthy();

    rerender(<OltpTenantPanel orgId='org-b' />);
    expect(screen.queryByTestId("admin-org-oltp-test-verdict")).toBeNull();
  });
});
