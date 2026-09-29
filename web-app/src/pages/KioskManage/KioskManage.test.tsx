// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import queryKeys from "@/hooks/api/queryKey";
import { FrontlineService } from "@/services/api/frontline";
import type { KioskDevice } from "@/types/frontline";
import type { Organization, OrgRole } from "@/types/organization";
import KioskManagePage from ".";
import { kioskAppName } from "./utils";

// The probe and the org list are the page's inputs; the leave mutation stays
// real, down to the service call, so success and failure are the hook's own.
let device: KioskDevice | undefined;
let orgs: Organization[];

vi.mock("@/hooks/auth/useFrontline", () => ({
  useKioskDevice: () => ({ data: device, isPending: device === undefined })
}));
vi.mock("@/hooks/api/organizations", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/api/organizations")>()),
  useOrgs: () => ({ data: orgs })
}));
vi.mock("@/services/api/frontline", () => ({
  FrontlineService: { leaveKioskMode: vi.fn() }
}));
vi.mock("@/components/OxyLogo", () => ({ default: () => null }));

const leaveKioskMode = vi.mocked(FrontlineService.leaveKioskMode);

const APP_URL = "https://app.oxygen-hq.com/customer-apps/poke-house/store-ops/";

const KIOSK: KioskDevice = {
  bound: true,
  id: "kiosk-1",
  org: "poke-house",
  orgName: "Poke House",
  device: "Front counter",
  location: { id: "loc-1", name: "Clovis" },
  returnTo: APP_URL
};

const org = (slug: string, role: OrgRole): Organization => ({
  id: `${slug}-id`,
  name: slug,
  slug,
  role
});

const renderPage = () => {
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  const invalidate = vi.spyOn(client, "invalidateQueries");
  const user = userEvent.setup();
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/kiosk"]}>
        <KioskManagePage />
      </MemoryRouter>
    </QueryClientProvider>
  );
  return { user, invalidate };
};

