// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RevisionFileEntry } from "@/services/api/revisionFiles";
import { buildTree } from "../utils";
import FileTree, { filterEntries, indexTree } from "./FileTree";

const entries: RevisionFileEntry[] = [
  { path: "config.yml", kind: "config" },
  { path: "semantics/views/orders.view.yml", kind: "view" },
  { path: "semantics/views/stores.view.yml", kind: "view" },
  { path: "agents/sales.agentic.yml", kind: "agent" }
];

afterEach(cleanup);

const names = () => screen.getAllByRole("treeitem").map((el) => el.textContent);

describe("FileTree", () => {
  it("opens the top-level folders and opens a file on click", () => {
    const onSelect = vi.fn();
    render(<FileTree entries={entries} selectedPath={null} onSelect={onSelect} />);
    expect(names()).toEqual(["agents", "sales.agentic.yml", "semantics", "views", "config.yml"]);
    fireEvent.click(screen.getByText("config.yml"));
    expect(onSelect).toHaveBeenCalledWith({ path: "config.yml", kind: "config" });
  });

  it("opens the folders above the selected file", () => {
    render(
      <FileTree
        entries={entries}
        selectedPath='semantics/views/orders.view.yml'
        onSelect={vi.fn()}
      />
    );
    const selected = screen.getByText("orders.view.yml").closest("[role=treeitem]");
    expect(selected?.getAttribute("aria-current")).toBe("true");
  });

  it("keeps the open file marked, and only it, whatever else is clicked or pressed", () => {
    render(
      <FileTree
        entries={entries}
        selectedPath='semantics/views/orders.view.yml'
        onSelect={vi.fn()}
      />
    );
    const current = () =>
      screen
        .getAllByRole("treeitem")
        .filter((el) => el.getAttribute("aria-current") === "true")
        .map((el) => el.textContent);
    fireEvent.click(screen.getByText("agents"));
    expect(current()).toEqual(["orders.view.yml"]);
    const tree = screen.getByTestId("ide-files-tree");
    fireEvent.keyDown(tree, { key: "a", code: "KeyA", ctrlKey: true });
    fireEvent.keyDown(tree, { key: "ArrowDown", code: "ArrowDown", shiftKey: true });
    expect(current()).toEqual(["orders.view.yml"]);
  });

  it("filters to matching paths with their folders open, and says when nothing matches", () => {
    render(<FileTree entries={entries} selectedPath={null} onSelect={vi.fn()} />);
    const filter = screen.getByTestId("ide-files-filter");
    fireEvent.change(filter, { target: { value: "STORES" } });
    expect(names()).toEqual(["semantics", "views", "stores.view.yml"]);
    fireEvent.change(filter, { target: { value: "nope" } });
    expect(screen.queryByRole("tree")).toBeNull();
    expect(screen.getByText(/No file matches/)).toBeTruthy();
  });

  it("collapses and expands everything", () => {
    render(<FileTree entries={entries} selectedPath={null} onSelect={vi.fn()} />);
    fireEvent.click(screen.getByLabelText("Collapse all"));
    expect(names()).toEqual(["agents", "semantics", "config.yml"]);
  });
});

describe("filterEntries", () => {
  it("keeps everything for a blank query", () => {
    expect(filterEntries(entries, "  ")).toHaveLength(4);
  });
});

describe("indexTree", () => {
  it("keys folders and files apart and lists children in tree order", () => {
    const nodes = indexTree(buildTree(entries));
    expect(nodes.get("root")?.children).toEqual(["d:agents", "d:semantics", "f:config.yml"]);
    expect(nodes.get("d:semantics/views")?.children).toEqual([
      "f:semantics/views/orders.view.yml",
      "f:semantics/views/stores.view.yml"
    ]);
    expect(nodes.get("f:config.yml")?.entry).toEqual({ path: "config.yml", kind: "config" });
  });
});
