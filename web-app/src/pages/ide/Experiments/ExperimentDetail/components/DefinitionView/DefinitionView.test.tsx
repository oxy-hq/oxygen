// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import type { ExperimentListItem, ExperimentStatus } from "@/types/experiment";
import { definitionFixture } from "../../../fixtures";
import DefinitionView from "./index";

afterEach(cleanup);

const summary = (status: string, definition: unknown = definitionFixture) =>
  ({ name: "e", status: status as ExperimentStatus, definition }) as ExperimentListItem;

const view = (definition: unknown = definitionFixture) => {
  render(<DefinitionView summary={summary("live", definition)} />);
  return screen.getByTestId("experiment-definition");
};

describe("DefinitionView", () => {
  it("keeps the lever's run and close behaviour on hover, not as a standing sentence", async () => {
    const user = userEvent.setup();
    view();
    const lever = screen.getByTestId("definition-lever");
    expect(lever.textContent).not.toContain("Turned off when the experiment closes");
    await user.hover(within(lever).getByTestId("definition-lever-hint"));
    expect(await screen.findByRole("tooltip")).toHaveTextContent(
      "Turned off when the experiment closes"
    );
  });

  it("reads as sentences, not yaml", () => {
    const t = view().textContent ?? "";
    for (const s of [
      "A customer-level promo lifts order value per customer-day",
      "Randomises each customer",
      "ops",
      "Total order value",
      "orders.total_order_value",
      "expects an increase",
      "α 0.05",
      "80%",
      "$1.25/unit-day",
      "No driver set",
      "fruit_summary",
      "within 2 days",
      "{{orders.total_order_value}} > 0",
      "UTC",
      "orders.order_date",
      "Assignment seed 42",
      "56 days before and 56 days after each switch",
      "3-day anticipation buffer",
      "7-day washout",
      "90%",
      "80%",
      "orders.total_orders",
      "Aborts the run if Refund rate looks anomalous"
    ]) {
      expect(t).toContain(s);
    }
    for (const raw of ["for_each", "read_back", "anticipation_days", "when_on", "Guardrails —"]) {
      expect(t).not.toContain(raw);
    }
  });

  it("says how many days it keeps reading after the lever turns off", () => {
    const t = view({
      ...definitionFixture,
      design: { ...definitionFixture.design, carryover_days: 14 }
    }).textContent;
    expect(t).toContain("Keeps reading 14 days after the lever turns off");
  });

  it("says in words which rows a filtered outcome and sibling read", () => {
    const t = view({
      ...definitionFixture,
      outcome: {
        ...definitionFixture.outcome,
        target_filter: [{ dimension: "orders.sku", op: "equals", values: ["TB-40"] }]
      },
      substitution_check: {
        sibling: "orders.total_orders",
        filter: [{ dimension: "orders.sku", op: "not_equals", values: ["TB-40"] }]
      }
    }).textContent;
    expect(t).toContain("only rows where sku is TB-40");
    expect(t).toContain("only rows where sku is not TB-40");
    expect(t).not.toContain("target_filter");
  });

  it("lists waves in a table with their units", () => {
    view();
    const row = screen.getByRole("row", { name: /2026-03-16/ });
    expect(row.textContent).toContain("2 customers");
    expect(row.textContent).toContain("alice");
  });

  it("says so in words when there are no guardrails or substitution check", () => {
    const t = view({ ...definitionFixture, guardrails: [], substitution_check: null }).textContent;
    expect(t).toContain("No guardrails registered.");
    expect(t).toContain("No substitution check.");
    expect(t).not.toContain("—");
  });

  it("describes a switchback and an unpriced draft", () => {
    const sb = {
      ...definitionFixture,
      design: {
        ...definitionFixture.design,
        waves: [],
        pre_days: null,
        post_days: null,
        seed: null,
        switchback: {
          period_days: 7,
          periods: [
            { from: "2026-04-06", state: "on" },
            { from: "2026-04-13", state: "off" }
          ]
        }
      }
    };
    const t = view(sb).textContent ?? "";
    expect(t).toContain("7-day periods");
    expect(t).toContain("No seed yet");
    cleanup();
    const draft = { ...sb, design: { ...sb.design, switchback: null } };
    expect(view(draft).textContent).toContain("Not priced yet");
  });

  it("shows unknown keys with humanized labels, as text", () => {
    const el = view({ ...definitionFixture, vendor_note: "<b>kept</b>" });
    expect(within(el).getByTestId("definition-other").textContent).toContain("Vendor note");
    expect(el.textContent).toContain("<b>kept</b>");
    expect(within(el).queryByText("kept")).toBeNull();
    expect(el.textContent).not.toContain("vendor_note");
  });

  it("omits Other settings without unknown keys", () => {
    view();
    expect(screen.queryByTestId("definition-other")).toBeNull();
  });

  it("mentions the family and prior when set", () => {
    const t = view({
      ...definitionFixture,
      outcome: {
        ...definitionFixture.outcome,
        family: 3,
        driver: "orders.promo_redemptions",
        alpha: 0.2,
        prior_coefficient: { value: 4.2, source: "fitted" }
      }
    }).textContent;
    expect(t).toContain("Directional");
    expect(t).toContain("3 outcomes");
    expect(t).toContain("orders.promo_redemptions");
    expect(t).toContain("4.2");
    expect(t).toContain("fitted");
  });

  it("reads the promised effect per unit of the design's grain", () => {
    const fx = definitionFixture as { design: Record<string, unknown> };
    view({ ...fx, design: { ...fx.design, grain: "week" } });
    expect(screen.getByTestId("definition-stat-mde").textContent).toContain("$1.25/unit-week");
  });

  it("leads with stat tiles for the design's key facts", () => {
    view();
    const tile = (id: string) => screen.getByTestId(`definition-stat-${id}`).textContent ?? "";
    expect(tile("target")).toContain("Total order value");
    expect(tile("direction")).toContain("Increase");
    expect(tile("confidence")).toContain("95%");
    expect(tile("confidence")).toContain("α 0.05");
    expect(tile("power")).toContain("80%");
    expect(tile("mde")).toContain("$1.25/unit-day");
    expect(tile("schedule")).toContain("2");
    expect(tile("window")).toContain("56");
    expect(tile("guardrails")).toContain("1");
  });

  it("is open for a draft and folded, but still mounted, for a live experiment", () => {
    const { unmount } = render(<DefinitionView summary={summary("draft")} />);
    expect(screen.getByRole("button", { name: /definition/i })).toHaveAttribute(
      "aria-expanded",
      "true"
    );
    unmount();
    render(<DefinitionView summary={summary("live")} />);
    expect(screen.getByRole("button", { name: /definition/i })).toHaveAttribute(
      "aria-expanded",
      "false"
    );
    expect(screen.getByTestId("experiment-definition")).toHaveTextContent("orders.refund_rate");
  });

  it("keeps long parts mounted: read-back, wave units, quality floors", () => {
    const t = view().textContent ?? "";
    expect(t).toContain("{{orders.total_order_value}} >= 0");
    expect(t).toContain("carol");
    expect(t).toContain("otherwise the result is refused");
  });

  it("renders a definition from an older build that lacks newer keys", () => {
    const old = JSON.parse(JSON.stringify(definitionFixture));
    delete old.guardrails;
    delete old.design.switchback;
    delete old.design.waves;
    delete old.outcome.alpha;
    delete old.substitution_check;
    const el = view(old);
    expect(el.textContent).toContain("—");
    expect(el.textContent).toContain("orders.total_order_value");
    expect(screen.getByTestId("definition-stat-confidence")).toHaveTextContent("—");
    expect(screen.getByTestId("definition-stat-guardrails")).toHaveTextContent("0");
    cleanup();
    expect(view({}).textContent).toContain("—");
    cleanup();
    expect(view({ design: null, outcome: null, lever: null }).textContent).toContain("—");
  });

  it("mounts without a definition", () => {
    expect(view(null)).toBeTruthy();
  });
});
