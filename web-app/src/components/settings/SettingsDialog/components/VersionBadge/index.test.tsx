// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { VersionInfo } from "@/services/api/version";

const getVersion = vi.fn<() => Promise<VersionInfo>>();
vi.mock("@/services/api/version", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/api/version")>()),
  getVersion: () => getVersion()
}));

import { VersionBadge } from "./index";

// The build details popover is Oxy staff's; everyone else sees only the name.
let staff = true;
vi.mock("@/hooks/api/users/useCurrentUser", () => ({
  default: () => ({ data: { is_owner: false, is_app_admin: staff } })
}));

const info = (over: Partial<VersionInfo> = {}): VersionInfo => ({
  image_tag: "main-1bcefe3",
  service: "oxy",
  build_info: {
    git_commit: "1bcefe3fb22711b41a378e31338e7352120f61f5",
    git_commit_short: "1bcefe3",
    build_timestamp: "2026-10-15T07:12:50Z",
    build_profile: "release"
  },
  ...over
});

afterEach(() => {
  cleanup();
  getVersion.mockReset();
  staff = true;
});

describe("VersionBadge", () => {
  it("names the build by its image tag, with no semver", async () => {
    getVersion.mockResolvedValue(info());
    render(<VersionBadge />);
    expect((await screen.findByTestId("version-badge")).textContent).toBe("main-1bcefe3");

    await userEvent.click(screen.getByTestId("version-badge"));
    expect(screen.getByTestId("version-badge-image").textContent).toBe("main-1bcefe3");
    expect(screen.queryByText(/^Version:/)).toBeNull();
  });

  it("falls back to the commit for a server that is not an image", async () => {
    getVersion.mockResolvedValue(
      info({ image_tag: null, build_info: { ...info().build_info, git_commit_short: "dev" } })
    );
    render(<VersionBadge />);
    expect((await screen.findByTestId("version-badge")).textContent).toBe("dev");

    await userEvent.click(screen.getByTestId("version-badge"));
    expect(screen.getByTestId("version-badge-image").textContent).toBe("not an image");
  });

  it("shows everyone but Oxy staff only the build's name, with no commit", async () => {
    staff = false;
    getVersion.mockResolvedValue(info());
    render(<VersionBadge />);
    expect((await screen.findByTestId("version-badge")).textContent).toBe("main-1bcefe3");
    await userEvent.click(screen.getByTestId("version-badge"));
    expect(screen.queryByTestId("version-badge-image")).toBeNull();
  });
});
