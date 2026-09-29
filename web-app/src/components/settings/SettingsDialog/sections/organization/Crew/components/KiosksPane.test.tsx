// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { KioskDevice, KioskDeviceRow } from "@/types/frontline";
import { KiosksPane } from "./KiosksPane";

// Which kiosk this browser is: the one input the badge reads. Everything the
// rows do (reissue, revoke, the idle editor, the new-kiosk dialog) is inert.
let thisBrowser: KioskDevice | undefined;

vi.mock("@/hooks/auth/useFrontline", () => ({
  useKioskDevice: () => ({ data: thisBrowser })
}));
vi.mock("@/hooks/api/organizations", () => {
  const idle = () => ({ mutate: vi.fn(), mutateAsync: vi.fn(), isPending: false });
  return {
    useReissueEnrolLink: idle,
    useRevokeDevice: idle,
    useUpdateDevice: idle,
    useCreateDevice: idle,
    useLocations: () => ({ data: [] })
  };
});

afterEach(() => {
  cleanup();
  thisBrowser = undefined;
});

const row = (id: string, name: string): KioskDeviceRow => ({
  id,
  name,
  return_to: null,
  created_at: "2026-09-01T00:00:00Z",
  bound_at: "2026-09-01T01:00:00Z",
  last_seen_at: null,
  revoked_at: null,
  enrol_expires_at: null,
  location_id: null,
  location_name: null,
  idle_timeout_seconds: 1800
});

const DEVICES = [row("kiosk-1", "Front counter"), row("kiosk-2", "Drive-thru")];

const pane = () =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <KiosksPane
        orgId='org-1'
        orgSlug='poke-house'
        apps={[]}
        devices={DEVICES}
        isPending={false}
        isError={false}
      />
    </QueryClientProvider>
  );

const bound = (id: string | undefined): KioskDevice => ({
  bound: true,
  id,
  org: "poke-house",
  orgName: "Poke House",
  device: "Front counter",
  returnTo: null
});

describe("KiosksPane — This browser", () => {
  it("marks the row whose id is this browser's kiosk, and only that row", () => {
    thisBrowser = bound("kiosk-1");
    pane();
    expect(screen.getByTestId("settings-crew-kiosk-this-browser-kiosk-1")).toHaveTextContent(
      "This browser"
    );
    expect(screen.queryByTestId("settings-crew-kiosk-this-browser-kiosk-2")).toBeNull();
    expect(screen.getAllByText("This browser")).toHaveLength(1);
  });

  it("marks nothing in a browser that is no kiosk, or one the server did not name", () => {
    thisBrowser = { bound: false };
    const { unmount } = pane();
    expect(screen.queryByText("This browser")).toBeNull();
    unmount();

    // A server older than the `id` field: no row can be matched, so none is.
    thisBrowser = bound(undefined);
    pane();
    expect(screen.queryByText("This browser")).toBeNull();
  });
});
