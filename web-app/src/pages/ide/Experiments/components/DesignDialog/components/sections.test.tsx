// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type DesignFormValues, emptyDesignValues } from "../designForm";
import AboutSection from "./AboutSection";
import GuardrailsSection from "./GuardrailsSection";
import LeverSection from "./LeverSection";
import OutcomeSection from "./OutcomeSection";
import PricingSection from "./PricingSection";
import ScheduleSection from "./ScheduleSection";

vi.mock("@/hooks/api/useMeasureFilterOptions", () => ({
  useMeasureDimensions: () => ({ view: "", dimensions: [], isLoading: false }),
  useDimensionValues: () => ({ data: [], isLoading: false, isError: false })
}));
vi.mock("@/stores/useCurrentOrg", () => ({
  default: (pick: (s: { org: { id: string } }) => unknown) => pick({ org: { id: "org-1" } })
}));
vi.mock("@/hooks/api/organizations/useOrgMembers", () => ({ useOrgMembers: () => ({ data: [] }) }));
vi.mock("@/hooks/api/appAccess/useTeams", () => ({ useTeams: () => ({ data: [] }) }));
vi.mock("@/hooks/api/useWorldModel", () => ({
  useWorldModel: () => ({
    data: {
      edges: [],
      entities: [
        {
          id: "customer",
          label: "customer",
          view: "customers",
          depth: 0,
          dimensions: [{ name: "tier", dim_type: "string" }],
          own_measures: [],
          induced_measures: []
        }
      ]
    }
  }),
  useWmInstances: (entity: string | null, search: string) => ({
    data: entity
      ? {
          total: 3,
          has_more: false,
          items: ["c-1", "c-2", "c-3"]
            .filter((k) => k.includes(search))
            .map((k) => ({ key: k, display: `Customer ${k}` }))
        }
      : undefined
  })
}));
vi.mock("@/hooks/api/useMetricTree", () => ({
  useMetricTree: () => ({
    data: {
      edges: [],
      nodes: [
        {
          id: "orders.total_order_value",
          view: "orders",
          measure: "total_order_value",
          label: "Total order value",
          measure_type: "sum",
          is_composite: false,
          drillable: true
        }
      ]
    }
  }),
  useTimeDimensions: () => ({ data: { by_view: { orders: ["orders.order_date"] } } })
}));
vi.mock("@/hooks/api/automations/useAutomations", () => ({
  default: () => ({ data: [{ path: "a/fruit_summary.automation.yml" }] })
}));

afterEach(cleanup);

function Harness({
  values,
  children
}: {
  values?: Partial<DesignFormValues>;
  children: (p: {
    form: ReturnType<typeof useForm<DesignFormValues>>;
    messageFor: (n: string) => string | undefined;
  }) => React.ReactNode;
}) {
  const form = useForm<DesignFormValues>({ defaultValues: { ...emptyDesignValues(), ...values } });
  return (
    <>
      {children({ form, messageFor: () => undefined })}
      <output data-testid='stored'>{JSON.stringify(form.watch())}</output>
    </>
  );
}
const stored = () =>
  JSON.parse(screen.getByTestId("stored").textContent ?? "{}") as DesignFormValues;
const field = (name: string) => screen.getByTestId(`experiment-design-${name}`) as HTMLInputElement;
const optionValues = (select: HTMLElement) =>
  within(select)
    .getAllByRole("option")
    .map((o) => (o as HTMLOptionElement).value);

