/**
 * Which workspace each file belongs to, and so which `config.yml` its
 * references resolve against.
 *
 * A workspace is the directory holding a `config.yml`, as for `oxy`: the Rust
 * command climbed to the nearest one and resolved every file below it against
 * it. Workspaces do not nest, so a `config.yml` INSIDE one is not a
 * workspace's (`examples/modeling/ecommerce-analytics/config.yaml` is an
 * Airform project's): it is neither checked against the workspace schema nor
 * an owner, and a note says so.
 */

import type { Finding } from "./finding.js";
import type { WorkspaceConfig } from "./references.js";

/** Is this path a workspace config (either spelling)? Matched on the basename. */
export function isConfig(rel: string): boolean {
  return /(^|\/)config\.ya?ml$/.test(rel);
}

/** The kinds that live in a workspace and read its `config.yml`. */
export const WORKSPACE_SCHEMAS = new Set([
  "workflow.json",
  "agentic.json",
  "view.json",
  "topic.json"
]);

export interface References {
  ownerOf(rel: string): string | undefined;
  /** A config that loaded → its targets; one that did not → `undefined`. */
  loaded: Map<string, WorkspaceConfig | undefined>;
}

/** The walk's files, with the workspaces they belong to. */
export interface Plan {
  /** What to check, nested configs removed. */
  files: string[];
  /** The owning configs, checked first so references have one to resolve against. */
  roots: string[];
  refs: References;
  notes: Set<string>;
}

function dirOf(rel: string): string {
  const d = rel.includes("/") ? rel.slice(0, rel.lastIndexOf("/")) : "";
  return d === "." ? "" : d;
}

/** Is directory `dir` the same as, or above, `rel`'s directory? */
function contains(dir: string, rel: string): boolean {
  return dir === "" || rel.startsWith(`${dir}/`);
}

function strictlyAbove(above: string, below: string): boolean {
  return above !== below && (above === "" || below.startsWith(`${above}/`));
}

/**
 * Plan a run. Walking, the outermost configs are the workspaces. With
 * `--file`, `owner` is the one config above the directory `oxyc` runs in, as
 * `oxy` found it — which may sit above that directory, so it owns the file
 * whatever their paths say.
 */
export function planRun(walked: string[], owner?: string[]): Plan {
  const notes = new Set<string>();
  if (owner) return { files: walked, roots: owner, refs: references(owner, true), notes };
  const configs = walked.filter(isConfig);
  const roots = configs.filter((c) => !configs.some((o) => strictlyAbove(dirOf(o), dirOf(c))));
  const nested = configs.filter((c) => !roots.includes(c));
  for (const n of nested) {
    notes.add(`${n}: not checked as a workspace config.yml — it sits inside another workspace`);
  }
  return {
    files: walked.filter((f) => !nested.includes(f)),
    roots,
    refs: references(roots, false),
    notes
  };
}

function references(roots: string[], single: boolean): References {
  return {
    loaded: new Map(),
    ownerOf(rel) {
      if (single) return roots[0];
      // `config.yml` before `config.yaml` in one directory: `oxy` reads the first.
      const owners = roots.filter((r) => contains(dirOf(r), rel));
      return owners.find((r) => r.endsWith(".yml")) ?? owners[0];
    }
  };
}

/**
 * Workspace files with no `config.yml` above them: `oxy` did not start
 * ("Could not find config.yml"), and nothing they reference can resolve.
 * Among CHECKED files only: one that was not read says nothing about its
 * config, and a run that read nothing keeps the verdict that gives it.
 */
export function missingConfig(
  checked: string[],
  schemaFor: (rel: string) => string | undefined,
  refs: References
): Finding[] {
  const orphans = checked.filter(
    (f) => WORKSPACE_SCHEMAS.has(schemaFor(f) ?? "") && !refs.ownerOf(f)
  );
  if (orphans.length === 0) return [];
  const more = orphans.length > 1 ? ` and ${orphans.length - 1} more` : "";
  return [
    {
      file: "config.yml",
      path: "(root)",
      kind: "missing",
      message:
        `no config.yml above ${orphans[0]}${more}: a workspace is the directory holding one, ` +
        "and its databases, models and integrations are what these files reference"
    }
  ];
}

/** What checking every planned file produced. */
export interface Outcome<S extends string> {
  checked: string[];
  unchecked: Array<{ path: string; code: S }>;
  findings: Finding[];
}

/**
 * Check every planned file, configs first, and report in path order. A config
 * `--file` did not name is read for its references and reported only when it
 * does not load — `oxy` stopped there too.
 */
export function checkPlanned<S extends string>(
  plan: Plan,
  check: (rel: string) => Finding[] | S
): Outcome<S> {
  const out: Outcome<S> = { checked: [], unchecked: [], findings: [] };
  const byFile = new Map<string, Finding[]>();
  for (const rel of [...plan.roots, ...plan.files.filter((f) => !plan.roots.includes(f))]) {
    const result = check(rel);
    const named = plan.files.includes(rel);
    if (typeof result === "string") {
      if (named) out.unchecked.push({ path: rel, code: result });
      continue;
    }
    if (named) out.checked.push(rel);
    if (named || result.length > 0) byFile.set(rel, result);
  }
  out.checked.sort((a, b) => a.localeCompare(b));
  out.findings = [...byFile.keys()]
    .sort((a, b) => a.localeCompare(b))
    .flatMap((rel) => byFile.get(rel) ?? []);
  return out;
}
