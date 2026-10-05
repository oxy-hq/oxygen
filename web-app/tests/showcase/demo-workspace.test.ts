import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { describeDemoWorkspace, readDemoWorkspace } from "./demo-workspace";

function checkout(files: Record<string, string>): string {
  const dir = mkdtempSync(join(tmpdir(), "showcase-demo-"));
  for (const [path, body] of Object.entries(files)) {
    mkdirSync(join(dir, path, ".."), { recursive: true });
    writeFileSync(join(dir, path), body);
  }
  const git = (...args: string[]) => spawnSync("git", args, { cwd: dir, encoding: "utf-8" });
  git("init", "-q");
  git("add", "-A");
  return dir;
}

describe("readDemoWorkspace", () => {
  it("reads the databases and the files the product has a name for", () => {
    const dir = checkout({
      "config.yml": [
        "databases:",
        "  - name: airhouse",
        "    type: airhouse",
        "  - name: local",
        "    type: duckdb",
        "    dataset: .db/",
        "  - name: gone",
        "    type: duckdb",
        "    dataset: .missing/"
      ].join("\n"),
      ".db/orders.csv": "",
      "procedures/active_users.automation.yml": "",
      "old/legacy.procedure.yml": "",
      "agents/default.agent.yml": "",
      "notes.md": "",
      "data/orders.csv": ""
    });
    const demo = readDemoWorkspace(dir);
    expect(demo.databases).toEqual([
      { name: "airhouse", type: "airhouse", hasData: false },
      { name: "local", type: "duckdb", hasData: true },
      { name: "gone", type: "duckdb", hasData: false }
    ]);
    expect(demo.files).toEqual([
      { kind: "Agents", paths: ["agents/default.agent.yml"] },
      {
        kind: "Automations",
        paths: ["old/legacy.procedure.yml", "procedures/active_users.automation.yml"]
      }
    ]);
  });
  it("knows nothing about a directory that is not there", () => {
    expect(readDemoWorkspace(join(tmpdir(), "showcase-demo-absent"))).toEqual({
      databases: [],
      files: []
    });
  });
});

describe("describeDemoWorkspace", () => {
  it("says which databases answer on a throwaway instance", () => {
    const text = describeDemoWorkspace({
      databases: [
        { name: "airhouse", type: "airhouse", hasData: false },
        { name: "local", type: "duckdb", hasData: true }
      ],
      files: []
    });
    expect(text).toMatch(/Only the file-backed ones hold data on this instance \(`local`\)/);
    expect(text).toContain("- `airhouse` (airhouse)");
  });
  it("names a long kind by its first paths and a count", () => {
    const paths = Array.from(
      { length: 34 },
      (_, i) => `views/v${String(i).padStart(2, "0")}.view.yml`
    );
    const text = describeDemoWorkspace({
      databases: [],
      files: [{ kind: "Semantic views", paths }]
    });
    expect(text).toContain("Semantic views (34):");
    expect(text).toContain("- views/v29.view.yml");
    expect(text).not.toContain("views/v30.view.yml");
    expect(text).toContain("- … and 4 more");
  });
  it("is empty when nothing is known, so the prompt carries no empty section", () => {
    expect(describeDemoWorkspace({ databases: [], files: [] })).toBe("");
  });
});