describe("numbers are constrained", () => {
  const expectNumber = (name: string, min: string, max: string | null, step: string) => {
    const el = field(name);
    expect(el.type).toBe("number");
    expect(el.min).toBe(min);
    expect(el.max).toBe(max ?? "");
    expect(el.step).toBe(step);
  };

  it("day counts, in a waves design", () => {
    render(<Harness>{(p) => <ScheduleSection {...p} />}</Harness>);
    expectNumber("pre-days", "1", null, "1");
    expectNumber("post-days", "1", null, "1");
    expectNumber("anticipation-days", "0", null, "1");
    expectNumber("washout-days", "0", null, "1");
  });

  it("the period length, in a switchback", () => {
    render(<Harness values={{ kind: "switchback" }}>{(p) => <ScheduleSection {...p} />}</Harness>);
    expectNumber("period-days", "1", null, "1");
  });

  it("the quality floors, as percentages from 0 to 100", () => {
    render(<Harness>{(p) => <ScheduleSection {...p} />}</Harness>);
    expectNumber("coverage-floor", "0", "100", "any");
    expectNumber("compliance-floor", "0", "100", "any");
  });

  it("the grace days, family, seed, minimum effect and prior", () => {
    render(
      <Harness>
        {(p) => (
          <>
            <LeverSection {...p} />
            <OutcomeSection {...p} />
            <PricingSection {...p} />
          </>
        )}
      </Harness>
    );
    expectNumber("grace-days", "0", null, "1");
    expectNumber("family", "1", null, "1");
    expect(field("seed").type).toBe("number");
    expect(field("seed").step).toBe("1");
    expect(field("mde-at-proposal").type).toBe("number");
    expect(field("mde-at-proposal").step).toBe("any");
    expect(field("prior-value").type).toBe("number");
    expect(field("prior-value").step).toBe("any");
  });

  it("the significance of an anomaly guardrail, between 0 and 1", () => {
    render(
      <Harness
        values={{ guardrails: [{ kind: "anomaly", measure: "", significance: "0.05", when: "" }] }}
      >
        {(p) => <GuardrailsSection {...p} />}
      </Harness>
    );
    expectNumber("guardrails-0-significance", "0", "1", "any");
  });
});

describe("the time zone", () => {
  it("is a select of IANA zones, UTC first", async () => {
    render(<Harness>{(p) => <ScheduleSection {...p} />}</Harness>);
    const select = field("timezone");
    expect(select.tagName).toBe("SELECT");
    const zones = optionValues(select);
    expect(zones[0]).toBe("UTC");
    expect(zones).toContain("America/New_York");
    expect(select.value).toBe("UTC");

    await userEvent.selectOptions(select, "America/New_York");
    expect(stored().timezone).toBe("America/New_York");
  });

  it("still shows a stored zone that is not on the list", () => {
    render(
      <Harness values={{ timezone: "Mars/Olympus_Mons" }}>
        {(p) => <ScheduleSection {...p} />}
      </Harness>
    );
    expect(field("timezone").value).toBe("Mars/Olympus_Mons");
  });
});

describe("alpha and power", () => {
  it("are selects of sensible values", async () => {
    render(<Harness>{(p) => <OutcomeSection {...p} />}</Harness>);
    expect(optionValues(field("alpha"))).toEqual(["0.01", "0.05", "0.1", "0.2"]);
    expect(optionValues(field("power"))).toEqual(["70", "80", "90", "95"]);
    expect(field("alpha").value).toBe("0.05");
    expect(field("power").value).toBe("80");

    await userEvent.selectOptions(field("alpha"), "0.1");
    await userEvent.selectOptions(field("power"), "90");
    expect(stored().alpha).toBe("0.1");
    expect(stored().power).toBe("90");
  });

  it("show a stored value that is not a standard one", () => {
    render(
      <Harness values={{ alpha: "0.07", power: "85" }}>{(p) => <OutcomeSection {...p} />}</Harness>
    );
    expect(field("alpha").value).toBe("0.07");
    expect(field("power").value).toBe("85");
    expect(optionValues(field("alpha"))).toContain("0.07");
  });
});

