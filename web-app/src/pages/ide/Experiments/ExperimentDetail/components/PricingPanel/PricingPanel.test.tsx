// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ExperimentListItem, Proposal } from "@/types/experiment";
import { definitionFixture } from "../../../fixtures";
import { failed, pending, proposal, refused } from "./fixtures";
import PricingPanel from "./index";
import { PRICING_TIMEOUT_MS } from "./proposals";

const state = vi.hoisted(() => ({
  proposals: [] as Proposal[],
  isSuccess: true,
  error: null as Error | null,
  refetch: vi.fn(),
  mutateAsync: vi.fn(),
  adopt: vi.fn(),
  retrospective: vi.fn()
}));

vi.mock("@/hooks/api/useExperiments", () => ({
  useExperimentProposals: () => ({
    data: state.proposals,
    isSuccess: state.isSuccess,
    isError: state.error !== null,
    error: state.error,
    refetch: state.refetch
  }),
  usePriceExperiment: () => ({ mutateAsync: state.mutateAsync, isPending: false }),
  useUseProposal: () => ({ mutateAsync: state.adopt, isPending: false }),
  useExperimentDesign: () => ({ data: { content_hash: "hash-1" } }),
  useRetrospectiveExperiment: () => ({ mutateAsync: state.retrospective, isPending: false })
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

const role = vi.hoisted(() => ({ admin: true, editor: true }));
vi.mock("@/hooks/useRole", () => ({
  useRole: () => ({ is: { workspaceAdmin: role.admin, workspaceEditor: role.editor } })
}));

const summary: ExperimentListItem = {
  name: "customer_orders_pilot",
  owner: "ops",
  hypothesis: "A promo lifts sales",
  design_id: "design-1",
  design_hash: "hash-1",
  design_frozen_at: null,
  status: "draft",
  registration_refusal: null,
  next_switch_on: null,
  retrospective: false,
  retrospective_ready: false,
  proposals_count: 0,
  latest_proposal: null,
  report_glance: null,
  definition: definitionFixture
};

type User = ReturnType<typeof userEvent.setup>;
const testid = (id: string) => screen.getByTestId(id);

/** Row `i` as a valid staggered design. */
const fillWaves = async (user: User, i = 0) => {
  await user.type(testid(`experiment-price-${i}-wave-sizes`), "3, 3");
  fireEvent.change(testid(`experiment-price-${i}-first-switch`), {
    target: { value: "2099-06-01" }
  });
};
/** Row `i` as a valid switchback (period 7 and 6 pairs are the defaults). */
const fillSwitchback = async (user: User, i: number) => {
  await user.click(testid(`experiment-price-${i}-shape-switchback`));
  fireEvent.change(testid(`experiment-price-${i}-first-period`), {
    target: { value: "2099-06-01" }
  });
};
const queueAs = (requestId: string, ids: string[], log: () => Proposal[]) =>
  state.mutateAsync.mockImplementation(async () => {
    state.proposals = log();
    return { request_id: requestId, proposal_ids: ids };
  });

beforeEach(() => {
  role.admin = true;
  role.editor = true;
  state.proposals = [];
  state.isSuccess = true;
  state.error = null;
  state.refetch.mockReset();
  state.mutateAsync.mockReset();
  state.adopt.mockReset().mockResolvedValue({ id: "design-2" });
  state.mutateAsync.mockResolvedValue({ request_id: "req-new", proposal_ids: ["n1"] });
  vi.mocked(toast.error).mockClear();
});
afterEach(cleanup);

describe("PricingPanel past data", () => {
  it("offers Analyse past data on a draft whose dates are past, such as an observed rollout", async () => {
    const user = userEvent.setup();
    render(<PricingPanel summary={{ ...summary, retrospective_ready: true }} />);

    await user.click(testid("experiment-retrospective"));
    expect(state.retrospective).toHaveBeenCalledWith("hash-1");
  });

  it("offers nothing of the kind while the design's dates are still to come", () => {
    render(<PricingPanel summary={summary} />);
    expect(screen.queryByTestId("experiment-retrospective-offer")).toBeNull();
  });
});

describe("PricingPanel permissions", () => {
  it("disables the submit button for a viewer and says an editor is needed", () => {
    role.editor = false;
    render(<PricingPanel summary={summary} />);

    expect(testid("experiment-price-submit")).toBeDisabled();
    expect(testid("experiment-price-denied")).toHaveTextContent("workspace editor");
  });
});

describe("PricingPanel defaults", () => {
  const switchbackDraft: ExperimentListItem = {
    ...summary,
    definition: {
      ...definitionFixture,
      design: {
        ...definitionFixture.design,
        waves: [],
        switchback: { period_days: 7, periods: [] }
      }
    }
  };

  it("starts a switchback draft's candidate as a switchback, and adds switchbacks", async () => {
    const user = userEvent.setup();
    render(<PricingPanel summary={switchbackDraft} />);

    expect(testid("experiment-price-0-shape-switchback")).toBeChecked();
    await user.click(testid("experiment-price-add-candidate"));
    expect(testid("experiment-price-1-shape-switchback")).toBeChecked();
  });

  it("starts a waves draft's candidate as staggered waves", () => {
    render(<PricingPanel summary={summary} />);

    expect(testid("experiment-price-0-shape-waves")).toBeChecked();
  });
});

describe("PricingPanel validation", () => {
  it("shows an inline error next to each empty field of the candidate and queues nothing", async () => {
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    await user.click(testid("experiment-price-submit"));

    expect(await screen.findByTestId("experiment-price-0-wave-sizes-error")).toHaveTextContent(
      "Enter at least one wave size, for example 2, 3."
    );
    expect(testid("experiment-price-0-first-switch-error")).toHaveTextContent(
      "Choose the first switch date."
    );
    expect(state.mutateAsync).not.toHaveBeenCalled();
  });

  it("rejects a wave size below 2 next to the sizes field", async () => {
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    await user.type(testid("experiment-price-0-wave-sizes"), "1, 3");
    await user.click(testid("experiment-price-submit"));

    expect(await screen.findByTestId("experiment-price-0-wave-sizes-error")).toHaveTextContent(
      "Wave sizes must be 2 or more; 1 is too small."
    );
    expect(state.mutateAsync).not.toHaveBeenCalled();
  });

  it("rejects a first switch date in the past", async () => {
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    await user.type(testid("experiment-price-0-wave-sizes"), "3, 3");
    fireEvent.change(testid("experiment-price-0-first-switch"), {
      target: { value: "2020-01-01" }
    });
    await user.click(testid("experiment-price-submit"));

    expect(await screen.findByTestId("experiment-price-0-first-switch-error")).toHaveTextContent(
      "2020-01-01 is already past everywhere"
    );
    expect(state.mutateAsync).not.toHaveBeenCalled();
  });

  it("rejects a switchback's zero pairs next to the pairs field of that candidate only", async () => {
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    await fillWaves(user);
    await user.click(testid("experiment-price-add-candidate"));
    await fillSwitchback(user, 1);
    await user.clear(testid("experiment-price-1-pairs"));
    await user.type(testid("experiment-price-1-pairs"), "0");
    await user.click(testid("experiment-price-submit"));

    expect(await screen.findByTestId("experiment-price-1-pairs-error")).toHaveTextContent(
      "Pairs must be a whole number of 1 or more."
    );
    expect(screen.queryByTestId("experiment-price-0-wave-sizes-error")).toBeNull();
    expect(state.mutateAsync).not.toHaveBeenCalled();
  });

  it("holds the submit button until the proposal log has loaded", () => {
    state.isSuccess = false;
    render(<PricingPanel summary={summary} />);

    expect(testid("experiment-price-submit")).toBeDisabled();
  });

  it("says why the submit is held when the proposal log failed to load, and retries", async () => {
    state.isSuccess = false;
    state.error = new Error("proposal log unavailable");
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    expect(testid("experiment-price-submit")).toBeDisabled();
    expect(testid("experiment-proposals-error")).toHaveTextContent("proposal log unavailable");
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(state.refetch).toHaveBeenCalledTimes(1);
  });

  it("shows a queueing failure inline and does not start waiting", async () => {
    state.mutateAsync.mockRejectedValue(new Error("a request prices 1 to 6 candidates"));
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    await fillWaves(user);
    await user.click(testid("experiment-price-submit"));

    expect(await screen.findByText("a request prices 1 to 6 candidates")).toBeInTheDocument();
    expect(screen.queryByTestId("experiment-price-waiting")).toBeNull();
    expect(toast.error).not.toHaveBeenCalled();
  });
});

describe("PricingPanel candidates", () => {
  it("queues one candidate with the parsed request when the form is valid", async () => {
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    await fillWaves(user);
    await user.click(testid("experiment-price-submit"));

    await waitFor(() =>
      expect(state.mutateAsync).toHaveBeenCalledWith({
        candidates: [
          {
            shape: "waves",
            wave_sizes: [3, 3],
            first_switch_on: "2099-06-01",
            spacing_days: 14,
            alpha: 0.05
          }
        ]
      })
    );
  });

  it("queues matched pairs with no sizes or blocks, which it hides", async () => {
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    await user.click(testid("experiment-price-0-match-pairs"));
    expect(screen.queryByTestId("experiment-price-0-wave-sizes")).toBeNull();
    expect(screen.queryByTestId("experiment-price-0-blocks")).toBeNull();
    await user.type(testid("experiment-price-0-first-switch"), "2099-06-01");
    await user.click(testid("experiment-price-submit"));

    await waitFor(() =>
      expect(state.mutateAsync).toHaveBeenCalledWith({
        candidates: [
          {
            shape: "waves",
            pairs: true,
            wave_sizes: [],
            first_switch_on: "2099-06-01",
            spacing_days: 14,
            alpha: 0.05
          }
        ]
      })
    );
  });

  it("builds several candidates in one request: waves, and a switchback at both alphas", async () => {
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    await fillWaves(user);
    await user.click(testid("experiment-price-add-candidate"));
    await fillSwitchback(user, 1);
    // The switchback row shows its own fields and hides the waves fields.
    expect(screen.queryByTestId("experiment-price-1-wave-sizes")).toBeNull();
    await user.click(testid("experiment-price-1-alpha-both"));
    expect(testid("experiment-price-count")).toHaveTextContent("3 of 6 candidates");
    await user.click(testid("experiment-price-submit"));

    await waitFor(() =>
      expect(state.mutateAsync).toHaveBeenCalledWith({
        candidates: [
          {
            shape: "waves",
            wave_sizes: [3, 3],
            first_switch_on: "2099-06-01",
            spacing_days: 14,
            alpha: 0.05
          },
          {
            shape: "switchback",
            first_period_on: "2099-06-01",
            period_days: 7,
            pairs: 6,
            alpha: 0.05
          },
          {
            shape: "switchback",
            first_period_on: "2099-06-01",
            period_days: 7,
            pairs: 6,
            alpha: 0.2
          }
        ]
      })
    );
  });

  it("sends blocks and a group dimension when they are given", async () => {
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    await fillWaves(user);
    await user.type(testid("experiment-price-0-blocks"), "2");
    await user.type(testid("experiment-price-0-assign-by"), "category");
    await user.click(testid("experiment-price-submit"));

    await waitFor(() =>
      expect(state.mutateAsync).toHaveBeenCalledWith({
        candidates: [expect.objectContaining({ blocks: 2, assign_by: "category" })]
      })
    );
  });

  it("refuses more than six candidates after both-alpha expansion, saying a row at both counts twice", async () => {
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    for (let i = 0; i < 3; i++) await user.click(testid("experiment-price-add-candidate"));
    for (let i = 0; i < 4; i++) await user.click(testid(`experiment-price-${i}-alpha-both`));
    expect(testid("experiment-price-count")).toHaveTextContent("8 of 6 candidates");
    await user.click(testid("experiment-price-submit"));

    expect(await screen.findByTestId("experiment-price-candidates-error")).toHaveTextContent(
      "a row priced at both alphas counts twice"
    );
    expect(state.mutateAsync).not.toHaveBeenCalled();
  });

  it("stops offering another candidate at six rows", async () => {
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    for (let i = 0; i < 5; i++) await user.click(testid("experiment-price-add-candidate"));

    expect(testid("experiment-price-candidate-5")).toBeInTheDocument();
    expect(testid("experiment-price-add-candidate")).toBeDisabled();
  });

  it("removes a candidate, and offers no remove on the only one", async () => {
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    expect(screen.queryByTestId("experiment-price-0-remove")).toBeNull();
    await user.click(testid("experiment-price-add-candidate"));
    await user.click(testid("experiment-price-1-remove"));

    expect(screen.queryByTestId("experiment-price-candidate-1")).toBeNull();
    expect(testid("experiment-price-count")).toHaveTextContent("1 of 6 candidates");
  });
});

describe("PricingPanel outcome", () => {
  it("waits on its own request, counting settled candidates, and shows no older result as the answer", async () => {
    state.proposals = [proposal({ id: "old", request_id: "req-old" })];
    queueAs("req-new", ["n1", "n2"], () => [
      pending("n1", { request_id: "req-new" }),
      proposal({ id: "n2", request_id: "req-new" }),
      proposal({ id: "old", request_id: "req-old" })
    ]);
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    await fillWaves(user);
    await user.click(testid("experiment-price-0-alpha-both"));
    await user.click(testid("experiment-price-submit"));

    expect(await screen.findByTestId("experiment-price-waiting")).toBeInTheDocument();
    expect(testid("experiment-price-progress")).toHaveTextContent("1 of 2 candidates");
    expect(screen.queryByTestId("experiment-price-outcome")).toBeNull();
  });

  it("shows every candidate side by side once the whole request has settled", async () => {
    const user = userEvent.setup();
    queueAs("req-new", ["n1", "n2"], () => [
      pending("n1", { request_id: "req-new" }),
      pending("n2", { request_id: "req-new" })
    ]);
    const { rerender } = render(<PricingPanel summary={summary} />);

    await fillWaves(user);
    await user.click(testid("experiment-price-0-alpha-both"));
    await user.click(testid("experiment-price-submit"));
    await screen.findByTestId("experiment-price-waiting");

    state.proposals = [
      proposal({ id: "n1", request_id: "req-new", alpha: 0.05, mde: 17.5, mde_unit_day: 17.5 }),
      proposal({ id: "n2", request_id: "req-new", alpha: 0.2, mde: 13.3, mde_unit_day: 13.3 })
    ];
    rerender(<PricingPanel summary={summary} />);

    expect(await screen.findByTestId("experiment-price-outcome")).toHaveAttribute(
      "data-outcome",
      "priced"
    );
    expect(testid("experiment-candidate-1")).toHaveAttribute("data-proposal-id", "n2");
    expect(testid("experiment-candidate-2")).toHaveAttribute("data-proposal-id", "n1");
    expect(screen.queryByTestId("experiment-price-waiting")).toBeNull();
  });

  it("shows a refusal and a fault as page content when that is how the request settles", async () => {
    const refusal = "the lever `promo_on` resolves to 0 automations";
    const user = userEvent.setup();
    queueAs("req-new", ["n1", "n2"], () => [
      refused("n1", refusal, { request_id: "req-new" }),
      failed("n2", "the price job was cancelled before it ran", { request_id: "req-new" })
    ]);
    render(<PricingPanel summary={summary} />);

    await fillWaves(user);
    await user.click(testid("experiment-price-0-alpha-both"));
    await user.click(testid("experiment-price-submit"));

    expect(await screen.findByTestId("experiment-price-refusal-1-text")).toHaveTextContent(refusal);
    expect(testid("experiment-price-failure-1-text")).toHaveTextContent("cancelled before it ran");
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("resumes waiting on load when the newest request still has a pending candidate", () => {
    state.proposals = [
      proposal({ id: "a", request_id: "mid" }),
      pending("b", { request_id: "mid" })
    ];
    render(<PricingPanel summary={summary} />);

    expect(testid("experiment-price-waiting")).toBeInTheDocument();
    expect(testid("experiment-price-progress")).toHaveTextContent("1 of 2 candidates");
  });

  it("stops waiting on a reloaded request that never settles, re-enabling the form", () => {
    vi.useFakeTimers();
    try {
      state.proposals = [
        proposal({ id: "a", request_id: "mid" }),
        pending("b", { request_id: "mid" })
      ];
      render(<PricingPanel summary={summary} />);
      expect(testid("experiment-price-waiting")).toBeInTheDocument();
      expect(testid("experiment-price-submit")).toBeDisabled();

      act(() => {
        vi.advanceTimersByTime(PRICING_TIMEOUT_MS + 1_000);
      });

      expect(screen.queryByTestId("experiment-price-waiting")).toBeNull();
      expect(testid("experiment-price-timeout")).toHaveTextContent("taking longer than expected");
      expect(testid("experiment-price-submit")).not.toBeDisabled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("stops its clock once pricing has settled", async () => {
    const user = userEvent.setup();
    const started = vi.spyOn(globalThis, "setInterval");
    const stopped = vi.spyOn(globalThis, "clearInterval");
    try {
      queueAs("req-new", ["n1"], () => [pending("n1", { request_id: "req-new" })]);
      const { rerender } = render(<PricingPanel summary={summary} />);
      await fillWaves(user);
      await user.click(testid("experiment-price-submit"));
      await screen.findByTestId("experiment-price-waiting");

      state.proposals = [proposal({ id: "n1", request_id: "req-new" })];
      rerender(<PricingPanel summary={summary} />);
      await screen.findByTestId("experiment-price-outcome");

      const live = started.mock.results
        .map((r) => r.value)
        .filter((id) => !stopped.mock.calls.some(([cleared]) => cleared === id));
      expect(live).toEqual([]);
    } finally {
      started.mockRestore();
      stopped.mockRestore();
    }
  });

  it("shows the latest settled request on load, so a refresh does not lose the detail", () => {
    state.proposals = [proposal({ id: "earlier" })];
    render(<PricingPanel summary={summary} />);

    expect(testid("experiment-price-outcome")).toHaveAttribute("data-outcome", "priced");
    expect(testid("experiment-candidate-detail")).toHaveAttribute("data-proposal-id", "earlier");
  });
});

describe("PricingPanel proposal count", () => {
  it("never undercounts: the list's total covers proposals beyond the log's page", () => {
    state.proposals = [proposal({ id: "a" })];
    render(<PricingPanel summary={{ ...summary, proposals_count: 240 }} />);

    expect(testid("experiment-proposal-count")).toHaveTextContent("240 assignments proposed");
  });

  it("counts every priced candidate, not requests", () => {
    state.proposals = [
      proposal({ id: "a", request_id: "r1", created_at: "2026-09-01T00:00:00Z" }),
      refused("b", "no", { request_id: "r1", created_at: "2026-09-01T00:00:01Z" }),
      proposal({ id: "c", request_id: "r2", created_at: "2026-09-03T00:00:00Z" }),
      proposal({ id: "d", request_id: "r2", created_at: "2026-09-03T00:00:01Z" })
    ];
    render(<PricingPanel summary={summary} />);

    expect(testid("experiment-proposal-count")).toHaveTextContent("3 assignments proposed so far");
  });

  it("uses the singular for one", () => {
    state.proposals = [proposal({ id: "a" })];
    render(<PricingPanel summary={summary} />);

    expect(testid("experiment-proposal-count")).toHaveTextContent("1 assignment proposed so far");
  });
});

describe("PricingPanel — use this design", () => {
  it("adopts the selected done proposal and says the draft was updated", async () => {
    state.proposals = [proposal({ id: "p1" })];
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    await user.click(testid("experiment-use-design"));

    expect(state.adopt).toHaveBeenCalledWith("p1");
    expect(await screen.findByTestId("experiment-use-design-done")).toHaveTextContent(
      "Draft design updated"
    );
  });

  it("says in plain words what the button saves, and shows no file text", () => {
    state.proposals = [proposal({ id: "p1" })];
    render(<PricingPanel summary={summary} />);

    expect(testid("experiment-candidate-saves")).toHaveTextContent(
      "saves the assignment above, its seed and the promised minimum detectable effect"
    );
    expect(screen.queryByTestId("experiment-snippet")).toBeNull();
    expect(screen.queryByText(/yaml/i)).toBeNull();
    expect(screen.queryByText(/commit the file/i)).toBeNull();
  });

  it("offers no button, only a note, for a proposal priced before the draft changed", () => {
    state.proposals = [proposal({ id: "p1", stale: true })];
    render(<PricingPanel summary={summary} />);

    expect(screen.queryByTestId("experiment-use-design")).toBeNull();
    expect(testid("experiment-use-design-stale")).toHaveTextContent(
      "The draft changed after this was priced. Price it again to use it."
    );
  });

  it("shows the server's refusal when adopting fails", async () => {
    state.proposals = [proposal({ id: "p1" })];
    state.adopt.mockRejectedValue(new Error("the proposal is stale"));
    const user = userEvent.setup();
    render(<PricingPanel summary={summary} />);

    await user.click(testid("experiment-use-design"));

    expect(await screen.findByTestId("experiment-use-design-error")).toHaveTextContent(
      "the proposal is stale"
    );
  });

  it("disables the button for a viewer", () => {
    role.editor = false;
    state.proposals = [proposal({ id: "p1" })];
    render(<PricingPanel summary={summary} />);

    expect(testid("experiment-use-design")).toBeDisabled();
  });
});

describe("PricingPanel at a glance", () => {
  it("leads with a draft badge, the draft design and no latest-MDE yet", () => {
    render(<PricingPanel summary={summary} />);

    expect(testid("experiment-pricing-status")).toHaveTextContent("Draft");
    expect(testid("experiment-pricing-design")).toHaveTextContent("Draft");
    expect(testid("experiment-pricing-latest")).toHaveTextContent("Not priced yet");
  });

  it("shows the latest priced proposal's MDE per unit-day as a tile", () => {
    render(
      <PricingPanel
        summary={{
          ...summary,
          latest_proposal: {
            id: "p",
            request_id: "r",
            status: "done",
            created_at: "2026-09-30T01:00:00Z",
            mde: 17.5,
            mde_unit_day: 17.5,
            refusal: null
          }
        }}
      />
    );

    expect(testid("experiment-pricing-latest")).toHaveTextContent("17.50/unit-day");
  });

  it("keeps the explanation folded but on the page", () => {
    render(<PricingPanel summary={summary} />);

    expect(testid("experiment-pricing-how")).toHaveTextContent("smallest effect it could read");
  });
});
