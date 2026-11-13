import { describe, expect, it } from "vitest";
import { ancestorFolders, buildTree, type FolderNode } from "./utils";

describe("buildTree", () => {
  it("nests files under their folders, folders first", () => {
    const tree = buildTree([
      { path: "config.yml", kind: "config" },
      { path: "semantics/views/orders.view.yml", kind: "view" },
      { path: "semantics/topics/sales.topic.yml", kind: "topic" },
      { path: "agents/sales.agentic.yml", kind: "agent" }
    ]);
    expect(tree.children.map((c) => c.name)).toEqual(["agents", "semantics", "config.yml"]);
    const semantics = tree.children[1] as FolderNode;
    expect(semantics.path).toBe("semantics");
    expect(semantics.children.map((c) => c.name)).toEqual(["topics", "views"]);
    const views = semantics.children[1] as FolderNode;
    expect(views.path).toBe("semantics/views");
    expect(views.children).toEqual([
      {
        type: "file",
        name: "orders.view.yml",
        entry: { path: "semantics/views/orders.view.yml", kind: "view" }
      }
    ]);
  });

  it("keeps dot files at the root", () => {
    const tree = buildTree([{ path: ".world-model.yml", kind: "world_model" }]);
    expect(tree.children.map((c) => c.name)).toEqual([".world-model.yml"]);
  });
});

describe("ancestorFolders", () => {
  it("lists every folder above a file", () => {
    expect(ancestorFolders("semantics/views/orders.view.yml")).toEqual([
      "semantics",
      "semantics/views"
    ]);
    expect(ancestorFolders("config.yml")).toEqual([]);
  });
});
