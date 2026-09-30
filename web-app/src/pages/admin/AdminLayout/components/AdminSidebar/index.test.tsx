// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SidebarProvider } from "@/components/ui/shadcn/sidebar";
import type { PlatformCapability } from "@/types/auth";
import { AdminSidebar } from ".";

// The rail's workspace-health badge used to fetch for every staff member, so an App
// Operator — never shown Workspace health, 403'd by its endpoint — got "You don't have
// permission to do this." on every admin page. The fetch now follows the rail entry.
const user = vi.hoisted(() => ({
  value: { is_owner: false, platform_capabilities: [] as PlatformCapability[] }
}));
const healthCalls = vi.hoisted(() => [] as Array<{ enabled?: boolean }>);

vi.mock("@/hooks/api/users/useCurrentUser", () => ({ default: () => ({ data: user.value }) }));
vi.mock("@/hooks/api/workspaceHealth/useWorkspaceHealth", () => ({
  useWorkspaceHealth: (options: { enabled?: boolean } = {}) => {
    healthCalls.push(options);
    return { data: undefined };
  }
}));
vi.mock("./components/Footer", () => ({ Footer: () => null }));

function renderAs(isOwner: boolean, capabilities: PlatformCapability[]) {
  user.value = { is_owner: isOwner, platform_capabilities: capabilities };
  render(
    <MemoryRouter initialEntries={["/admin/apps"]}>
      <SidebarProvider>
        <AdminSidebar />
      </SidebarProvider>
    </MemoryRouter>
  );
  return healthCalls.map((c) => c.enabled);
}

describe("AdminSidebar's workspace-health badge", () => {
  beforeEach(() => {
    cleanup();
    healthCalls.length = 0;
  });

  it("never fetches for an App Operator, who is not shown Workspace health", () => {
    const enabled = renderAs(false, ["manage_apps", "develop_apps"]);
    expect(enabled.length).toBeGreaterThan(0);
    expect(enabled.every((e) => e === false)).toBe(true);
  });

  it("fetches for staff holding operate_platform, and for the owner", () => {
    expect(renderAs(false, ["operate_platform"]).every((e) => e === true)).toBe(true);
    healthCalls.length = 0;
    expect(renderAs(true, []).every((e) => e === true)).toBe(true);
  });
});
