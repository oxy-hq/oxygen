// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DesignFormValues } from "../designForm";
import { emptyDesignValues } from "../designForm";
import {
  AssignByField,
  AutomationField,
  EntityField,
  MeasureField,
  TimeDimensionField
} from "./pickers";

const lists = vi.hoisted(() => ({ loaded: true }));

vi.mock("@/hooks/api/useWorldModel", () => ({
  useWorldModel: () => ({
    data: lists.loaded
      ? {
          edges: [],
          entities: [
            {
              id: "customer",
              label: "customer",
              view: "customers",
              description: null,
              depth: 0,
              dimensions: [{ name: "tier", dim_type: "string" }],
              own_measures: [],
              induced_measures: []
            },
            {
              id: "store",
              label: "store",
              view: "stores",
              description: "A shop",
              depth: 0,
              dimensions: [
                { name: "region", dim_type: "string", label: "Sales region" },
                { name: "format", dim_type: "string" }
              ],
              own_measures: [],
              induced_measures: []
            }
          ]
        }
      : undefined
  })
}));
vi.mock("@/hooks/api/useMetricTree", () => ({
  useMetricTree: () => ({
    data: lists.loaded
      ? {
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
            },
            {
              id: "refunds.rate",
              view: "refunds",
              measure: "rate",
              label: "rate",
              description: "Share of orders refunded",
              measure_type: "number",
              is_composite: true,
              drillable: false
            }
          ]
        }
      : undefined
  }),
  useTimeDimensions: () => ({
    data: lists.loaded
      ? { by_view: { orders: ["orders.order_date"], refunds: ["refunds.day"] } }
      : undefined
  })
}));
vi.mock("@/hooks/api/automations/useAutomations", () => ({
  default: () => ({
    data: lists.loaded
      ? [
          {
            path: "automations/fruit_summary.automation.yml",
            description: "Sends the summary"
          },
          { path: "automations/promo.automation.yml" }
        ]
      : undefined
  })
}));

afterEach(() => {
  cleanup();
  lists.loaded = true;
});

