/**
 * `oxyc migrate-automations`, ported from the Rust `oxy migrate-automations`.
 *
 * The first three cases are the Rust tests, case for case. The rest pin what a
 * naive port gets wrong (every occurrence replaced, non-UTF-8 left alone, the
 * Rust skip list rather than `validate`'s) and the transcript and exit codes a
 * script can read.
 */

import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { CliError, ExitCode } from "../util/errors.js";
import {
  type MigrateIo,
  planRenames,
  rewriteText,
  runMigrateAutomations,
  targetPath
} from "./migrate-automations.js";

const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

function scratch(): string {
  const dir = mkdtempSync(join(tmpdir(), "oxyc-migrate-automations-"));
  SCRATCH.push(dir);
  return dir;
}

function write(root: string, name: string, content: string | Buffer): void {
  const path = join(root, name);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
}

/** Runs the command and keeps its transcript, one tagged line per call. */
function run(root: string, dryRun = false, path?: string) {
  const lines: string[] = [];
  const io: MigrateIo = {
    out: (l) => lines.push(`out: ${l}`),
    warn: (l) => lines.push(`warn: ${l}`),
    hint: (l) => lines.push(`hint: ${l}`)
  };
  const report = runMigrateAutomations({ path: path ?? root, dryRun }, root, io);
  return { report, lines };
}

function failure(fn: () => unknown): CliError {
  try {
    fn();
  } catch (e) {
    if (e instanceof CliError) return e;
    throw e;
  }
  throw new Error("expected a CliError");
}

describe("migrate-automations (the Rust tests)", () => {
  it("renames and rewrites references", () => {
    const root = scratch();
    write(root, "procedures/report.procedure.yml", "tasks: []\n");
    write(root, "workflows/sync.workflow.yml", "tasks: []\n");
    write(
      root,
      "agents/default.agentic.yml",
      "workflow_ref: procedures/report.procedure.yml\nincludes:\n  - workflows/*.workflow.yml\n"
    );

    run(root);

    expect(existsSync(join(root, "procedures/report.automation.yml"))).toBe(true);
    expect(existsSync(join(root, "workflows/sync.automation.yml"))).toBe(true);
    expect(existsSync(join(root, "procedures/report.procedure.yml"))).toBe(false);

    const agent = readFileSync(join(root, "agents/default.agentic.yml"), "utf8");
    expect(agent).toContain("procedures/report.automation.yml");
    expect(agent).toContain("workflows/*.automation.yml");
    expect(agent).not.toContain(".procedure.yml");
    expect(agent).not.toContain(".workflow.yml");
  });

  it("skips collisions without overwriting", () => {
    const root = scratch();
    // Both map to corr.automation.yml: a collision, and both are skipped.
    write(root, "corr.procedure.yml", "a: 1\n");
    write(root, "corr.workflow.yml", "b: 2\n");
    // A clean one renames fine.
    write(root, "solo.procedure.yml", "c: 3\n");

    run(root);

    expect(existsSync(join(root, "corr.procedure.yml"))).toBe(true);
    expect(existsSync(join(root, "corr.workflow.yml"))).toBe(true);
    expect(existsSync(join(root, "corr.automation.yml"))).toBe(false);
    expect(existsSync(join(root, "solo.automation.yml"))).toBe(true);
  });

  it("dry run changes nothing", () => {
    const root = scratch();
    write(root, "x.procedure.yml", "tasks: []\n");

    run(root, true);

    expect(existsSync(join(root, "x.procedure.yml"))).toBe(true);
    expect(existsSync(join(root, "x.automation.yml"))).toBe(false);
  });
});

