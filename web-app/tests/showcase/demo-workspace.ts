// What the Demo workspace holds, read from the examples directory it is seeded
// from, so a plan names an automation, an agent or a database that exists
// instead of one the planner assumed. Names and paths only.
//
// Two plans that guessed, on real PRs: one opened "the first automation" to
// show an agent step's form and found a SQL-only one; one ran its query on the
// connection the SQL IDE opens with, which needs a warehouse the instance does
// not have, and filmed the connection error.

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { parse } from "yaml";

export interface DemoWorkspace {
  /** `hasData`: its data is files in the workspace, so it answers on an instance with no warehouse. */
  databases: { name: string; type: string; hasData: boolean }[];
  /** Workspace-relative paths, grouped by what the product calls them. */
  files: { kind: string; paths: string[] }[];
}

// The kinds a reader meets in the product, by the file suffix that makes one.
const KINDS: [kind: string, suffix: RegExp][] = [
  ["Agents", /\.agent\.yml$/],
  ["Agentic agents", /\.agentic\.yml$/],
  ["Automations", /\.(automation|procedure)\.yml$/],
  ["Data apps", /\.app\.yml$/],
  ["Semantic views", /\.view\.yml$/],
  ["Topics", /\.topic\.yml$/],
  ["Monitors", /\.monitor\.yml$/],
  ["Pipelines", /\.airway\.yml$/]
];
// A kind past this is named by its first paths and a count: the planner needs
// something real to open, not the whole tree.
const PER_KIND = 30;
// A database of this type over a `dataset` directory is files in the
// workspace. Every other one is a service or a credential a throwaway instance
// does not have.
const FILE_BACKED_TYPE = "duckdb";

function trackedFiles(dir: string): string[] {
  const res = spawnSync("git", ["ls-files"], { cwd: dir, encoding: "utf-8" });
  return res.status === 0 ? res.stdout.split("\n").filter(Boolean) : [];
}

function databases(dir: string): DemoWorkspace["databases"] {
  const config = join(dir, "config.yml");
  if (!existsSync(config)) return [];
  const parsed = parse(readFileSync(config, "utf-8")) as {
    databases?: { name?: unknown; type?: unknown; dataset?: unknown }[];
  } | null;
  return (parsed?.databases ?? []).flatMap((d) => {
    if (typeof d.name !== "string" || typeof d.type !== "string") return [];
    const hasData =
      d.type === FILE_BACKED_TYPE &&
      typeof d.dataset === "string" &&
      existsSync(join(dir, d.dataset));
    return [{ name: d.name, type: d.type, hasData }];
  });
}

/** Empty when `dir` is missing or not a checkout: the planner is then told nothing rather than something wrong. */
export function readDemoWorkspace(dir: string): DemoWorkspace {
  if (!existsSync(dir)) return { databases: [], files: [] };
  const tracked = trackedFiles(dir);
  const files = KINDS.map(([kind, suffix]) => ({
    kind,
    paths: tracked.filter((f) => suffix.test(f)).sort()
  })).filter((group) => group.paths.length > 0);
  return { databases: databases(dir), files };
}

/** The workspace as the planner reads it; an empty string when nothing is known. */
export function describeDemoWorkspace(demo: DemoWorkspace): string {
  const lines: string[] = [];
  if (demo.databases.length) {
    const answers = demo.databases.filter((d) => d.hasData).map((d) => `\`${d.name}\``);
    lines.push(
      "Databases in its config. Only the file-backed ones hold data on this instance" +
        ` (${answers.length ? answers.join(", ") : "none"}); a query on any other fails with a ` +
        "connection error, so a plan that runs a query first picks one of those:",
      ...demo.databases.map((d) => `- \`${d.name}\` (${d.type})`)
    );
  }
  for (const group of demo.files) {
    const shown = group.paths.slice(0, PER_KIND);
    const more = group.paths.length - shown.length;
    lines.push(
      `${group.kind} (${group.paths.length}):`,
      ...shown.map((p) => `- ${p}`),
      ...(more > 0 ? [`- … and ${more} more`] : [])
    );
  }
  return lines.join("\n");
}