beforeEach(() => {
  device = undefined;
  orgs = [];
  localStorage.clear();
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("/kiosk — not a kiosk", () => {
  it("says so, and points at where a kiosk is revoked", () => {
    device = { bound: false };
    orgs = [org("acme", "member"), org("poke-house", "admin")];
    renderPage();
    expect(screen.getByText("This browser isn't a store tablet")).toBeInTheDocument();
    expect(screen.getByText(/Settings → Organization → Crew → Kiosks/)).toBeInTheDocument();
    // The org the viewer administers, opened at Crew.
    expect(screen.getByTestId("kiosk-manage-crew-settings")).toHaveAttribute(
      "href",
      "/poke-house?settings=organization.crew"
    );
    expect(screen.getByTestId("kiosk-manage-open-oxygen")).toHaveAttribute("href", "/");
    expect(screen.queryByTestId("kiosk-manage-leave")).toBeNull();
  });
});

describe("/kiosk — a kiosk", () => {
  it("shows a crew-facing member the tablet and its ways out, but not Leave", () => {
    device = KIOSK;
    orgs = [org("poke-house", "member")];
    renderPage();
    expect(screen.getByTestId("kiosk-manage-summary")).toHaveTextContent(
      "Front counter · Clovis · Poke House"
    );
    expect(screen.getByTestId("kiosk-manage-open-app")).toHaveAttribute("href", APP_URL);
    expect(screen.getByTestId("kiosk-manage-open-app")).toHaveTextContent("Open Store Ops");
    expect(screen.getByTestId("kiosk-manage-open-oxygen")).toHaveAttribute("href", "/");
    expect(screen.queryByTestId("kiosk-manage-leave")).toBeNull();
    expect(screen.getByTestId("kiosk-manage-admin-only")).toHaveTextContent(
      "Only an owner or admin of Poke House"
    );
  });

  it("offers no app button when the kiosk opens none", () => {
    device = { ...KIOSK, returnTo: null, location: null };
    orgs = [org("poke-house", "admin")];
    renderPage();
    expect(screen.getByTestId("kiosk-manage-summary")).toHaveTextContent(
      /^Front counter · Poke House$/
    );
    expect(screen.queryByTestId("kiosk-manage-open-app")).toBeNull();
  });

  it("does not offer Leave to an admin of some other org", () => {
    device = KIOSK;
    orgs = [org("acme", "owner")];
    renderPage();
    expect(screen.queryByTestId("kiosk-manage-leave")).toBeNull();
  });

  it.each<OrgRole>(["owner", "admin"])(
    "lets the kiosk org's %s leave, after confirming in the page",
    async (role) => {
      device = KIOSK;
      orgs = [org("poke-house", role)];
      const nativeConfirm = vi.spyOn(window, "confirm");
      const { user } = renderPage();

      await user.click(screen.getByTestId("kiosk-manage-leave"));
      expect(screen.getByTestId("kiosk-manage-leave-panel")).toBeInTheDocument();
      expect(nativeConfirm).not.toHaveBeenCalled();
      expect(leaveKioskMode).not.toHaveBeenCalled();

      await user.click(screen.getByTestId("kiosk-manage-leave-cancel"));
      expect(screen.queryByTestId("kiosk-manage-leave-panel")).toBeNull();
      expect(leaveKioskMode).not.toHaveBeenCalled();
    }
  );
});

describe("/kiosk — leaving kiosk mode", () => {
  it("revokes through the org's route, says the browser is back to normal, and drops the probe", async () => {
    device = KIOSK;
    orgs = [org("poke-house", "admin")];
    leaveKioskMode.mockResolvedValue(undefined);
    localStorage.setItem("oxy_kiosk_browser", "1");
    const { user, invalidate } = renderPage();

    await user.click(screen.getByTestId("kiosk-manage-leave"));
    await user.click(screen.getByTestId("kiosk-manage-leave-confirm"));

    await waitFor(() => expect(screen.getByTestId("kiosk-manage-left")).toBeInTheDocument());
    // Leaving is one of the acts that may forget the kiosk; the probe that
    // follows is not.
    expect(localStorage.getItem("oxy_kiosk_browser")).toBeNull();
    expect(leaveKioskMode).toHaveBeenCalledWith("poke-house-id");
    expect(screen.getByText("This browser is back to normal")).toBeInTheDocument();
    expect(screen.getByTestId("kiosk-manage-left-home")).toHaveAttribute("href", "/");
    expect(invalidate).toHaveBeenCalledWith({ queryKey: queryKeys.frontline.device() });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: queryKeys.org.frontlineDevices("poke-house-id")
    });
  });

  it("stays on the tablet and says why when the server refuses", async () => {
    device = KIOSK;
    orgs = [org("poke-house", "admin")];
    leaveKioskMode.mockRejectedValue({
      isAxiosError: true,
      response: { status: 404, data: { error: "this browser is not a kiosk of this organization" } }
    });
    const { user, invalidate } = renderPage();

    await user.click(screen.getByTestId("kiosk-manage-leave"));
    await user.click(screen.getByTestId("kiosk-manage-leave-confirm"));

    await waitFor(() =>
      expect(screen.getByTestId("kiosk-manage-leave-error")).toHaveTextContent(
        "This browser isn't a kiosk of Poke House any more"
      )
    );
    expect(screen.queryByTestId("kiosk-manage-left")).toBeNull();
    expect(screen.getByTestId("kiosk-manage-leave-confirm")).toBeEnabled();
    expect(invalidate).not.toHaveBeenCalledWith({ queryKey: queryKeys.frontline.device() });
  });

  it("says nothing changed and to try again when the server can't tell", async () => {
    device = KIOSK;
    orgs = [org("poke-house", "admin")];
    localStorage.setItem("oxy_kiosk_browser", "1");
    leaveKioskMode.mockRejectedValue({
      isAxiosError: true,
      response: { status: 503, data: { error: "database unavailable" } }
    });
    const { user } = renderPage();

    await user.click(screen.getByTestId("kiosk-manage-leave"));
    await user.click(screen.getByTestId("kiosk-manage-leave-confirm"));

    const error = await screen.findByTestId("kiosk-manage-leave-error");
    expect(error).toHaveTextContent("still a kiosk");
    expect(error).toHaveTextContent("Try again in a moment");
    expect(error).not.toHaveTextContent("isn't a kiosk");
    expect(screen.getByTestId("kiosk-manage-leave-confirm")).toBeEnabled();
    expect(localStorage.getItem("oxy_kiosk_browser")).toBe("1");
  });
});

describe("kioskAppName", () => {
  it("names the app from either custom-app URL scheme", () => {
    expect(kioskAppName(APP_URL)).toBe("Store Ops");
    expect(kioskAppName("https://poke-house--store-ops.customer-apps.oxygen-hq.com/")).toBe(
      "Store Ops"
    );
    expect(kioskAppName("https://dev--poke--inventory.customer-apps.oxy.tech/")).toBe("Inventory");
    expect(kioskAppName("https://app.oxygen-hq.com/poke-house")).toBeNull();
    expect(kioskAppName("not a url")).toBeNull();
    expect(kioskAppName(null)).toBeNull();
  });
});
