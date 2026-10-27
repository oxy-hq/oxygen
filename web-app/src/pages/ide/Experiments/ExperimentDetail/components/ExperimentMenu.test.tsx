// @vitest-environment jsdom

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Design, ExperimentListItem } from "@/types/experiment";
import { definitionFixture } from "../../fixtures";
import ExperimentMenu from "./ExperimentMenu";

const h = vi.hoisted(() => ({
  save: vi.fn(),
  duplicate: vi.fn(),
  navigate: vi.fn(),
  frozenAt: null as string | null,
  reopenable: false
}));

vi.mock("@/hooks/api/useMeasureFilterOptions", () => ({
  useMeasureDimensions: () => ({ view: "", dimensions: [], isLoading: false }),
  useDimensionValues: () => ({ data: [], isLoading: false, isError: false })
}));
vi.mock("@/hooks/api/organizations/useOrgMembers", () => ({
  useOrgMembers: () => ({ data: undefined })
}));
vi.mock("@/hooks/api/appAccess/useTeams", () => ({ useTeams: () => ({ data: undefined }) }));
vi.mock("@/hooks/api/useWorldModel", () => ({
  useWorldModel: () => ({ data: undefined }),
  useWmInstances: () => ({ data: undefined })
}));
vi.mock("@/hooks/api/useMetricTree", () => ({
  useMetricTree: () => ({ data: undefined }),
  useTimeDimensions: () => ({ data: undefined })
}));
vi.mock("@/hooks/api/automations/useAutomations", () => ({
  default: () => ({ data: undefined })
}));
vi.mock("@/hooks/api/useExperiments", () => ({
  useExperimentDesign: () => ({
    data: {
      id: "design-1",
      content_hash: "hash-2",
      frozen_at: h.frozenAt,
      reopenable: h.reopenable,
      updated_at: "2026-02-18T03:15:00Z",
      config: definitionFixture
    } satisfies Partial<Design>,
    isError: false
  }),
  useSaveDesign: () => ({ mutateAsync: h.save, isPending: false }),
  useDuplicateExperiment: () => ({ mutateAsync: h.duplicate, isPending: false })
}));
vi.mock("react-router-dom", async (orig) => ({
  ...(await orig<typeof import("react-router-dom")>()),
  useNavigate: () => h.navigate
}));
vi.mock("../../useExperimentRoutes", () => ({
  useExperimentRoutes: () => ({ DETAIL: (name: string) => `/experiments/${name}` })
}));
vi.mock("@/hooks/useRole", () => ({
  useRole: () => ({ is: { workspaceAdmin: true, workspaceEditor: true } })
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));

const summary = (): ExperimentListItem => ({
  name: "e",
  owner: "ops",
  hypothesis: "h",
  design_id: "design-1",
  design_hash: "hash-2",
  design_frozen_at: h.frozenAt,
  status: h.frozenAt ? "live" : "draft",
  registration_refusal: null,
  next_switch_on: null,
  retrospective: false,
  retrospective_ready: false,
  proposals_count: 0,
  latest_proposal: null,
  definition: definitionFixture
});

const renderSection = () =>
  render(
    <MemoryRouter>
      <ExperimentMenu summary={summary()} />
    </MemoryRouter>
  );

const openMenu = (user: ReturnType<typeof userEvent.setup>) =>
  user.click(screen.getByTestId("experiment-menu"));

beforeEach(() => {
  h.frozenAt = null;
  h.reopenable = false;
  h.save.mockReset().mockResolvedValue({ id: "design-1" });
  h.duplicate.mockReset().mockResolvedValue({ id: "design-9" });
  h.navigate.mockReset();
});
afterEach(cleanup);

describe("ExperimentMenu", () => {
  it("offers Edit in the menu for a draft, and no Design card", async () => {
    const user = userEvent.setup();
    renderSection();

    expect(screen.queryByTestId("experiment-designs")).toBeNull();
    await openMenu(user);
    expect(screen.getByTestId("experiment-edit-design")).toBeInTheDocument();
    expect(screen.queryByTestId("experiment-duplicate-design")).toBeNull();
  });

  it("opens the form pre-filled from the draft and saves over its content hash", async () => {
    const user = userEvent.setup();
    renderSection();

    await openMenu(user);
    await user.click(screen.getByTestId("experiment-edit-design"));
    const field = (name: string) =>
      screen.getByTestId(`experiment-design-${name}`) as HTMLInputElement;
    expect(field("experiment").value).toBe("customer_orders_pilot");
    expect(field("experiment").readOnly).toBe(true);
    expect(field("grace-days").value).toBe("2");
    expect(field("waves-1-units").value).toBe("carol, dave");
    expect(field("guardrails-0-measure").value).toBe("orders.refund_rate");
    expect(screen.queryByText(/yaml/i)).toBeNull();

    await user.clear(field("owner"));
    await user.type(field("owner"), "growth");
    await user.click(screen.getByTestId("experiment-design-submit"));

    await waitFor(() => expect(h.save).toHaveBeenCalledTimes(1));
    // The stored design's defaults are not sent back: the server fills them again.
    expect(h.save).toHaveBeenCalledWith({
      config: {
        experiment: "customer_orders_pilot",
        hypothesis: "A customer-level promo lifts order value per customer-day",
        owner: "growth",
        for_each: "customer",
        lever: {
          on: { via: "fruit_summary" },
          off: { via: "fruit_summary" },
          read_back: {
            when_on: "{{orders.total_order_value}} > 0",
            when_off: "{{orders.total_order_value}} >= 0",
            grace_days: 2
          }
        },
        design: {
          timezone: "UTC",
          time_dimension: "orders.order_date",
          waves: [
            { switch_on: "2026-03-16", units: ["alice", "bob"] },
            { switch_on: "2026-03-23", units: ["carol", "dave"] }
          ],
          seed: 42,
          pre_days: 56,
          post_days: 56,
          anticipation_days: 3,
          washout_days: 7
        },
        outcome: {
          target: "orders.total_order_value",
          direction: "increase",
          mde_at_proposal: 1.25
        },
        substitution_check: { sibling: "orders.total_orders" },
        guardrails: [
          {
            criteria: { type: "anomaly", measure: "orders.refund_rate", significance: 0.01 },
            action: "abort"
          }
        ]
      },
      baseHash: "hash-2"
    });
  });

  it("shows the stale-hash 409 at the top of the form", async () => {
    h.save.mockRejectedValue(new Error("the design changed since you opened it; reload and retry"));
    const user = userEvent.setup();
    renderSection();

    await openMenu(user);
    await user.click(screen.getByTestId("experiment-edit-design"));
    await user.click(screen.getByTestId("experiment-design-submit"));

    expect(await screen.findByTestId("experiment-design-error")).toHaveTextContent(
      "The design changed since you opened it"
    );
  });

  it("places each reason of a 422 beside its field, in the field's own words", async () => {
    h.save.mockRejectedValue(
      new Error(
        "the design is not valid: outcome.alpha is 1.5, which is not in (0, 1); lever.read_back.grace_days is 9, which is not shorter than design.switchback.period_days (7); the target could not be resolved"
      )
    );
    const user = userEvent.setup();
    renderSection();

    await openMenu(user);
    await user.click(screen.getByTestId("experiment-edit-design"));
    await user.click(screen.getByTestId("experiment-design-submit"));

    const alpha = await screen.findByTestId("experiment-design-alpha-error");
    expect(alpha).toHaveTextContent('"Significance level" is 1.5, which is not in (0, 1)');
    expect(screen.getByTestId("experiment-design-grace-days-error")).toHaveTextContent(
      '"Days allowed to confirm a switch" is 9, which is not shorter than "Period length" (7)'
    );
    const top = screen.getByTestId("experiment-design-error");
    expect(top).toHaveTextContent("The target could not be resolved");
    expect(top).toHaveTextContent("The other reasons are shown beside their fields.");
    expect(top).not.toHaveTextContent("outcome.alpha");
  });

  it("offers Duplicate, not Edit, in the menu once registered, and never says frozen", async () => {
    h.frozenAt = "2026-02-20T03:15:00Z";
    const user = userEvent.setup();
    renderSection();

    await openMenu(user);
    expect(screen.getByTestId("experiment-duplicate-design")).toHaveTextContent(
      "Duplicate as new draft"
    );
    expect(screen.queryByTestId("experiment-edit-design")).toBeNull();
    expect(document.body).not.toHaveTextContent(/froze/i);
  });

  it("offers Edit for a refused analysis of past data, which reopens it", async () => {
    h.frozenAt = "2026-02-20T03:15:00Z";
    h.reopenable = true;
    const user = userEvent.setup();
    renderSection();

    await openMenu(user);
    expect(screen.getByTestId("experiment-edit-design")).toHaveTextContent("Edit design");
    expect(screen.queryByTestId("experiment-duplicate-design")).toBeNull();
  });

  it("duplicates under the typed name, defaulting to <name>_copy, then opens the new experiment", async () => {
    h.frozenAt = "2026-02-20T03:15:00Z";
    const user = userEvent.setup();
    renderSection();

    await openMenu(user);
    await user.click(screen.getByTestId("experiment-duplicate-design"));
    const input = screen.getByTestId("experiment-duplicate-name") as HTMLInputElement;
    expect(input.value).toBe("e_copy");
    await user.clear(input);
    await user.type(input, "e_v2");
    await user.click(screen.getByTestId("experiment-duplicate-submit"));

    await waitFor(() => expect(h.duplicate).toHaveBeenCalledWith("e_v2"));
    expect(h.navigate).toHaveBeenCalledWith("/experiments/e_v2");
  });

  it("keeps the dialog open with the server's message when the name is taken", async () => {
    h.frozenAt = "2026-02-20T03:15:00Z";
    h.duplicate.mockRejectedValue(new Error("an experiment named `e_copy` already exists"));
    const user = userEvent.setup();
    renderSection();

    await openMenu(user);
    await user.click(screen.getByTestId("experiment-duplicate-design"));
    await user.click(screen.getByTestId("experiment-duplicate-submit"));

    expect(await screen.findByTestId("experiment-duplicate-error")).toHaveTextContent(
      "already exists"
    );
    expect(h.navigate).not.toHaveBeenCalled();
  });
});
