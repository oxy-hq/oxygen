// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ExperimentListItem, FlipCounts, RunDetail, RunStatus } from "@/types/experiment";
import { definitionFixture } from "../../../fixtures";
import OperationsView from "./index";

const run = vi.hoisted(() => ({
  value: {} as {
    data: RunStatus | undefined;
    isPending: boolean;
    error: Error | null;
    refetch: () => void;
  },
  abort: vi.fn()
}));

vi.mock("@/hooks/api/useExperiments", () => ({
  useExperimentRun: () => run.value,
  useAbortExperiment: () => ({ mutateAsync: run.abort, isPending: false }),
  useExperimentInterim: () => ({ data: undefined, isPending: true, error: null }),
  useRequestInterim: () => ({ mutate: vi.fn(), isPending: false, error: null })
}));

const role = vi.hoisted(() => ({ admin: true, editor: true }));
vi.mock("@/hooks/useRole", () => ({
  useRole: () => ({ is: { workspaceAdmin: role.admin, workspaceEditor: role.editor } })
}));

const summary: ExperimentListItem = {
  name: "beverage_promo_pilot",
  owner: "ops",
  hypothesis: "h",
  design_id: "design-2",
  design_hash: "hash-2",
  design_frozen_at: "2026-02-20T03:15:00Z",
  status: "live",
  registration_refusal: null,
  next_switch_on: "2026-03-16",
  retrospective: false,
  retrospective_ready: false,
  proposals_count: 1,
  latest_proposal: null,
  definition: definitionFixture
};

const detail = (over: Partial<RunDetail> = {}): RunDetail => ({
  state: "live",
  design: "waves",
  periods: null,
  assign_by: null,
  group_membership: null,
  design_id: "design-2",
  registered_by: "user-1",
  registered_at: "2026-02-20T03:15:00Z",
  retrospective: false,
  next_switch_on: "2026-03-16",
  flips: { pending: 1, sent: 4, confirmed: 3, unconfirmed: 1, failed: 1, cancelled: 0 },
  failed_flips: [
    { unit: "bondi", state: "on", reason: "the lever `promo_on` was renamed or deleted" }
  ],
  compliance: {
    c_treated: 0.5,
    c_control_leak: 0.1,
    restated_mde: 30.2,
    below_floor: true,
    n_treated: 10,
    n_control: 10
  },
  coverage: { last_window: 0.96, floor: 0.9 },
  guardrails: [{ criteria: "refund_rate anomaly", status: "ok", detail: null, units: [] }],
  guardrail_breach: null,
  abort_reason: null,
  abort_levers: null,
  aborted_on: null,
  units_on: ["bondi", "newtown"],
  checked_at: "2026-03-10T00:00:00Z",
  problems: [],
  next_check_at: "2026-03-10T01:00:00Z",
  not_advancing: false,
  estimate: { state: "none" },
  ...over
});

const serve = (
  over: Partial<RunDetail> = {},
  extra: Record<string, unknown> = {},
  top: Partial<RunStatus> = {}
) => {
  run.value = {
    data: {
      name: "beverage_promo_pilot",
      status: "live",
      registration_refusal: null,
      retrospective_ready: false,
      retrospective_refusal: null,
      lever_revisions: ["rev-a"],
      run: { ...detail(over), ...extra } as RunDetail,
      ...top
    },
    isPending: false,
    error: null,
    refetch: vi.fn()
  };
};

beforeEach(() => {
  role.admin = true;
  role.editor = true;
  run.abort.mockReset();
  run.abort.mockResolvedValue(undefined);
  serve();
});
afterEach(cleanup);

