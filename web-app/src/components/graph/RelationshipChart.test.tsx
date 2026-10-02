// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const layoutTree = vi.hoisted(() => vi.fn());
vi.mock("./elkLayout", () => ({ layoutTree }));

import RelationshipChart from "./RelationshipChart";

const chart = (label: string) => (
  <RelationshipChart nodes={[{ id: "partner", data: { label } }]} edges={[]} />
);

beforeEach(() => {
  vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

it("says so when ELK cannot lay the graph out, and draws again once it can", async () => {
  layoutTree.mockRejectedValueOnce(new Error("Referenced shape does not exist"));
  const { rerender } = render(chart("Acme"));

  expect(await screen.findByText("Failed to draw the chart")).toBeTruthy();

  layoutTree.mockImplementationOnce((nodes: unknown[]) => Promise.resolve(nodes));
  rerender(chart("Globex"));

  expect(await screen.findByText("Globex")).toBeTruthy();
  expect(screen.queryByText("Failed to draw the chart")).toBeNull();
});
