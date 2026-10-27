// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { toast } from "sonner";
import { afterEach, describe, expect, it, vi } from "vitest";
import { failed, grouped, proposal, refused, switchback } from "../fixtures";
import PriceOutcome from "./PriceOutcome";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

afterEach(cleanup);

describe("PriceOutcome — side by side", () => {
  it("ranks the grouped candidate first and shows both MDE scales", () => {
    const unit = proposal({ id: "unit", mde: 17.5, mde_unit_day: 17.5 });
    // The biggest raw number, the smallest per unit-day.
    const group = grouped("group", { mde: 1200, mde_unit_day: 9 });
    render(<PriceOutcome proposals={[unit, group]} />);

    expect(screen.getByTestId("experiment-candidate-1")).toHaveAttribute(
      "data-proposal-id",
      "group"
    );
    expect(screen.getByTestId("experiment-candidate-2")).toHaveAttribute(
      "data-proposal-id",
      "unit"
    );
    // The design's own unit, labelled — and the comparable figure beside it.
    expect(screen.getByTestId("experiment-candidate-mde-1")).toHaveTextContent(
      "$1,200.00/group-day"
    );
    expect(screen.getByTestId("experiment-candidate-mde-unit-day-1")).toHaveTextContent(
      "$9.00/unit-day"
    );
    expect(screen.getByTestId("experiment-candidate-mde-2")).toHaveTextContent("$17.50/unit-day");
  });

  it("states the group-size caveat only when a group design is among the candidates", () => {
    const { rerender } = render(<PriceOutcome proposals={[proposal({ id: "unit" })]} />);
    expect(screen.queryByTestId("experiment-candidate-group-caveat")).toBeNull();

    rerender(<PriceOutcome proposals={[proposal({ id: "unit" }), grouped("g")]} />);
    expect(screen.getByTestId("experiment-candidate-group-caveat")).toHaveTextContent(
      "spreads evenly over a group's members"
    );
  });

  it("shows the top-ranked candidate's detail first, and another's when it is selected", () => {
    const unit = proposal({ id: "unit", mde_unit_day: 17.5, snippet: "unit snippet seed: 1" });
    const group = grouped("group", { mde_unit_day: 9, snippet: "group snippet seed: 2" });
    render(<PriceOutcome proposals={[unit, group]} />);

    expect(screen.getByTestId("experiment-candidate-detail")).toHaveAttribute(
      "data-proposal-id",
      "group"
    );

    fireEvent.click(screen.getByTestId("experiment-candidate-select-2"));

    expect(screen.getByTestId("experiment-candidate-detail")).toHaveAttribute(
      "data-proposal-id",
      "unit"
    );
    expect(screen.getByText(/into the draft design/i)).toBeInTheDocument();
    expect(screen.queryByText(/snippet seed/)).toBeNull();
  });

  it("shows each candidate's own α, so the same design at 0.05 and 0.2 can be compared", () => {
    const strict = proposal({ id: "strict", alpha: 0.05, mde: 17.5, mde_unit_day: 17.5 });
    const loose = proposal({ id: "loose", alpha: 0.2, mde: 13.3, mde_unit_day: 13.3 });
    render(<PriceOutcome proposals={[strict, loose]} />);

    const first = within(screen.getByTestId("experiment-candidate-1"));
    expect(first.getByText("0.2")).toBeInTheDocument();
    expect(screen.getByTestId("experiment-candidate-mde-unit-day-1")).toHaveTextContent("$13.30");
    expect(
      within(screen.getByTestId("experiment-candidate-2")).getByText("0.05")
    ).toBeInTheDocument();
  });

  it("shows a switchback's schedule in pairs, not waves", () => {
    render(<PriceOutcome proposals={[switchback("sw")]} />);

    const periods = screen.getByTestId("experiment-candidate-periods");
    expect(periods).toHaveTextContent("Pair 1 · 2099-06-01 · On");
    expect(periods).toHaveTextContent("Pair 1 · 2099-06-08 · Off");
    expect(periods).toHaveTextContent("Pair 2 · 2099-06-22 · On");
    expect(screen.queryByTestId("experiment-candidate-waves")).toBeNull();
  });

  it("counts groups, not units, and names the dimension, for a group design", () => {
    render(<PriceOutcome proposals={[grouped("g")]} />);

    expect(screen.getByTestId("experiment-candidate-waves")).toHaveTextContent("2 groups");
    expect(screen.getByTestId("experiment-candidate-detail")).toHaveTextContent("category");
  });

  it("says how a blocked draw was blocked", () => {
    render(
      <PriceOutcome
        proposals={[
          proposal({
            id: "b",
            strata: [
              ["a", "b", "c"],
              ["d", "e", "f"]
            ]
          })
        ]}
      />
    );

    expect(screen.getByTestId("experiment-candidate-strata")).toHaveTextContent("2 blocks");
  });

  it("shows the selected candidate's warnings and its frozen World Model slope", () => {
    render(
      <PriceOutcome
        proposals={[
          proposal({
            id: "p",
            warnings: ["A real lever with washout_days: 0 lands a partial first day."],
            prior_coefficient: { value: 1.42, source: "fitted" }
          })
        ]}
      />
    );

    expect(screen.getByTestId("experiment-price-warnings")).toHaveTextContent("partial first day");
    expect(screen.getByTestId("experiment-candidate-prior")).toHaveTextContent("1.42");
    expect(screen.getByTestId("experiment-candidate-prior")).toHaveTextContent(
      "fitted from history"
    );
  });

  it("shows the MDE in $/unit-day and relative, and the waves, for a plain design", () => {
    render(<PriceOutcome proposals={[proposal({ id: "p" })]} />);

    expect(screen.getByTestId("experiment-candidate-mde")).toHaveTextContent("$17.50/unit-day");
    expect(screen.getByTestId("experiment-candidate-mde-relative")).toHaveTextContent("5.2%");
    expect(screen.getByTestId("experiment-candidate-waves")).toHaveTextContent("2099-06-01");
    expect(screen.getByTestId("experiment-candidate-waves")).toHaveTextContent("3 units");
    expect(screen.getByTestId("experiment-price-outcome")).toHaveAttribute(
      "data-outcome",
      "priced"
    );
  });
});

