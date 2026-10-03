// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PaywallScreen } from "./PaywallScreen";

// The paywall variant under test. `PaywallScreen` picks its copy from this, so
// each case sets it before rendering.
const paywall = vi.hoisted(() => ({ status: "incomplete" }));

// Mock the data hooks so the paywall renders deterministically with no
// network calls. Pricing must NEVER appear in any variant — this test is
// the regression guard for the central "no public pricing" guarantee of the
// 2026-04-28 sales-gated redesign.
vi.mock("@/hooks/api/billing", () => ({
  useOrgBillingStatus: () => ({
    data: {
      status: paywall.status,
      grace_period_ends_at: null,
      payment_action_url: null
    }
  }),
  useCreatePortalSession: () => ({ mutate: vi.fn(), isPending: false })
}));

// A second org and a signed-in user, so the org switcher and the account
// header — both of which sit inside the paywall — are part of the text scanned.
vi.mock("@/hooks/api/organizations", () => ({
  useOrgs: () => ({
    data: [
      { id: "org-1", name: "Paused Org", slug: "paused-org", role: "owner" },
      { id: "org-2", name: "Other Org", slug: "other-org", role: "member" }
    ]
  }),
  useAcceptInvitation: () => ({ mutateAsync: vi.fn(), isPending: false })
}));

vi.mock("@/hooks/api/users/useCurrentUser", () => ({
  default: () => ({ data: { id: "user-1", email: "owner@example.com", name: "Owner" } })
}));

vi.mock("@/contexts/AuthContext", () => ({ useAuth: () => ({ logout: vi.fn() }) }));

vi.mock("@/stores/usePaywallStore", () => ({
  usePaywallStore: (sel: (s: { status: string }) => unknown) => sel({ status: paywall.status })
}));

vi.mock("@/stores/useCurrentOrg", () => ({
  default: (sel: (s: { org: { id: string } | undefined }) => unknown) =>
    sel({ org: { id: "org-1" } })
}));

afterEach(() => cleanup());

const wrap = (ui: React.ReactNode) => {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <MemoryRouter>
      <QueryClientProvider client={qc}>{ui}</QueryClientProvider>
    </MemoryRouter>
  );
};

describe("PaywallScreen — no-pricing regression (2026-04-28 sales-gated)", () => {
  it.each([
    ["incomplete", "admin", true],
    ["incomplete", "member", false],
    ["unpaid", "admin", true],
    ["unpaid", "member", false],
    ["canceled", "admin", true],
    ["canceled", "member", false]
  ] as const)("renders no pricing for %s / %s", (status, _label, isAdmin) => {
    paywall.status = status;
    const { container } = render(wrap(<PaywallScreen isAdmin={isAdmin} />));
    // The absence checks below are only worth something if the screen actually
    // rendered: an empty container contains no pricing either. These are the
    // parts every variant shows.
    expect(screen.getByRole("heading", { level: 1 })).not.toBeEmptyDOMElement();
    expect(screen.getByRole("link", { name: "Contact account team" })).toBeInTheDocument();
    expect(screen.getByText("Other Org")).toBeInTheDocument();

    const text = container.textContent ?? "";
    // Hard regression checks. If a future refactor re-introduces a public
    // pricing surface inside PaywallScreen, one of these will trip.
    expect(text).not.toMatch(/\$\d/);
    expect(text.toLowerCase()).not.toContain("subscribe");
    expect(text).not.toMatch(/\/seat/i);
    expect(text).not.toMatch(/\/month/i);
    expect(text).not.toMatch(/\/year/i);
  });
});
