// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppAccessDialog } from "./AppAccessDialog";

/**
 * The dialog takes the reference design only where it is asked to (the home's
 * gear); the settings, admin and partner consoles keep it as it was.
 */

vi.mock("@/hooks/api/appAccess", () => ({
  useAppAccess: () => ({
    data: { app_id: "a1", visibility: "org", grants: [] },
    isFetching: false,
    isError: false
  }),
  useGrantableTeams: () => ({ data: [] }),
  useGrantablePeople: () => ({ data: [], isError: false }),
  useSetAppAccess: () => ({ mutateAsync: vi.fn(), isPending: false })
}));

const renderDialog = (reference?: boolean) =>
  render(
    <AppAccessDialog
      open
      onOpenChange={() => {}}
      scope={{ kind: "org", orgId: "org-1" }}
      appId='a1'
      appName='Store Ops'
      reference={reference}
    />
  );

afterEach(cleanup);

describe("AppAccessDialog", () => {
  it("keeps its own look by default", () => {
    renderDialog();
    const dialog = screen.getByRole("dialog");
    expect(dialog.className).not.toContain("design-reference");
    expect(dialog.dataset.reference).toBeUndefined();
    expect(screen.getByText("Visibility").className).toContain("font-medium");
  });

  it("draws the reference design when asked, scope and labels included", () => {
    renderDialog(true);
    const dialog = screen.getByRole("dialog");
    expect(dialog.className).toContain("design-reference");
    expect(dialog.dataset.reference).toBe("true");
    expect(screen.getByText("Visibility").className).toContain("uppercase");
    expect(screen.getByTestId("app-access-save").className).toContain("bg-brand");
    expect(screen.getByRole("heading", { name: "Who can open Store Ops" })).toBeTruthy();
  });
});
