// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ExperimentListItem } from "@/types/experiment";
import { definitionFixture } from "../../fixtures";
import { COLUMN } from "../column";
import RegistrableView from "./RegistrableView";

const h = vi.hoisted(() => ({
  register: vi.fn(),
  admin: true
}));

vi.mock("@/hooks/api/useExperiments", () => ({
  useExperimentDesign: () => ({
    data: { id: "design-2", content_hash: "hash-2", updated_at: "2026-02-18T03:15:00Z" }
  }),
  useRegisterExperiment: () => ({ mutateAsync: h.register, isPending: false })
}));
vi.mock("@/hooks/useRole", () => ({
  useRole: () => ({ is: { workspaceAdmin: h.admin, workspaceEditor: true } })
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

const summary: ExperimentListItem = {
  name: "e",
  owner: "ops",
  hypothesis: "h",
  design_id: "design-2",
  design_hash: "hash-2",
  design_frozen_at: null,
  status: "registrable",
  registration_refusal: null,
  next_switch_on: "2026-03-02",
  retrospective: false,
  retrospective_ready: false,
  proposals_count: 0,
  latest_proposal: null,
  report_glance: null,
  definition: definitionFixture
};

beforeEach(() => {
  h.admin = true;
  h.register.mockReset().mockResolvedValue({ run_id: "r", design_id: "design-2" });
});
afterEach(cleanup);

describe("RegistrableView", () => {
  it("confirms what registering pins, then registers with the content hash", async () => {
    const user = userEvent.setup();
    render(<RegistrableView summary={summary} />);

    await user.click(screen.getByTestId("experiment-register"));
    const dialog = screen.getByTestId("experiment-register-dialog");
    expect(dialog).toHaveTextContent("Registering is final");
    expect(dialog).toHaveTextContent("cannot be undone");
    expect(dialog).not.toHaveTextContent(/version|froze/i);
    expect(dialog).toHaveTextContent("flipping the lever on schedule");
    expect(h.register).not.toHaveBeenCalled();

    await user.click(screen.getByTestId("experiment-register-confirm"));
    expect(h.register).toHaveBeenCalledWith("hash-2");
  });

  it("keeps a 403 in the dialog as the permission message", async () => {
    h.register.mockRejectedValue(
      new Error("You don't have permission to register this experiment.")
    );
    const user = userEvent.setup();
    render(<RegistrableView summary={summary} />);

    await user.click(screen.getByTestId("experiment-register"));
    await user.click(screen.getByTestId("experiment-register-confirm"));

    expect(await screen.findByTestId("experiment-register-error")).toHaveTextContent(
      "don't have permission to register"
    );
  });

  it("disables Register for someone who is not a workspace owner or admin", () => {
    h.admin = false;
    render(<RegistrableView summary={summary} />);

    expect(screen.getByTestId("experiment-register")).toBeDisabled();
  });

  it("leads with a status badge and the deadline as a tile", () => {
    render(<RegistrableView summary={summary} />);

    expect(screen.getByTestId("experiment-registrable-status")).toHaveTextContent(
      "Ready to register"
    );
    expect(screen.getByTestId("experiment-registrable-design")).toHaveTextContent("Draft");
    expect(screen.getByTestId("experiment-registrable-deadline")).toHaveTextContent("2026-03-02");
  });

  it("sits in the page's shared column, aligned with the Definition", () => {
    render(<RegistrableView summary={summary} />);
    const root = screen.getByTestId("experiment-registrable");
    for (const c of COLUMN.split(" ")) expect(root).toHaveClass(c);
  });
});
