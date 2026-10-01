// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import useCanUsePreviews from "@/hooks/useCanUsePreviews";
import type { AssumeSession } from "@/types/adminAssume";
import OwnerRedirect from "./OwnerRedirect";

/**
 * `OwnerRedirect` keeps a Global Owner in the admin console — except while they
 * act as a tenant. That session is the only door staff have into a tenant's
 * product, and previews (a staff tool) live there: without the exception an
 * `OXY_OWNER` could never see the Previews tab, the preview bar or "Open preview",
 * and an acting owner looped between `AdminLayout` (→ tenant) and here (→ admin).
 */

type User = { email: string; is_owner: boolean; is_app_admin: boolean };

const OWNER: User = { email: "root@oxy.test", is_owner: true, is_app_admin: false };
const MEMBER: User = { email: "crew@acme.test", is_owner: false, is_app_admin: false };

const SESSION: AssumeSession = {
  id: "s-1",
  org_id: "org-1",
  org_name: "Acme",
  org_slug: "acme",
  is_partner: false,
  actor_email: OWNER.email,
  reason: "check the preview",
  started_at: "2026-09-30T00:00:00Z",
  expires_at: "2026-09-30T01:00:00Z",
  expires_in_seconds: 3600
};

const mocks = vi.hoisted(() => ({
  user: undefined as User | undefined,
  sessions: [] as AssumeSession[],
  assumePending: false,
  assumeEnabled: [] as boolean[]
}));

vi.mock("@/hooks/api/users/useCurrentUser", () => ({
  default: () => ({ data: mocks.user, isPending: false })
}));
vi.mock("@/hooks/api/adminAssume", () => ({
  useCurrentAssume: (enabled: boolean) => {
    mocks.assumeEnabled.push(enabled);
    // A disabled react-query query reports `isPending: true` forever — the hook
    // under test must not wait on it for someone who can't act.
    if (!enabled) return { data: undefined, isPending: true };
    return mocks.assumePending
      ? { data: undefined, isPending: true }
      : { data: mocks.sessions, isPending: false };
  }
}));

/** A workspace page standing in for the product: it shows the preview gate. */
function WorkspacePage() {
  const canPreview = useCanUsePreviews();
  return <div data-testid='workspace'>previews:{String(canPreview)}</div>;
}

function Location() {
  const { pathname } = useLocation();
  return <div data-testid='location'>{pathname}</div>;
}

const WS_PATH = "/acme/workspaces/ws-1/ide";

const renderAt = (entry = WS_PATH) =>
  render(
    <MemoryRouter initialEntries={[entry]}>
      <Routes>
        <Route element={<OwnerRedirect />}>
          <Route path=':orgSlug/workspaces/:wsId/*' element={<WorkspacePage />} />
        </Route>
        <Route path='*' element={<Location />} />
      </Routes>
    </MemoryRouter>
  );

beforeEach(() => {
  mocks.user = OWNER;
  mocks.sessions = [];
  mocks.assumePending = false;
  mocks.assumeEnabled = [];
});
afterEach(cleanup);

describe("OwnerRedirect", () => {
  it("bounces an owner with no assume session to the admin queue", () => {
    renderAt();
    expect(screen.getByTestId("location").textContent).toBe("/admin/billing/queue");
    expect(screen.queryByTestId("workspace")).toBeNull();
  });

  it("lets an owner acting as the tenant into the workspace, where previews are open", () => {
    mocks.sessions = [SESSION];
    renderAt();
    expect(screen.getByTestId("workspace").textContent).toBe("previews:true");
    expect(screen.queryByTestId("location")).toBeNull();
  });

  it("waits for the session list before deciding for an owner", () => {
    mocks.assumePending = true;
    renderAt();
    // Neither the product nor the admin queue: an acting owner must not be
    // bounced on the first render just because the list hasn't answered yet.
    expect(screen.queryByTestId("workspace")).toBeNull();
    expect(screen.queryByTestId("location")).toBeNull();
  });

  it("never holds a non-staff member on the session list, and never redirects them", () => {
    mocks.user = MEMBER;
    renderAt();
    expect(screen.getByTestId("workspace").textContent).toBe("previews:false");
    expect(mocks.assumeEnabled.every((enabled) => !enabled)).toBe(true);
  });
});
