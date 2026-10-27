// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type DesignFormValues, emptyDesignValues } from "../designForm";
import ConditionField from "./ConditionField";

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
        },
        {
          id: "refunds.rate",
          view: "refunds",
          measure: "rate",
          label: "Refund rate",
          measure_type: "number",
          is_composite: false,
          drillable: false
        }
      ]
    }
  })
}));

afterEach(cleanup);

function Harness({ whenOn = "", ratio = false }: { whenOn?: string; ratio?: boolean }) {
  const form = useForm<DesignFormValues>({
    defaultValues: { ...emptyDesignValues(), whenOn }
  });
  return (
    <>
      <ConditionField
        form={form}
        messageFor={() => undefined}
        name='whenOn'
        label='Confirmed on when'
        ratio={ratio}
      />
      <output data-testid='stored'>{form.watch("whenOn")}</output>
    </>
  );
}
const stored = () => screen.getByTestId("stored").textContent;
const measure = () => screen.getByTestId("experiment-design-when-on-measure") as HTMLInputElement;
const op = () => screen.getByTestId("experiment-design-when-on-op") as HTMLSelectElement;
const num = () => screen.getByTestId("experiment-design-when-on-value") as HTMLInputElement;
const whole = () => screen.getByTestId("experiment-design-when-on") as HTMLInputElement;

describe("ConditionField", () => {
  it("writes the {{view.measure}} <op> <number> string from a measure, an operator and a number", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(measure());
    await user.click(screen.getByRole("option", { name: /orders\.total_order_value/ }));
    expect(stored()).toBe("");
    await user.selectOptions(op(), ">=");
    await user.type(num(), "0");

    expect(stored()).toBe("{{orders.total_order_value}} >= 0");
  });

  it("offers the measures with their labels", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(measure());
    expect(screen.getByRole("listbox")).toHaveTextContent("Refund rate");
  });

  it("reads a stored condition into its three parts", () => {
    render(<Harness whenOn='{{orders.total_order_value}} < 12.5' />);
    expect(measure().value).toBe("orders.total_order_value");
    expect(op().value).toBe("<");
    expect(num().value).toBe("12.5");
    expect(stored()).toBe("{{orders.total_order_value}} < 12.5");
  });

  it("still takes the whole condition typed into the field with the form's testid", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(whole());
    await user.paste("{{orders.total_order_value}} > 0");
    expect(stored()).toBe("{{orders.total_order_value}} > 0");
    expect(measure().value).toBe("orders.total_order_value");
    expect(num().value).toBe("0");
    fireEvent.change(whole(), { target: { value: "" } });
    expect(stored()).toBe("");
    expect(measure().value).toBe("");
  });

  it("shows a condition it cannot build read-only, untouched, with a way to edit it as text", async () => {
    const user = userEvent.setup();
    const odd = "{{orders.total_order_value}} > {{refunds.rate}}";
    render(<Harness whenOn={odd} />);

    expect(screen.queryByTestId("experiment-design-when-on-measure")).toBeNull();
    expect(whole().value).toBe(odd);
    expect(whole().readOnly).toBe(true);
    expect(stored()).toBe(odd);

    await user.click(screen.getByRole("button", { name: "Edit as text" }));
    expect(whole().readOnly).toBe(false);
    await user.clear(whole());
    await user.type(whole(), "{{{{refunds.rate}} > 0.1");
    expect(stored()).toBe("{{refunds.rate}} > 0.1");

    await user.click(screen.getByRole("button", { name: "Use the builder" }));
    expect(measure().value).toBe("refunds.rate");
    expect(op().value).toBe(">");
    expect(num().value).toBe("0.1");
  });

  it("can switch a parseable condition to text and back", async () => {
    const user = userEvent.setup();
    render(<Harness whenOn='{{refunds.rate}} > 0' />);
    await user.click(screen.getByRole("button", { name: "Edit as text" }));
    expect(screen.queryByTestId("experiment-design-when-on-measure")).toBeNull();
    expect(whole().value).toBe("{{refunds.rate}} > 0");
    expect(whole().readOnly).toBe(false);
  });

  it("flags a measure that is not in the semantic model", () => {
    render(<Harness whenOn='{{orders.nope}} > 0' />);
    expect(screen.getByTestId("experiment-design-when-on-measure-match")).toHaveAttribute(
      "data-tone",
      "warning"
    );
  });
});

describe("ConditionField with a ratio (trigger guardrails)", () => {
  const denominator = () =>
    screen.getByTestId("experiment-design-when-on-denominator") as HTMLInputElement;

  it("divides by a second measure when one is chosen", async () => {
    const user = userEvent.setup();
    render(<Harness ratio />);
    await user.click(measure());
    await user.click(screen.getByRole("option", { name: /orders\.total_order_value/ }));
    await user.click(denominator());
    await user.click(screen.getByRole("option", { name: /refunds\.rate/ }));
    await user.type(num(), "0.6");

    expect(stored()).toBe("{{orders.total_order_value}} / {{refunds.rate}} > 0.6");
  });

  it("opens a stored ratio in the builder, not read-only", () => {
    render(<Harness ratio whenOn='{{orders.total_order_value}} / {{refunds.rate}} > 0.6' />);
    expect(measure().value).toBe("orders.total_order_value");
    expect(denominator().value).toBe("refunds.rate");
  });

  it("offers no denominator where one measure is the rule (read-back)", () => {
    render(<Harness />);
    expect(screen.queryByTestId("experiment-design-when-on-denominator")).toBeNull();
  });
});