describe("OperationsView", () => {
  it("shows the pinned revision, next switch, flips, compliance, coverage and guardrails", () => {
    render(<OperationsView summary={summary} />);

    expect(screen.getByTestId("experiment-pinned-revision")).toHaveTextContent("Registered on");
    expect(screen.getByTestId("experiment-pinned-revision")).toHaveTextContent(
      "2026-02-20 03:15 UTC"
    );
    expect(screen.getByTestId("experiment-next-switch")).toHaveTextContent("2026-03-16");
    const flips = screen.getByTestId("experiment-flips");
    expect(flips).toHaveTextContent("Sent 4");
    expect(flips).toHaveTextContent("Confirmed 3");
    expect(flips).toHaveTextContent("Unconfirmed 1");
    expect(flips).toHaveTextContent("Failed 1");
    expect(flips).toHaveTextContent("Pending 1");
    expect(screen.getByTestId("experiment-failed-flips")).toHaveTextContent("bondi");
    expect(screen.getByTestId("experiment-failed-flips")).toHaveTextContent(
      "the lever `promo_on` was renamed or deleted"
    );
    expect(screen.getByTestId("experiment-compliance")).toHaveTextContent("50.0%");
    expect(screen.getByTestId("experiment-compliance")).toHaveTextContent("10.0%");
    expect(screen.getByTestId("experiment-compliance")).toHaveAttribute("data-tone", "warning");
    expect(screen.getByTestId("experiment-compliance")).toHaveTextContent("Below the floor");
    expect(screen.getByTestId("experiment-coverage")).toHaveTextContent("96.0%");
    expect(screen.getByTestId("experiment-guardrails")).toHaveTextContent("refund_rate anomaly");
    expect(screen.getByTestId("experiment-guardrails")).toHaveTextContent("OK");
  });

  it("says leakage was not measured when no control entry was read, not 0.0%", () => {
    serve({
      compliance: {
        c_treated: 0.5,
        c_control_leak: 0,
        restated_mde: null,
        below_floor: false,
        n_treated: 10,
        n_control: 0
      }
    });
    render(<OperationsView summary={summary} />);

    const block = screen.getByTestId("experiment-compliance");
    expect(block).toHaveTextContent("Leaked to control: not measured");
    expect(block.textContent).not.toContain("leaked to control: 0.0%");
  });

  it("says so when no treated flip has been decided, rather than showing 0%", () => {
    serve({ compliance: null });
    render(<OperationsView summary={summary} />);

    expect(screen.getByTestId("experiment-compliance")).toHaveTextContent(
      "No treated flip decided"
    );
    expect(screen.getByTestId("experiment-compliance").textContent).not.toContain("0.0%");
  });

  it("shows a prominent not-advancing banner when the API says so", () => {
    serve({ not_advancing: true });
    render(<OperationsView summary={summary} />);

    const banner = screen.getByTestId("experiment-not-advancing");
    expect(banner).toHaveAttribute("role", "alert");
    expect(banner).toHaveTextContent("Not advancing");
    expect(banner).toHaveTextContent(
      "This experiment's scheduler isn't running — nothing will flip until it is"
    );
  });

  it("shows no banner while the run is advancing", () => {
    render(<OperationsView summary={summary} />);

    expect(screen.queryByTestId("experiment-not-advancing")).toBeNull();
  });

  it("says when it was registered, never frozen, a version or a drift notice", () => {
    render(<OperationsView summary={summary} />);

    expect(screen.getByTestId("experiment-pinned-revision")).toHaveTextContent(
      "Registered on 2026-02-20 03:15 UTC"
    );
    expect(screen.getByTestId("experiment-pinned-revision")).not.toHaveTextContent(
      /version|froze/i
    );
    expect(screen.queryByTestId("experiment-drift")).toBeNull();
  });

  it("says when the lever's definition changed mid-run, and only then", () => {
    const { unmount } = render(<OperationsView summary={summary} />);
    expect(screen.queryByTestId("experiment-lever-changed")).toBeNull();
    unmount();

    serve({}, {}, { lever_revisions: ["rev-a", "rev-b"] });
    render(<OperationsView summary={summary} />);
    expect(screen.getByTestId("experiment-lever-changed")).toHaveTextContent("2 revisions");
  });

  it("shows a sticky guardrail breach and what the last check could not evaluate, verbatim", () => {
    serve({
      guardrail_breach: "refund_rate breached its envelope <b>twice</b>",
      guardrails: [
        { criteria: "refund_rate anomaly", status: "breached", detail: null, units: [] }
      ],
      problems: ["the measure `store_days.labor_cost_pct` is not in the semantic model"]
    });
    const { container } = render(<OperationsView summary={summary} />);

    expect(screen.getByTestId("experiment-guardrail-breach").textContent).toContain(
      "refund_rate breached its envelope <b>twice</b>"
    );
    expect(container.querySelector("b")).toBeNull();
    expect(screen.getByTestId("experiment-problems")).toHaveTextContent(
      "the measure `store_days.labor_cost_pct` is not in the semantic model"
    );
  });

  it("never renders an effect number, even if the payload carries one", () => {
    serve(
      {},
      {
        estimate: 41.7,
        effect: { estimate: 12.3, interval: { low: 1.11, high: 22.2 } },
        net_effect: 9.99,
        headline: "gross +$77.77/unit-day"
      }
    );
    const { container } = render(<OperationsView summary={summary} />);

    const text = container.textContent ?? "";
    for (const leaked of ["41.7", "12.3", "1.11", "22.2", "9.99", "77.77"]) {
      expect(text).not.toContain(leaked);
    }
    // The operations view has no $/unit-day figure of any kind — not even the restated MDE.
    expect(text).not.toContain("unit-day");
    expect(text).not.toContain("30.2");
    expect(
      screen.getByText(/registered result is sealed until the run reports/i)
    ).toBeInTheDocument();
  });

  it("does not render a failed read as an empty run", () => {
    run.value = {
      data: undefined,
      isPending: false,
      error: new Error("run not found"),
      refetch: vi.fn()
    };
    render(<OperationsView summary={summary} />);

    expect(screen.getByText("run not found")).toBeInTheDocument();
    expect(screen.queryByTestId("experiment-operations")).toBeNull();
  });
});