describe("the measures, entities and automations", () => {
  it("outcome, driver and sibling are measure pickers", () => {
    render(
      <Harness values={{ target: "orders.total_order_value", sibling: "orders.nope" }}>
        {(p) => <OutcomeSection {...p} />}
      </Harness>
    );
    for (const name of ["target", "driver", "sibling"]) {
      expect(field(name)).toHaveAttribute("role", "combobox");
    }
    expect(screen.getByTestId("experiment-design-target-match")).toHaveAttribute(
      "data-tone",
      "success"
    );
    expect(screen.getByTestId("experiment-design-sibling-match")).toHaveAttribute(
      "data-tone",
      "warning"
    );
  });

  it("the lever's automations are pickers and its read-back conditions are builders", () => {
    render(
      <Harness values={{ onVia: "fruit_summary", whenOn: "{{orders.total_order_value}} > 0" }}>
        {(p) => <LeverSection {...p} />}
      </Harness>
    );
    expect(field("on-via")).toHaveAttribute("role", "combobox");
    expect(field("off-via")).toHaveAttribute("role", "combobox");
    expect(field("when-on-measure").value).toBe("orders.total_order_value");
    expect(field("when-off-measure").value).toBe("");
  });

  it("a trigger guardrail's condition is a builder, an anomaly one's measure a picker", () => {
    render(
      <Harness
        values={{
          guardrails: [
            {
              kind: "trigger",
              measure: "",
              significance: "0.05",
              when: "{{orders.total_order_value}} < 5"
            },
            { kind: "anomaly", measure: "orders.total_order_value", significance: "0.05", when: "" }
          ]
        }}
      >
        {(p) => <GuardrailsSection {...p} />}
      </Harness>
    );
    expect(field("guardrails-0-when-measure").value).toBe("orders.total_order_value");
    expect(field("guardrails-1-measure")).toHaveAttribute("role", "combobox");
  });

  it("the entity and its dimension are pickers, and the time dimension follows the outcome", () => {
    render(
      <Harness values={{ forEach: "customer" }}>
        {(p) => (
          <>
            <AboutSection {...p} nameLocked={false} />
            <ScheduleSection {...p} />
          </>
        )}
      </Harness>
    );
    expect(field("for-each")).toHaveAttribute("role", "combobox");
    expect(field("assign-by")).toHaveAttribute("role", "combobox");
    expect(field("time-dimension")).toHaveAttribute("role", "combobox");
  });
});

describe("the experiment name", () => {
  it("is flagged as soon as it is not letters, digits and underscores", async () => {
    const user = userEvent.setup();
    render(<Harness>{(p) => <AboutSection {...p} nameLocked={false} />}</Harness>);
    await user.type(field("experiment"), "my pilot");
    expect(screen.getByTestId("experiment-design-experiment-error")).toHaveTextContent(
      "letters, digits and underscores"
    );
    await user.clear(field("experiment"));
    await user.type(field("experiment"), "my_pilot_2");
    expect(screen.queryByTestId("experiment-design-experiment-error")).toBeNull();
  });

  it("is not judged once it is locked", () => {
    render(
      <Harness values={{ experiment: "old-name" }}>
        {(p) => <AboutSection {...p} nameLocked />}
      </Harness>
    );
    expect(screen.queryByTestId("experiment-design-experiment-error")).toBeNull();
  });
});

describe("unit lists", () => {
  it("offer the entity's instances and write them as a comma list", async () => {
    const user = userEvent.setup();
    render(
      <Harness values={{ forEach: "customer", waves: [{ switchOn: "2026-03-02", units: "c-1" }] }}>
        {(p) => <PricingSection {...p} />}
      </Harness>
    );
    expect(screen.getByTestId("experiment-design-waves-0-units-chips")).toHaveTextContent("c-1");

    await user.click(field("waves-0-units-search"));
    const list = screen.getByRole("listbox");
    expect(
      within(list)
        .getAllByRole("option")
        .map((o) => o.getAttribute("data-value"))
    ).toEqual(["c-2", "c-3"]);
    await user.click(screen.getByRole("option", { name: /c-3/ }));
    expect(stored().waves[0].units).toBe("c-1, c-3");

    await user.click(screen.getByRole("button", { name: "Remove c-1" }));
    expect(stored().waves[0].units).toBe("c-3");
  });
});