function Harness({
  values,
  children
}: {
  values?: Partial<DesignFormValues>;
  children: (p: {
    form: ReturnType<typeof useForm<DesignFormValues>>;
    messageFor: () => undefined;
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
const stored = (key: keyof DesignFormValues) =>
  (JSON.parse(screen.getByTestId("stored").textContent ?? "{}") as DesignFormValues)[key];
const input = (name: string) => screen.getByTestId(`experiment-design-${name}`) as HTMLInputElement;
const optionValues = () => screen.getAllByRole("option").map((o) => o.getAttribute("data-value"));
const match = (name: string) => screen.getByTestId(`experiment-design-${name}-match`);

describe("EntityField", () => {
  it("lists the semantic model's entities and picking one writes its id", async () => {
    const user = userEvent.setup();
    render(<Harness>{(p) => <EntityField {...p} />}</Harness>);
    await user.click(input("for-each"));
    expect(optionValues()).toEqual(["customer", "store"]);
    await user.click(screen.getByRole("option", { name: /store/ }));
    expect(stored("forEach")).toBe("store");
    expect(match("for-each")).toHaveAttribute("data-tone", "success");
  });

  it("flags a value that is no entity, but keeps it", () => {
    render(<Harness values={{ forEach: "shop" }}>{(p) => <EntityField {...p} />}</Harness>);
    expect(match("for-each")).toHaveAttribute("data-tone", "warning");
    expect(input("for-each").value).toBe("shop");
  });

  it("says nothing about a value while the list has not loaded", () => {
    lists.loaded = false;
    render(<Harness values={{ forEach: "shop" }}>{(p) => <EntityField {...p} />}</Harness>);
    expect(screen.queryByTestId("experiment-design-for-each-match")).toBeNull();
  });
});

describe("AssignByField", () => {
  it("lists the dimensions of the chosen entity only", async () => {
    const user = userEvent.setup();
    render(<Harness values={{ forEach: "store" }}>{(p) => <AssignByField {...p} />}</Harness>);
    await user.click(input("assign-by"));
    expect(optionValues()).toEqual(["format", "region"]);
    await user.click(screen.getByRole("option", { name: /region/ }));
    expect(stored("assignBy")).toBe("region");
    expect(match("assign-by")).toHaveAttribute("data-tone", "success");
  });

  it("offers nothing until an entity is chosen", async () => {
    const user = userEvent.setup();
    render(<Harness>{(p) => <AssignByField {...p} />}</Harness>);
    await user.click(input("assign-by"));
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("flags a dimension the entity does not have", () => {
    render(
      <Harness values={{ forEach: "store", assignBy: "tier" }}>
        {(p) => <AssignByField {...p} />}
      </Harness>
    );
    expect(match("assign-by")).toHaveAttribute("data-tone", "warning");
  });
});

describe("MeasureField", () => {
  it("groups the measures by view, with each one's label or description beside it", async () => {
    const user = userEvent.setup();
    render(
      <Harness>{(p) => <MeasureField {...p} name='target' label='Outcome measure' />}</Harness>
    );
    await user.click(input("target"));
    const list = screen.getByRole("listbox");
    expect(list).toHaveTextContent("orders");
    expect(list).toHaveTextContent("refunds");
    expect(list).toHaveTextContent("Total order value");
    expect(list).toHaveTextContent("Share of orders refunded");
    expect(optionValues()).toEqual(["orders.total_order_value", "refunds.rate"]);

    await user.click(screen.getByRole("option", { name: /refunds\.rate/ }));
    expect(stored("target")).toBe("refunds.rate");
  });

  it("works on a guardrail row's measure", async () => {
    const user = userEvent.setup();
    render(
      <Harness
        values={{ guardrails: [{ kind: "anomaly", measure: "", significance: "0.05", when: "" }] }}
      >
        {(p) => <MeasureField {...p} name='guardrails.0.measure' label='Measure to watch' />}
      </Harness>
    );
    await user.type(input("guardrails-0-measure"), "refunds");
    await user.click(screen.getByRole("option", { name: /refunds\.rate/ }));
    expect((stored("guardrails") as DesignFormValues["guardrails"])[0].measure).toBe(
      "refunds.rate"
    );
  });

  it("flags a measure that is not in the semantic model", () => {
    render(
      <Harness values={{ target: "orders.nope" }}>
        {(p) => <MeasureField {...p} name='target' label='Outcome measure' />}
      </Harness>
    );
    expect(match("target")).toHaveAttribute("data-tone", "warning");
  });
});

describe("AutomationField", () => {
  it("lists the workspace's automations by name", async () => {
    const user = userEvent.setup();
    render(
      <Harness>{(p) => <AutomationField {...p} name='onVia' label='Turned on by' />}</Harness>
    );
    await user.click(input("on-via"));
    expect(optionValues()).toEqual(["fruit_summary", "promo"]);
    await user.click(screen.getByRole("option", { name: /fruit_summary/ }));
    expect(stored("onVia")).toBe("fruit_summary");
    expect(match("on-via")).toHaveAttribute("data-tone", "success");
  });

  it("flags a name no automation has", () => {
    render(
      <Harness values={{ offVia: "ghost" }}>
        {(p) => <AutomationField {...p} name='offVia' label='Turned off by' />}
      </Harness>
    );
    expect(match("off-via")).toHaveAttribute("data-tone", "warning");
  });
});

describe("TimeDimensionField", () => {
  it("offers the dates of the outcome's view", async () => {
    const user = userEvent.setup();
    render(
      <Harness values={{ target: "orders.total_order_value" }}>
        {(p) => <TimeDimensionField {...p} />}
      </Harness>
    );
    await user.click(input("time-dimension"));
    expect(optionValues()).toEqual(["orders.order_date"]);
    await user.click(screen.getByRole("option"));
    expect(stored("timeDimension")).toBe("orders.order_date");
  });

  it("flags a date of another view", () => {
    render(
      <Harness values={{ target: "orders.total_order_value", timeDimension: "refunds.day" }}>
        {(p) => <TimeDimensionField {...p} />}
      </Harness>
    );
    expect(match("time-dimension")).toHaveAttribute("data-tone", "warning");
  });
});
