// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import WriteBackAction from "./WriteBackAction";

const apply = vi.hoisted(() => ({ mutateAsync: vi.fn(), setCurrentBranch: vi.fn() }));

vi.mock("@/hooks/api/useExperiments", () => ({
  useApplyWriteBack: () => ({ mutateAsync: apply.mutateAsync, isPending: false })
}));
vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "ws-1" }, branchName: "main" })
}));
vi.mock("@/stores/useIdeBranch", () => ({
  default: () => ({ setCurrentBranch: apply.setCurrentBranch })
}));

const role = vi.hoisted(() => ({ admin: true, editor: true }));
vi.mock("@/hooks/useRole", () => ({
  useRole: () => ({ is: { workspaceAdmin: role.admin, workspaceEditor: role.editor } })
}));

const Where = () => <output data-testid='where'>{useLocation().pathname}</output>;
const renderAction = () =>
  render(
    <MemoryRouter>
      <WriteBackAction name='beverage_promo_pilot' />
      <Where />
    </MemoryRouter>
  );

beforeEach(() => {
  role.admin = true;
  role.editor = true;
  apply.mutateAsync.mockReset();
  apply.setCurrentBranch.mockReset();
});
afterEach(cleanup);

describe("WriteBackAction", () => {
  it("is not offered to a viewer, who is told an editor is needed", () => {
    role.editor = false;
    renderAction();

    expect(screen.queryByTestId("experiment-writeback-apply")).toBeNull();
    expect(screen.getByTestId("experiment-writeback-denied")).toHaveTextContent("workspace editor");
  });

  it("creates the branch and links to it, switching the IDE onto it first", async () => {
    apply.mutateAsync.mockResolvedValue({
      branch: "experiments/beverage_promo_pilot",
      file_path: "sales_semantics/views/oxymart.view.yml"
    });
    renderAction();

    fireEvent.click(screen.getByTestId("experiment-writeback-apply"));

    expect(await screen.findByTestId("experiment-writeback-branch")).toHaveTextContent(
      "experiments/beverage_promo_pilot"
    );
    fireEvent.click(screen.getByTestId("experiment-writeback-open"));

    expect(apply.setCurrentBranch).toHaveBeenCalledWith("ws-1", "experiments/beverage_promo_pilot");
    expect(screen.getByTestId("where").textContent).toMatch(/\/ide\/files\/.+/);
  });

  it("shows a failed apply inline and leaves the button available to retry", async () => {
    apply.mutateAsync.mockRejectedValue(new Error("the IDE node is unavailable"));
    renderAction();

    fireEvent.click(screen.getByTestId("experiment-writeback-apply"));

    expect(await screen.findByTestId("experiment-writeback-error")).toHaveTextContent(
      "the IDE node is unavailable"
    );
    await waitFor(() => expect(screen.getByTestId("experiment-writeback-apply")).toBeEnabled());
  });
});