describe("PriceOutcome — refusals and faults", () => {
  it("renders a refusal verbatim as primary content beside the priced candidates, never as markdown or a toast", () => {
    const refusal =
      "unit `bondi` not found — did you mean <b>Bondi</b>? 2 * 3 {{store_days.promo}} & more";
    const { container } = render(
      <PriceOutcome proposals={[proposal({ id: "ok" }), refused("r", refusal, { alpha: 0.2 })]} />
    );

    expect(screen.getByTestId("experiment-price-refusal-1-text").textContent).toBe(refusal);
    // Text, not markup: the `<b>` is a literal string, not an element.
    expect(container.querySelector("b")).toBeNull();
    // The priced candidate is still there.
    expect(screen.getByTestId("experiment-price-comparison")).toBeInTheDocument();
    expect(screen.getByTestId("experiment-price-outcome")).toHaveAttribute("data-refused", "1");
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("names which candidate each refusal belongs to", () => {
    render(
      <PriceOutcome
        proposals={[refused("r", "no history", { request: switchback("x").request })]}
      />
    );

    expect(screen.getByTestId("experiment-price-refusal-1")).toHaveTextContent(
      "Switchback 2 pairs × 7 days from 2099-06-01"
    );
  });

  it("shows a faulted job's reason as page content, distinct from a refusal", () => {
    render(<PriceOutcome proposals={[failed("f", "the price job was cancelled before it ran")]} />);

    expect(screen.getByTestId("experiment-price-failures")).toBeInTheDocument();
    expect(screen.getByTestId("experiment-price-failure-1-text").textContent).toBe(
      "the price job was cancelled before it ran"
    );
    expect(screen.getByTestId("experiment-price-outcome")).toHaveAttribute(
      "data-outcome",
      "failed"
    );
    expect(screen.queryByTestId("experiment-price-refusals")).toBeNull();
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("shows no table and no candidate detail when every candidate was refused", () => {
    render(<PriceOutcome proposals={[refused("a", "one"), refused("b", "two")]} />);

    expect(screen.getByTestId("experiment-price-outcome")).toHaveAttribute(
      "data-outcome",
      "refused"
    );
    expect(screen.queryByTestId("experiment-price-comparison")).toBeNull();
    expect(screen.queryByTestId("experiment-candidate-detail")).toBeNull();
    expect(screen.getByTestId("experiment-price-refusal-2-text").textContent).toBe("two");
  });
});

describe("PriceOutcome — at a glance", () => {
  it("marks the best-ranked candidate and shows a status strip of counts", () => {
    render(
      <PriceOutcome
        proposals={[
          proposal({ id: "a", mde_unit_day: 17.5 }),
          proposal({ id: "b", mde_unit_day: 9 }),
          refused("r", "no history")
        ]}
      />
    );

    expect(screen.getByTestId("experiment-price-summary")).toHaveTextContent("2 priced");
    expect(screen.getByTestId("experiment-price-summary")).toHaveTextContent("1 refused");
    expect(
      within(screen.getByTestId("experiment-candidate-1")).getByTestId("experiment-candidate-best")
    ).toBeInTheDocument();
    expect(
      within(screen.getByTestId("experiment-candidate-2")).queryByTestId(
        "experiment-candidate-best"
      )
    ).toBeNull();
  });

  it("labels each card's unit so per group-day is never read as per unit-day", () => {
    render(
      <PriceOutcome
        proposals={[grouped("g", { mde: 1200, mde_unit_day: 9 }), proposal({ id: "u" })]}
      />
    );

    expect(screen.getByTestId("experiment-candidate-unit-1")).toHaveTextContent("per group-day");
    expect(screen.getByTestId("experiment-candidate-unit-2")).toHaveTextContent("per unit-day");
  });

  it("shows a warning count badge on the card", () => {
    render(<PriceOutcome proposals={[proposal({ id: "w", warnings: ["one", "two"] })]} />);

    expect(screen.getByTestId("experiment-candidate-warning-count-1")).toHaveTextContent(
      "2 warnings"
    );
  });
});