describe("Abort", () => {
  const openDialog = async (user: ReturnType<typeof userEvent.setup>) => {
    render(<OperationsView summary={summary} />);
    await user.click(screen.getByTestId("experiment-abort-button"));
    return screen.findByTestId("experiment-abort-dialog");
  };

  it("is offered while registered or live, and not once closing (the server answers 409)", () => {
    serve({ state: "closing" });
    render(<OperationsView summary={summary} />);

    expect(screen.queryByTestId("experiment-abort-button")).toBeNull();
    expect(screen.getByText(/can no longer be aborted/i)).toBeInTheDocument();
  });

  it("says a retrospective run moves no levers, instead of flip progress", () => {
    serve({ state: "closing", retrospective: true });
    render(<OperationsView summary={{ ...summary, retrospective: true }} />);

    expect(screen.getByTestId("experiment-retrospective-note")).toHaveTextContent(
      "Analysing past data — no levers are switched. The report is being prepared."
    );
    expect(screen.queryByTestId("experiment-abort-button")).toBeNull();
    expect(screen.queryByText(/can no longer be aborted/i)).toBeNull();
  });

  it("is not offered to a user below workspace admin, who is told why", () => {
    role.admin = false;
    serve({ state: "live" });
    render(<OperationsView summary={summary} />);

    expect(screen.queryByTestId("experiment-abort-button")).toBeNull();
    expect(screen.getByTestId("experiment-abort-denied")).toHaveTextContent(
      "Only a workspace owner or admin can abort"
    );
  });

  it("shows an accepted abort with its reason verbatim and no longer offers the button", () => {
    serve({ state: "live", abort_reason: "<b>refunds</b> spiked" });
    render(<OperationsView summary={summary} />);

    const notice = screen.getByTestId("experiment-abort-requested");
    expect(notice).toHaveTextContent("Abort requested");
    expect(notice).toHaveTextContent("<b>refunds</b> spiked");
    expect(notice.querySelector("b")).toBeNull();
    expect(screen.queryByTestId("experiment-abort-button")).toBeNull();
  });

  it("cannot be confirmed without a reason, and whitespace is not a reason", async () => {
    const user = userEvent.setup();
    await openDialog(user);

    expect(screen.getByTestId("experiment-abort-confirm")).toBeDisabled();
    await user.type(screen.getByTestId("experiment-abort-reason"), "   ");
    expect(screen.getByTestId("experiment-abort-confirm")).toBeDisabled();
  });

  it("sends the trimmed reason and closes on success", async () => {
    const user = userEvent.setup();
    await openDialog(user);

    await user.type(screen.getByTestId("experiment-abort-reason"), "  refunds spiked in wave 1 ");
    await user.click(screen.getByTestId("experiment-abort-confirm"));

    await waitFor(() =>
      expect(run.abort).toHaveBeenCalledWith({ reason: "refunds spiked in wave 1", levers: "off" })
    );
    await waitFor(() => expect(screen.queryByTestId("experiment-abort-dialog")).toBeNull());
  });

  it("says plainly what aborting does: which levers go off, who is told, the report, no undo", async () => {
    const user = userEvent.setup();
    const dialog = await openDialog(user);

    const effects = within(dialog).getByTestId("experiment-abort-effects");
    expect(effects).toHaveTextContent(
      "Turns the lever off at 2 units that are on now: bondi, newtown"
    );
    expect(effects).toHaveTextContent("Notifies ops");
    expect(effects).toHaveTextContent("The report is labelled Aborted and uses data up to today");
    expect(effects).toHaveTextContent("This can't be undone");
    expect(within(dialog).getByTestId("experiment-abort-levers-off")).toHaveAttribute(
      "data-state",
      "on"
    );
    expect(within(dialog).queryByTestId("experiment-abort-keep-warning")).toBeNull();
  });

  it("says nothing is on to turn off when no lever is on yet", async () => {
    serve({ state: "registered", units_on: [] });
    const user = userEvent.setup();
    const dialog = await openDialog(user);

    expect(within(dialog).getByTestId("experiment-abort-effects")).toHaveTextContent(
      "No lever is on yet, so none is switched"
    );
  });

  it("can keep the levers as they are, warns the experiment stops managing them, and sends that", async () => {
    const user = userEvent.setup();
    const dialog = await openDialog(user);

    await user.click(within(dialog).getByTestId("experiment-abort-levers-keep"));
    expect(within(dialog).getByTestId("experiment-abort-keep-warning")).toHaveTextContent(
      "The experiment will no longer manage bondi, newtown"
    );
    expect(within(dialog).getByTestId("experiment-abort-effects")).toHaveTextContent(
      "Leaves every lever as it is"
    );
    await user.type(screen.getByTestId("experiment-abort-reason"), "keep the promo running");
    await user.click(screen.getByTestId("experiment-abort-confirm"));

    await waitFor(() =>
      expect(run.abort).toHaveBeenCalledWith({ reason: "keep the promo running", levers: "keep" })
    );
  });

  it("shows that an accepted abort will leave the levers as they are", () => {
    serve({ state: "live", abort_reason: "owner call", abort_levers: "keep" });
    render(<OperationsView summary={summary} />);

    expect(screen.getByTestId("experiment-abort-requested")).toHaveTextContent(
      "Levers will be left as they are"
    );
  });

  it("keeps the dialog open and shows the server's message inline when the abort fails", async () => {
    run.abort.mockRejectedValue(new Error("run is closing and can no longer be aborted"));
    const user = userEvent.setup();
    await openDialog(user);

    await user.type(screen.getByTestId("experiment-abort-reason"), "manual stop");
    fireEvent.click(screen.getByTestId("experiment-abort-confirm"));

    expect(await screen.findByTestId("experiment-abort-error")).toHaveTextContent(
      "run is closing and can no longer be aborted"
    );
    expect(screen.getByTestId("experiment-abort-dialog")).toBeInTheDocument();
  });
});