describe("migrate-automations rules", () => {
  it("maps only the .yml legacy spellings", () => {
    expect(targetPath("/w/a/x.procedure.yml")).toBe("/w/a/x.automation.yml");
    expect(targetPath("/w/a/x.workflow.yml")).toBe("/w/a/x.automation.yml");
    expect(targetPath("/w/a/x.procedure.yaml")).toBeUndefined();
    expect(targetPath("/w/a/x.automation.yml")).toBeUndefined();
    expect(targetPath("/w/a/procedure.yml")).toBeUndefined();
  });

  it("treats an existing target as a collision, never overwriting it", () => {
    const root = scratch();
    write(root, "x.procedure.yml", "old: 1\n");
    write(root, "x.automation.yml", "new: 1\n");

    const { report } = run(root);

    expect(report.renames).toEqual([]);
    expect(report.collisions).toEqual([join(root, "x.procedure.yml")]);
    expect(readFileSync(join(root, "x.automation.yml"), "utf8")).toBe("new: 1\n");
    expect(readFileSync(join(root, "x.procedure.yml"), "utf8")).toBe("old: 1\n");
  });

  it("plans collisions from the file list alone", () => {
    const plan = planRenames(["/w/a.procedure.yml", "/w/a.workflow.yml", "/w/b.workflow.yml"]);
    expect(plan.renames).toEqual([{ from: "/w/b.workflow.yml", to: "/w/b.automation.yml" }]);
    expect(plan.collisions).toEqual(["/w/a.procedure.yml", "/w/a.workflow.yml"]);
  });

  it("replaces every occurrence in a file, not just the first", () => {
    expect(rewriteText("a.procedure.yml b.procedure.yml c.workflow.yml d.workflow.yml")).toBe(
      "a.automation.yml b.automation.yml c.automation.yml d.automation.yml"
    );
  });

  it("rewrites .yml, .yaml and .sql files, and nothing else", () => {
    const root = scratch();
    write(root, "x.procedure.yml", "tasks: []\n");
    write(root, "a.yml", "src: x.procedure.yml\n");
    write(root, "b.yaml", "src: x.procedure.yml\n");
    write(root, "c.sql", "-- x.procedure.yml\n");
    write(root, "d.md", "x.procedure.yml\n");

    const { report } = run(root);

    expect(readFileSync(join(root, "a.yml"), "utf8")).toBe("src: x.automation.yml\n");
    expect(readFileSync(join(root, "b.yaml"), "utf8")).toBe("src: x.automation.yml\n");
    expect(readFileSync(join(root, "c.sql"), "utf8")).toBe("-- x.automation.yml\n");
    expect(readFileSync(join(root, "d.md"), "utf8")).toBe("x.procedure.yml\n");
    expect(report.rewritten.map((p) => p.slice(root.length + 1))).toEqual([
      "a.yml",
      "b.yaml",
      "c.sql"
    ]);
  });

  it("leaves a file that is not UTF-8 byte for byte", () => {
    const root = scratch();
    write(root, "x.procedure.yml", "tasks: []\n");
    const bytes = Buffer.concat([Buffer.from("src: x.procedure.yml\n"), Buffer.from([0xff, 0xfe])]);
    write(root, "binary.yml", bytes);

    run(root);

    expect(readFileSync(join(root, "binary.yml")).equals(bytes)).toBe(true);
  });

  it("rewrites no reference when nothing was renamed", () => {
    const root = scratch();
    write(root, "x.automation.yml", "tasks: []\n");
    write(root, "agent.agentic.yml", "workflow_ref: gone.procedure.yml\n");

    const { lines } = run(root);

    expect(readFileSync(join(root, "agent.agentic.yml"), "utf8")).toBe(
      "workflow_ref: gone.procedure.yml\n"
    );
    expect(lines).toEqual([
      "out: No .procedure.yml / .workflow.yml files found — nothing to migrate."
    ]);
  });

  it("rewrites references on a run that also had collisions, including the collided file's", () => {
    // Kept from the Rust: the rewrite is a blind substring replace.
    const root = scratch();
    write(root, "c.procedure.yml", "a: 1\n");
    write(root, "c.automation.yml", "b: 2\n");
    write(root, "s.procedure.yml", "c: 3\n");
    write(root, "refs.yml", "- c.procedure.yml\n- s.procedure.yml\n");

    run(root);

    expect(readFileSync(join(root, "refs.yml"), "utf8")).toBe(
      "- c.automation.yml\n- s.automation.yml\n"
    );
  });

  it("skips the Rust skip list and dot-directories, but walks validate's extras", () => {
    const root = scratch();
    for (const dir of [".git", "target", "node_modules", "dist", "build", ".venv", ".hidden"]) {
      write(root, `${dir}/x.procedure.yml`, "tasks: []\n");
    }
    write(root, "out/kept.procedure.yml", "tasks: []\n");
    write(root, ".dotfile.procedure.yml", "tasks: []\n");

    const { report } = run(root);

    expect(report.renames.map((r) => r.from.slice(root.length + 1))).toEqual([
      ".dotfile.procedure.yml",
      "out/kept.procedure.yml"
    ]);
    expect(existsSync(join(root, "build/x.procedure.yml"))).toBe(true);
  });

  it("follows a symlinked directory, once", () => {
    const root = scratch();
    const outside = scratch();
    write(outside, "linked.procedure.yml", "tasks: []\n");
    symlinkSync(outside, join(root, "shared"));
    symlinkSync(root, join(root, "loop"));

    const { report } = run(root);

    expect(report.renames.map((r) => r.from.slice(root.length + 1))).toEqual([
      "shared/linked.procedure.yml"
    ]);
    expect(existsSync(join(outside, "linked.automation.yml"))).toBe(true);
  });
});

