// @vitest-environment jsdom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parse } from "yaml";
import NewExperimentButton from "./NewExperimentButton";

const h = vi.hoisted(() => ({ create: vi.fn() }));

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
  useCreateExperiment: () => ({ mutateAsync: h.create, isPending: false })
}));
vi.mock("@/hooks/useRole", () => ({
  useRole: () => ({ is: { workspaceAdmin: true, workspaceEditor: true } })
}));
vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "ws-1" }, branchName: "main" })
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

/** The draft the agentic flow creates; `vitest` runs from `web-app/`. */
const EXAMPLE = parse(
  readFileSync(
    resolve(process.cwd(), "../examples/experiments/customer_orders_pilot.experiment.yml"),
    "utf8"
  )
) as Record<string, unknown>;

beforeEach(() => {
  h.create.mockReset().mockResolvedValue({ version: 1 });
});
afterEach(cleanup);

const open = async () => {
  const user = userEvent.setup();
  render(
    <MemoryRouter>
      <NewExperimentButton />
    </MemoryRouter>
  );
  await user.click(screen.getByTestId("experiment-new"));
  return user;
};

const field = (name: string) => screen.getByTestId(`experiment-design-${name}`) as HTMLInputElement;

/** Replace a field's text the way a person would, with `{{` typed literally. */
const fill = async (user: ReturnType<typeof userEvent.setup>, name: string, value: string) => {
  await user.clear(field(name));
  await user.click(field(name));
  await user.paste(value);
};

/** Fill in every field of the example design. */
const fillExample = async (user: ReturnType<typeof userEvent.setup>) => {
  await fill(user, "experiment", "customer_orders_pilot");
  await fill(user, "owner", "ops");
  await fill(user, "hypothesis", "A customer-level promo lifts order value per customer-day");
  await fill(user, "for-each", "customer");
  await fill(user, "target", "orders.total_order_value");
  await fill(user, "on-via", "fruit_summary");
  await fill(user, "off-via", "fruit_summary");
  await fill(user, "when-on", "{{orders.total_order_value}} > 0");
  await fill(user, "when-off", "{{orders.total_order_value}} >= 0");
  await fill(user, "grace-days", "2");
  await fill(user, "time-dimension", "orders.order_date");
  await fill(user, "pre-days", "56");
  await fill(user, "post-days", "56");
  await fill(user, "anticipation-days", "3");
  await fill(user, "washout-days", "7");
};

describe("NewExperimentButton — the design form", () => {
  it("builds exactly the example design from the fields a person fills in", async () => {
    const user = await open();

    await fillExample(user);
    await user.click(screen.getByTestId("experiment-design-submit"));

    await waitFor(() => expect(h.create).toHaveBeenCalledTimes(1));
    expect(h.create.mock.calls[0][0]).toEqual(EXAMPLE);
  });

  it("offers no YAML import", async () => {
    await open();

    expect(screen.queryByText(/import/i)).toBeNull();
    expect(screen.queryByTestId("experiment-design-import-open")).toBeNull();
  });

  it("refuses an empty form beside each missing field, without calling the server", async () => {
    const user = await open();

    await user.click(screen.getByTestId("experiment-design-submit"));

    expect(await screen.findByTestId("experiment-design-experiment-error")).toHaveTextContent(
      "Give the experiment a name."
    );
    expect(screen.getByTestId("experiment-design-target-error")).toBeInTheDocument();
    expect(screen.getByTestId("experiment-design-grace-days-error")).toBeInTheDocument();
    expect(h.create).not.toHaveBeenCalled();
  });

  it("puts a taken name beside the name field", async () => {
    h.create.mockRejectedValue(
      new Error("an experiment named `customer_orders_pilot` already exists")
    );
    const user = await open();
    await fillExample(user);

    await user.click(screen.getByTestId("experiment-design-submit"));

    expect(await screen.findByTestId("experiment-design-experiment-error")).toHaveTextContent(
      "already exists"
    );
    expect(screen.getByTestId("experiment-design-error")).toHaveTextContent(
      "shown beside their fields"
    );
  });
});
