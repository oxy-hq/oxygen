// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import type { DesignFormValues } from "../designForm";
import { type Match, PickerField, type PickerGroup } from "./PickerField";

afterEach(cleanup);

const GROUPS: PickerGroup[] = [
  {
    title: "orders",
    options: [
      { value: "orders.count", label: "orders.count", detail: "Order count" },
      { value: "orders.revenue", label: "orders.revenue", detail: "Revenue" }
    ]
  },
  { title: "refunds", options: [{ value: "refunds.rate", label: "refunds.rate", detail: "" }] }
];

function Harness({
  groups = GROUPS,
  match = null,
  value = ""
}: {
  groups?: PickerGroup[];
  match?: Match | null;
  value?: string;
}) {
  const form = useForm<DesignFormValues>({ defaultValues: { target: value } as DesignFormValues });
  return (
    <>
      <PickerField
        form={form}
        messageFor={() => undefined}
        name='target'
        label='Measure'
        listLabel='Measures'
        groups={groups}
        match={match}
      />
      <output data-testid='stored'>{form.watch("target")}</output>
    </>
  );
}

const input = () => screen.getByTestId("experiment-design-target") as HTMLInputElement;
const stored = () => screen.getByTestId("stored").textContent;

describe("PickerField", () => {
  it("lists the options under their group titles on focus, and picking one writes its value", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(input());

    const list = await screen.findByRole("listbox", { name: "Measures" });
    expect(list).toHaveTextContent("orders");
    expect(list).toHaveTextContent("refunds");
    expect(list).toHaveTextContent("Revenue");

    await user.click(screen.getByRole("option", { name: /orders\.revenue/ }));
    expect(input().value).toBe("orders.revenue");
    expect(stored()).toBe("orders.revenue");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("filters by what is typed, in the value, the label and the detail", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.type(input(), "refu");
    expect(screen.getAllByRole("option").map((o) => o.getAttribute("data-value"))).toEqual([
      "refunds.rate"
    ]);

    await user.clear(input());
    await user.type(input(), "revenue");
    expect(screen.getAllByRole("option").map((o) => o.getAttribute("data-value"))).toEqual([
      "orders.revenue"
    ]);
  });

  it("still takes typed text that is not an option", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.type(input(), "something_else");
    expect(stored()).toBe("something_else");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("shows the verdict on the value in place of the hint", () => {
    render(<Harness value='orders.nope' match={{ tone: "warning", text: "Not a measure" }} />);
    const badge = screen.getByTestId("experiment-design-target-match");
    expect(badge).toHaveAttribute("data-tone", "warning");
    expect(badge).toHaveTextContent("Not a measure");
  });

  it("keeps browsers and password managers off the field", () => {
    render(<Harness />);
    expect(input()).toHaveAttribute("autocomplete", "off");
    expect(input()).toHaveAttribute("data-bwignore", "true");
    expect(input()).toHaveAttribute("data-1p-ignore", "true");
    expect(input()).toHaveAttribute("data-lpignore", "true");
    expect(input()).not.toHaveAttribute("list");
  });

  it("opens no list when there is nothing to pick", async () => {
    const user = userEvent.setup();
    render(<Harness groups={[]} />);
    await user.click(input());
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});

describe("PickerField inside a dialog", () => {
  it("closes only its list on Escape, and leaves Escape to the dialog once the list is closed", async () => {
    const user = userEvent.setup();
    const onOpenChange = vi.fn();
    render(
      <Dialog open onOpenChange={onOpenChange}>
        <DialogContent>
          <DialogTitle>t</DialogTitle>
          <DialogDescription>d</DialogDescription>
          <Harness />
        </DialogContent>
      </Dialog>
    );
    await user.click(input());
    expect(await screen.findByRole("listbox")).toBeInTheDocument();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(onOpenChange).not.toHaveBeenCalled();

    await user.keyboard("{Escape}");
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