const flipCounts = (over: Partial<FlipCounts> = {}): FlipCounts => ({
  pending: 0,
  sent: 0,
  confirmed: 0,
  unconfirmed: 0,
  failed: 0,
  cancelled: 0,
  ...over
});

describe("OperationsView — a switchback's schedule", () => {
  const periods = [
    {
      index: 0,
      from: "2026-03-02",
      state: "on" as const,
      flips: flipCounts({ confirmed: 6, sent: 6 })
    },
    {
      index: 1,
      from: "2026-03-09",
      state: "off" as const,
      flips: flipCounts({ confirmed: 4, sent: 6, failed: 1 })
    },
    {
      index: 2,
      from: "2026-03-16",
      state: "off" as const,
      flips: flipCounts({ confirmed: 3, sent: 6 })
    },
    { index: 3, from: "2026-03-23", state: "on" as const, flips: flipCounts() }
  ];

  it("shows each period in its pair with its state and how far its flips have got", () => {
    serve({ design: "switchback", periods });
    render(<OperationsView summary={summary} />);

    const rows = [1, 2, 3, 4].map((n) => screen.getByTestId(`experiment-period-${n}`));
    expect(rows.map((r) => r.getAttribute("data-pair"))).toEqual(["1", "1", "2", "2"]);
    expect(rows.map((r) => r.getAttribute("data-state"))).toEqual(["on", "off", "off", "on"]);
    expect(rows.map((r) => r.getAttribute("data-status"))).toEqual([
      "landed",
      "needs_attention",
      "in_progress",
      "upcoming"
    ]);
    expect(rows[0]).toHaveTextContent("6 of 6 confirmed");
    expect(rows[1]).toHaveTextContent("4 of 6 confirmed, 1 failed");
    expect(rows[3]).toHaveTextContent("—");
  });

  it("shows no schedule for a waves design", () => {
    render(<OperationsView summary={summary} />);

    expect(screen.queryByTestId("experiment-switchback")).toBeNull();
  });
});

