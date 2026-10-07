// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PlatformCapability } from "@/types/auth";

// Settings holds one preference, behind `operate_platform`. Offering the link to someone
// without it — an App Operator — sends them to a page that can only answer 403.
const user = vi.hoisted(() => ({
  value: undefined as { is_owner: boolean; platform_capabilities: PlatformCapability[] } | undefined
}));

vi.mock("@/hooks/api/users/useCurrentUser", () => ({ default: () => ({ data: user.value }) }));
vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({
    logout: vi.fn(),
    getUser: () => JSON.stringify({ email: "staff@oxy.tech", name: "Staff Member" })
  })
}));

import { Footer } from "./Footer";

type Standing = { is_owner: boolean; platform_capabilities: PlatformCapability[] };

/** Renders the footer for whatever `GET /user` answered, and opens its user menu. */
async function openMenuFor(current: Standing | undefined) {
  user.value = current;
  render(
    <MemoryRouter initialEntries={["/admin/apps"]}>
      <Footer />
    </MemoryRouter>
  );
  await userEvent.click(screen.getByTestId("admin-sidebar-user-menu"));
  // The menu itself must be open before "Settings is absent" means anything: a menu that
  // never opened has no Settings item for anyone, and every negative below would pass.
  expect(screen.getByTestId("admin-sidebar-logout")).toBeTruthy();
}

const openMenuAs = (isOwner: boolean, capabilities: PlatformCapability[]) =>
  openMenuFor({ is_owner: isOwner, platform_capabilities: capabilities });

afterEach(cleanup);

describe("the rail footer's Settings link", () => {
  it("is offered to staff holding operate_platform, and goes to the settings page", async () => {
    await openMenuAs(false, ["operate_platform"]);
    const settings = screen.getByTestId("admin-sidebar-settings");
    expect(settings.textContent).toBe("Settings");
    expect(settings.getAttribute("href")).toBe("/admin/settings");
  });

  it("is offered to the owner, whatever the capability list says", async () => {
    await openMenuAs(true, []);
    expect(screen.getByTestId("admin-sidebar-settings")).toBeTruthy();
  });

  it("is not offered to an App Operator, who holds only manage_apps", async () => {
    await openMenuAs(false, ["manage_apps", "develop_apps"]);
    expect(screen.queryByTestId("admin-sidebar-settings")).toBeNull();
    expect(screen.queryByText("Settings")).toBeNull();
  });

  it("is not offered before the user's standing has loaded", async () => {
    // `GET /user` has not answered yet. Unknown standing is not standing.
    await openMenuFor(undefined);
    expect(screen.queryByTestId("admin-sidebar-settings")).toBeNull();
  });

  it("sits above Log out", async () => {
    await openMenuAs(false, ["operate_platform"]);
    const items = screen.getAllByRole("menuitem").map((el) => el.textContent);
    expect(items).toEqual(["Settings", "Log out"]);
  });
});