describe("migrate-automations transcript", () => {
  it("prints the dry-run plan on stdout", () => {
    const root = scratch();
    write(root, "procedures/a.procedure.yml", "tasks: []\n");
    write(root, "b.workflow.yml", "tasks: []\n");

    const { lines, report } = run(root, true);

    expect(lines).toEqual([
      "out: Planned renames:",
      "out:   b.workflow.yml -> b.automation.yml",
      "out:   procedures/a.procedure.yml -> procedures/a.automation.yml",
      "out: ",
      "out: Dry run: 2 file(s) would be renamed. Re-run without --dry-run to apply."
    ]);
    expect(report.rewritten).toEqual([]);
  });

  it("prints each rename, each rewrite and the summary; collisions as warnings", () => {
    const root = scratch();
    write(root, "c.procedure.yml", "a: 1\n");
    write(root, "c.workflow.yml", "b: 2\n");
    write(root, "s.procedure.yml", "c: 3\n");
    write(root, "refs.yml", "- s.procedure.yml\n");

    const { lines } = run(root);

    expect(lines).toEqual([
      "warn: naming collisions (skipped — target already exists or two files map to one name):",
      "hint: c.procedure.yml",
      "hint: c.workflow.yml",
      "out: renamed s.procedure.yml -> s.automation.yml",
      "out: updated refs in refs.yml",
      "out: ",
      "out: ✓ Renamed 1 file(s); updated references in 1 file(s).",
      "warn: skipped 2 file(s) due to naming collisions (listed above) — resolve them by hand."
    ]);
  });

  it("prints only the collision block when every legacy file collides, and exits 0", () => {
    const root = scratch();
    write(root, "c.procedure.yml", "a: 1\n");
    write(root, "c.automation.yml", "b: 2\n");

    const { lines, report } = run(root);

    expect(lines).toEqual([
      "warn: naming collisions (skipped — target already exists or two files map to one name):",
      "hint: c.procedure.yml"
    ]);
    expect(report.renames).toEqual([]);
  });

  it("resolves a relative path against cwd and defaults to it", () => {
    const root = scratch();
    write(root, "ws/x.procedure.yml", "tasks: []\n");

    const io: MigrateIo = { out: () => {}, warn: () => {}, hint: () => {} };
    runMigrateAutomations({ path: "ws" }, root, io);
    expect(existsSync(join(root, "ws/x.automation.yml"))).toBe(true);

    write(root, "ws/y.workflow.yml", "tasks: []\n");
    runMigrateAutomations({}, join(root, "ws"), io);
    expect(existsSync(join(root, "ws/y.automation.yml"))).toBe(true);
  });
});

describe("migrate-automations exit codes", () => {
  it("is NOT_FOUND (5) for a path that does not exist", () => {
    const root = scratch();
    const e = failure(() => run(root, false, join(root, "nope")));
    expect(e.code).toBe(ExitCode.NOT_FOUND);
    expect(e.code).toBe(5);
  });

  it("is USAGE (2) for a path that is a file", () => {
    const root = scratch();
    write(root, "x.procedure.yml", "tasks: []\n");
    const e = failure(() => run(root, false, join(root, "x.procedure.yml")));
    expect(e.code).toBe(ExitCode.USAGE);
    expect(e.code).toBe(2);
    expect(e.message).toContain("is not a directory");
  });
});