describe("OperationsView — a grouped design's membership", () => {
  it("lists the pinned groups by name with member counts, and says what pinning means", () => {
    serve({
      assign_by: "category",
      group_membership: { Snacks: ["d", "e"], Beverages: ["a", "b", "c"] }
    });
    render(<OperationsView summary={summary} />);

    const groups = within(screen.getByTestId("experiment-groups"));
    expect(screen.getByTestId("experiment-groups-total")).toHaveTextContent("2 groups");
    expect(screen.getByTestId("experiment-groups-total")).toHaveTextContent("5 members");
    expect(screen.getByTestId("experiment-groups")).toHaveTextContent("neither arm");
    const rows = groups.getAllByRole("row", { hidden: true }).map((r) => r.textContent ?? "");
    expect(rows[1]).toContain("Beverages");
    expect(rows[1]).toContain("3");
    expect(rows[2]).toContain("Snacks");
    expect(screen.getByTestId("experiment-group-Beverages")).toHaveTextContent("a, b, c");
  });

  it("says membership is not pinned yet rather than showing an empty list", () => {
    serve({ assign_by: "category", group_membership: null });
    render(<OperationsView summary={summary} />);

    expect(screen.getByTestId("experiment-groups-unpinned")).toHaveTextContent("has not run yet");
    expect(screen.queryByTestId("experiment-groups-total")).toBeNull();
  });

  it("shows no groups section when the design assigns single units", () => {
    render(<OperationsView summary={summary} />);

    expect(screen.queryByTestId("experiment-groups")).toBeNull();
  });
});

