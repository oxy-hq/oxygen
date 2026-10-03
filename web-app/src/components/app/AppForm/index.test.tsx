// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppForm, type AppFormData } from "./index";

afterEach(() => {
  cleanup();
});

type OnChange = (data: Partial<AppFormData>) => void;

const control = (extra: Record<string, unknown> = {}) => ({
  type: "control",
  name: "region",
  control_type: "select",
  options: ["east", "west"],
  ...extra
});

/** Renders the form on one select control, edits its label, and returns the
 *  control as the form then emits it. */
const emittedControl = async (data: Partial<AppFormData>) => {
  const onChange = vi.fn<OnChange>();
  render(<AppForm data={data} onChange={onChange} />);
  fireEvent.click(screen.getByText("Display 1"));
  fireEvent.change(screen.getByLabelText("Label"), { target: { value: "Region" } });
  await waitFor(() => expect(onChange).toHaveBeenCalled());
  return { onChange, control: onChange.mock.calls[onChange.mock.calls.length - 1][0].display?.[0] };
};

// A select control's default is rendered into the app's SQL: `default: ""` as
// '' and no default as none. A registered text input holds "" for a control
// that has no default, so the two used to be indistinguishable on the way out.
describe("AppForm — a select control's default", () => {
  it("does not add a default the control did not have", async () => {
    const { control: emitted } = await emittedControl({ display: [control()] });
    expect(emitted).not.toHaveProperty("default");
  });

  it('keeps a file\'s `default: ""` through an edit', async () => {
    const { control: emitted } = await emittedControl({ display: [control({ default: "" })] });
    expect(emitted).toHaveProperty("default", "");
  });

  it("drops the default when the user clears it", async () => {
    const { onChange } = await emittedControl({ display: [control({ default: "east" })] });
    fireEvent.change(screen.getByLabelText("Default Value"), { target: { value: "" } });
    await waitFor(() =>
      expect(
        onChange.mock.calls[onChange.mock.calls.length - 1][0].display?.[0]
      ).not.toHaveProperty("default")
    );
  });

  it("writes `tasks: []` for an app whose tasks were all removed", async () => {
    const onChange = vi.fn<OnChange>();
    render(<AppForm data={{ tasks: [], display: [control()] }} onChange={onChange} />);
    fireEvent.click(screen.getByText("Display 1"));
    fireEvent.change(screen.getByLabelText("Label"), { target: { value: "Region" } });
    await waitFor(() => expect(onChange).toHaveBeenCalled());
    expect(onChange.mock.calls[onChange.mock.calls.length - 1][0].tasks).toEqual([]);
  });
});
