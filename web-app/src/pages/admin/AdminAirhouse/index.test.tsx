// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AirhouseFleet, AirhouseFleetRow } from "@/services/api/airhouseAdmin";

const useAirhouseFleet = vi.fn();
vi.mock("@/hooks/api/airhouse/useAdminAirhouse", () => ({
  useAirhouseFleet: () => useAirhouseFleet(),
  useProvisionAirhouseTenant: () => ({ mutateAsync: vi.fn(), isPending: false }),
  airhouseErrorMessage: (_e: unknown, f: string) => f
}));

import AdminAirhouse from "./index";

const unprovisioned = (n: number): AirhouseFleetRow[] =>
  Array.from({ length: n }, (_, i) => ({
    workspace_id: `w-${i}`,
    workspace_name: `workspace-${String(i).padStart(3, "0")}`,
    org_id: null,
    org_name: "Acme",
    status: "none",
    tenant_id: "",
    bucket: "",
    prefix: "",
    service_account_ready: false,
    sa_rotated_at: null,
    created_at: null,
    service_account_id: null,
    sa_created_at: null,
    bearer_max_role: null,
    bearer_max_ttl_secs: null
  }));

const fleet = (rows: AirhouseFleetRow[]): AirhouseFleet => ({
  rows,
  truncated: { unprovisioned: false, provisioned: false }
});

const mount = (rows: AirhouseFleetRow[]) => {
  useAirhouseFleet.mockReturnValue({
    data: fleet(rows),
    isPending: false,
    isError: false,
    error: null
  });
  // Routed: since the migration onto `AdminPage` the page takes its `<h1>` from
  // the admin route map, which is keyed off `useLocation()`. The real path is
  // passed so the heading resolves the way it does in the app rather than
  // falling back to "Admin".
  render(
    <MemoryRouter initialEntries={["/admin/airhouse"]}>
      <AdminAirhouse />
    </MemoryRouter>
  );
};

/**
 * The section's own published state.
 *
 * Read off `aria-expanded` rather than by counting rendered rows: absent rows
 * mean "collapsed" and "not in the tree at all" equally, so a negative
 * assertion stayed green when the section stopped rendering — which is the
 * failure it exists to catch. `getByTestId` throws when the toggle is gone, so
 * this discriminates, and it pins the a11y contract while it is there.
 */
const sectionIsOpen = () =>
  screen.getByTestId("admin-airhouse-unprovisioned-toggle").getAttribute("aria-expanded") ===
  "true";

// Braced: `mockReset()` returns the (callable) mock, and Vitest treats a
// function returned from `beforeEach` as a cleanup hook — so the unbraced form
// invoked the hook once after every test.
beforeEach(() => {
  useAirhouseFleet.mockReset();
});
afterEach(cleanup);

describe("AdminAirhouse — when the 'No warehouse' half starts open", () => {
  it("stays open for a short list, where a collapse would only cost a click", () => {
    mount(unprovisioned(4));
    expect(sectionIsOpen()).toBe(true);
  });

  it("starts collapsed for a long one, which would push the fleet off screen", () => {
    mount(unprovisioned(40));
    expect(sectionIsOpen()).toBe(false);
  });

  /**
   * The branch this test exists for. An operator who typed a query asked for
   * exactly these rows, so leaving them behind a disclosure makes the search
   * look like it found nothing.
   */
  it("opens once the operator has narrowed it themselves", async () => {
    mount(unprovisioned(40));
    expect(sectionIsOpen()).toBe(false);

    await userEvent.type(screen.getByTestId("admin-airhouse-filter"), "workspace-007");
    expect(sectionIsOpen()).toBe(true);
  });

  /**
   * The other half of the rule: what a query opens, clearing it closes again.
   *
   * Deliberately NOT claiming to pin the unfiltered-count choice — with the
   * query clause beside it the two counts coincide, so no test here can tell
   * them apart. That argument lives at the decision site.
   */
  it("re-collapses once the operator clears the query", async () => {
    mount(unprovisioned(40));
    const input = screen.getByTestId("admin-airhouse-filter");

    await userEvent.type(input, "workspace-007");
    expect(sectionIsOpen()).toBe(true);

    await userEvent.clear(input);
    expect(sectionIsOpen()).toBe(false);
  });

  /**
   * An explicit collapse must not outlive the situation it was made in. Pinned
   * for the session, it put every later search's results behind a disclosure —
   * the "search found nothing" problem this rule exists to prevent, back
   * permanently.
   */
  it("forgets an explicit collapse once the operator starts searching", async () => {
    mount(unprovisioned(4));
    await userEvent.click(screen.getByTestId("admin-airhouse-unprovisioned-toggle"));
    expect(sectionIsOpen()).toBe(false);

    await userEvent.type(screen.getByTestId("admin-airhouse-filter"), "workspace-002");
    expect(sectionIsOpen()).toBe(true);
  });

  it("keeps a collapse made during a search", async () => {
    mount(unprovisioned(40));
    await userEvent.type(screen.getByTestId("admin-airhouse-filter"), "workspace-0");
    expect(sectionIsOpen()).toBe(true);

    // Collapsed with the query already running: this one is about the rows on
    // screen, so it holds while they are.
    await userEvent.click(screen.getByTestId("admin-airhouse-unprovisioned-toggle"));
    expect(sectionIsOpen()).toBe(false);
    await userEvent.type(screen.getByTestId("admin-airhouse-filter"), "1");
    expect(sectionIsOpen()).toBe(false);
  });

  it("lets an explicit click win over either default", async () => {
    mount(unprovisioned(4));
    expect(sectionIsOpen()).toBe(true);

    await userEvent.click(screen.getByTestId("admin-airhouse-unprovisioned-toggle"));
    expect(sectionIsOpen()).toBe(false);
  });
});