describe("OperationsView — guardrails per unit", () => {
  const breached = {
    criteria: "refund_rate anomaly",
    status: "breached" as const,
    detail: null,
    units: [
      { unit: "bondi", status: "breached" as const, detail: "refunds 3.1x its own band" },
      { unit: "manly", status: "ok" as const, detail: null },
      { unit: "coogee", status: "ok" as const, detail: null }
    ]
  };

  it("names the unit that breached and counts the rest rather than listing them", () => {
    serve({ guardrails: [breached] });
    render(<OperationsView summary={summary} />);

    const unit = screen.getByTestId("experiment-guardrail-breached-unit");
    expect(unit).toHaveTextContent("bondi");
    expect(unit).toHaveTextContent("refunds 3.1x its own band");
    expect(screen.getByTestId("experiment-guardrail-units-ok")).toHaveTextContent(
      "2 of 3 units OK"
    );
    expect(screen.queryByText("manly")).toBeNull();
    const row = screen.getByTestId("experiment-check-guardrail-0");
    expect(row).toHaveAttribute("data-tone", "danger");
    expect(row).toHaveTextContent("Breached");
  });

  it("says which units a guardrail could not be evaluated for", () => {
    serve({
      guardrails: [
        {
          criteria: "labor_cost_pct trigger",
          status: "refused",
          detail: null,
          units: [{ unit: "bondi", status: "refused", detail: "no rows for this unit yet" }]
        }
      ]
    });
    render(<OperationsView summary={summary} />);

    expect(screen.getByTestId("experiment-guardrails")).toHaveTextContent("bondi");
    expect(screen.getByTestId("experiment-guardrails")).toHaveTextContent("could not be evaluated");
  });

  it.each([
    [{}, "per treated unit"],
    [{ assign_by: "category" }, "per group"],
    [{ design: "switchback" as const, periods: [] }, "every unit during the on periods"]
  ])("states what a guardrail is checked against (%j)", (over, words) => {
    serve(over);
    render(<OperationsView summary={summary} />);

    expect(screen.getByTestId("experiment-guardrail-scope")).toHaveTextContent(words);
    expect(screen.getByTestId("experiment-guardrail-scope")).toHaveTextContent("names the unit");
  });

  it("still shows no effect number beside the schedule, the groups and the units", () => {
    serve(
      {
        design: "switchback",
        periods: [
          { index: 0, from: "2026-03-02", state: "on", flips: flipCounts({ confirmed: 2 }) }
        ],
        assign_by: "category",
        group_membership: { Beverages: ["a"] },
        guardrails: [breached]
      },
      { estimate: 41.7, effect: { estimate: 12.3 } }
    );
    const { container } = render(<OperationsView summary={summary} />);

    const text = container.textContent ?? "";
    for (const leaked of ["41.7", "12.3"]) expect(text).not.toContain(leaked);
    expect(text).not.toContain("unit-day");
  });
});

describe("OperationsView — the estimate's standing", () => {
  it("shows the estimate state from the run", () => {
    serve({ estimate: { state: "running" } });
    render(<OperationsView summary={summary} />);

    expect(screen.getByTestId("experiment-estimate")).toHaveTextContent("Estimate running");
    expect(screen.getByTestId("experiment-estimate-state")).toHaveAttribute("data-tone", "info");
  });

  it("shows nothing for an estimate that does not exist yet", () => {
    render(<OperationsView summary={summary} />);

    expect(screen.queryByTestId("experiment-estimate")).toBeNull();
  });
});

