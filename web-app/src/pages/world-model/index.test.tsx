// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorldModel } from "@/types/worldModel";
import CustomerWorldModelPage from "./index";

/** The customer's read-only world model: apps-first only, plain words, no paths. */

let appsFirst = true;
const askedForModel = vi.fn();
let model: WorldModel;

vi.mock("@/hooks/useAppsFirstExperience", () => ({ default: () => appsFirst }));
vi.mock("@/stores/useCurrentOrg", () => ({
  default: <T,>(selector: (s: unknown) => T) =>
    selector({ org: { id: "org-1", slug: "northwind", name: "Northwind" } })
}));
vi.mock("@/hooks/api/useWorldModel", () => ({
  useWorldModel: () => {
    askedForModel();
    return { data: model, isLoading: false, error: null };
  }
}));
// The graph is the Factory's own component; here, a button per entity stands in.
vi.mock("@/pages/ide/WorldModel/components/WorldModelGraph", () => ({
  WorldModelGraph: ({
    model: m,
    onSelectEntity,
    onOpenPicker
  }: {
    model: WorldModel;
    onSelectEntity: (id: string) => void;
    onOpenPicker?: unknown;
  }) => (
    <div data-testid='graph' data-picker={String(!!onOpenPicker)}>
      {m.entities.map((e) => (
        <button key={e.id} type='button' onClick={() => onSelectEntity(e.id)}>
          node {e.label}
        </button>
      ))}
    </div>
  )
}));

function Location() {
  return <div data-testid='location'>{useLocation().pathname}</div>;
}

const renderPage = () =>
  render(
    <MemoryRouter initialEntries={["/northwind/workspaces/ws-1/world-model"]}>
      <Routes>
        <Route path='/northwind/workspaces/ws-1/world-model' element={<CustomerWorldModelPage />} />
        <Route path='*' element={<Location />} />
      </Routes>
    </MemoryRouter>
  );

beforeEach(() => {
  appsFirst = true;
  askedForModel.mockClear();
  model = {
    entities: [
      {
        id: "store",
        label: "Store",
        view: "stores_view",
        description: "A Northwind shop.",
        depth: 0,
        dimensions: [{ name: "city", dim_type: "string", label: "City" }],
        own_measures: [
          {
            name: "revenue",
            measure_type: "sum",
            additivity: "additive",
            label: "Revenue",
            expr: "SUM(orders.amount)"
          }
        ],
        induced_measures: []
      },
      {
        id: "order",
        label: "Order",
        view: "orders_view",
        depth: 1,
        dimensions: [],
        own_measures: [],
        induced_measures: []
      }
    ],
    edges: [{ from: "order", to: "store", functional: true }]
  };
});
afterEach(cleanup);

describe("the customer's world model page", () => {
  it("sends people home, without asking for the model, with the flag off", () => {
    appsFirst = false;
    renderPage();
    expect(screen.getByTestId("location")).toHaveTextContent("/northwind/home");
    expect(askedForModel).not.toHaveBeenCalled();
  });

  it("names the page for the org and opens no instance picker", () => {
    renderPage();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(
      "How Northwind's data fits together"
    );
    expect(screen.getByTestId("graph")).toHaveAttribute("data-picker", "false");
  });

  it("describes a selected entity in plain words, never its view or expression", () => {
    renderPage();
    fireEvent.click(screen.getByText("node Store"));
    const details = screen.getByTestId("world-model-details");
    expect(details).toHaveTextContent("A Northwind shop.");
    expect(details).toHaveTextContent("Revenue");
    expect(details).toHaveTextContent("City");
    expect(details).toHaveTextContent("Order");
    expect(details.textContent).not.toMatch(/stores_view|SUM\(|workspace/i);
  });
});