describe("OperationsView — at a glance", () => {
  it("leads with the run state as a coloured badge and the key numbers as tiles", () => {
    render(<OperationsView summary={summary} />);

    expect(screen.getByTestId("experiment-run-state")).toHaveAttribute("data-tone", "success");
    expect(screen.getByTestId("experiment-run-state")).toHaveTextContent("Live");
    const flips = screen.getByTestId("experiment-stat-flips");
    expect(flips).toHaveTextContent("3 of 6");
    expect(flips).toHaveAttribute("data-tone", "danger");
    expect(screen.getByTestId("experiment-coverage")).toHaveAttribute("data-tone", "success");
    expect(screen.getByTestId("experiment-stat-next-check")).toBeInTheDocument();
    expect(screen.getByTestId("experiment-stat-guardrails")).toHaveAttribute(
      "data-tone",
      "success"
    );
  });

  it.each([
    ["registered", "info"],
    ["closing", "warning"]
  ] as const)("tones a %s run %s", (state, tone) => {
    serve({ state });
    render(<OperationsView summary={summary} />);
    expect(screen.getByTestId("experiment-run-state")).toHaveAttribute("data-tone", tone);
  });

  it.each([
    ["ok", "success"],
    ["breached", "danger"],
    ["not_checked", "neutral"],
    ["refused", "warning"]
  ] as const)("tones a %s guardrail %s", (status, tone) => {
    serve({ guardrails: [{ criteria: "c", status, detail: null, units: [] }] });
    render(<OperationsView summary={summary} />);
    expect(screen.getByTestId("experiment-check-guardrail-0")).toHaveAttribute("data-tone", tone);
  });

  it("headlines the guardrails that passed when the rest are not checked yet", () => {
    serve({
      guardrails: [
        { criteria: "a", status: "ok", detail: null, units: [] },
        { criteria: "b", status: "not_checked", detail: null, units: [] }
      ]
    });
    render(<OperationsView summary={summary} />);
    const tile = screen.getByTestId("experiment-stat-guardrails");
    expect(tile).toHaveTextContent("1 OK");
    expect(tile).toHaveTextContent("1 not checked yet");
  });

  it("does not colour an empty landed count green, and counts one period in the singular", () => {
    serve({
      design: "switchback",
      periods: [{ index: 0, from: "2099-03-02", state: "on", flips: flipCounts() }]
    });
    render(<OperationsView summary={summary} />);
    const row = screen.getByTestId("experiment-switchback-summary");
    expect(row).toHaveTextContent("1 period");
    expect(row).not.toHaveTextContent("1 periods");
    expect(row.querySelector("[data-tone=success]")).toBeNull();
  });

  it("shows failed flips and problems as danger and warning rows, verbatim", () => {
    serve({ problems: ["measure `x` is missing"] });
    render(<OperationsView summary={summary} />);
    expect(screen.getByTestId("experiment-failed-flips")).toBeInTheDocument();
    expect(
      within(screen.getByTestId("experiment-failed-flips")).getByTestId(
        "experiment-check-failed-flip-0"
      )
    ).toHaveAttribute("data-tone", "danger");
    expect(screen.getByTestId("experiment-check-problem-0")).toHaveAttribute(
      "data-tone",
      "warning"
    );
    expect(screen.getByTestId("experiment-check-problem-0")).toHaveTextContent(
      "measure `x` is missing"
    );
  });

  it("keeps the switchback table and the group table folded, with a one-line summary visible", () => {
    serve({
      design: "switchback",
      periods: [
        { index: 0, from: "2026-03-02", state: "on", flips: flipCounts({ confirmed: 6, sent: 6 }) },
        { index: 1, from: "2026-03-09", state: "off", flips: flipCounts({ failed: 1, sent: 6 }) }
      ],
      assign_by: "category",
      group_membership: { A: ["a"] }
    });
    render(<OperationsView summary={summary} />);

    expect(screen.getByTestId("experiment-switchback-summary")).toHaveTextContent("1 landed");
    expect(screen.getByTestId("experiment-switchback-summary")).toHaveTextContent(
      "1 need attention"
    );
    expect(screen.getByTestId("experiment-period-1").closest("[data-state=closed]")).not.toBeNull();
    expect(screen.getByTestId("experiment-group-A").closest("[data-state=closed]")).not.toBeNull();
  });
});
