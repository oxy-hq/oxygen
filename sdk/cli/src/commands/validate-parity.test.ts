/**
 * `oxyc validate` against the verdicts of the Rust `oxy validate` it replaced.
 *
 * THE CORPUS. `fixtures/validate-parity/rust-verdicts.json` holds what the Rust
 * binary said, run for real (not read off the code) in each directory, before
 * it became a stub:
 *
 * - `cases/` — one small workspace per error the Rust command could report
 *   (each garde rule, each reference, each semantic check, each parse class),
 *   and one per near-miss it ACCEPTED (an unknown task `type:`, `avg`, a
 *   `default_filters` entry, an empty `config.yml`), because a replacement that
 *   rejects what the old command accepted breaks a workspace that worked.
 * - the repo's real workspaces — `examples/`, the fixtures the Rust
 *   integration test spawned the binary over, the template, the BIRD evals —
 *   FROZEN under `workspaces/` at the paths they had: every file `oxyc validate`
 *   reads from them (config, automations, agents, tests, views, topics), copied
 *   on the day the verdicts were recorded (four comments naming private server
 *   source files reworded; no check reads a comment). The test owns its inputs, so an edit
 *   to `examples/` cannot turn it red, and the Rust oracle is gone.
 *
 * THE RULE. For every directory, the set of `(file, kind)` that fail is the
 * same as Rust's, no more and no fewer, and the exit code is `9` exactly when
 * that set is non-empty (Rust's was `1`). Where oxyc deliberately says
 * something different, `DIVERGES` names the directory, what oxyc says instead
 * and why — and the test checks the recorded verdict really does differ, so
 * an entry cannot sit there hiding an agreement.
 *
 * NOTHING RE-RECORDS. `oxy validate` is a stub now. A change to these fixtures
 * or to the recorded verdicts is a change to the oracle: say why in the PR.
 */

import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import { CLI_KILL_TIMEOUT_MS, CLI_TEST_TIMEOUT_MS, cliRunError } from "../testing/cli-child.js";
import { ExitCode } from "../util/errors.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const PKG = resolve(HERE, "..", "..");
const BIN = join(PKG, "dist", "main.mjs");
const PARITY = join(PKG, "fixtures", "validate-parity");

vi.setConfig({ testTimeout: CLI_TEST_TIMEOUT_MS });

interface Failure {
  file: string;
  kind: string;
}

interface Recorded {
  exit: number;
  failures: Array<Failure & { rust: string }>;
}

const RECORDED = JSON.parse(readFileSync(join(PARITY, "rust-verdicts.json"), "utf8")) as {
  cases: Record<string, Recorded>;
  workspaces: Record<string, Recorded>;
};

/** Where oxyc answers differently on purpose: what it says, and why. */
const DIVERGES: Record<string, { oxyc: Failure[]; why: string }> = {
  "agentic-db-missing": {
    oxyc: [{ file: "a.agentic.yml", kind: "ref" }],
    why:
      "STRICTER. Rust parsed an agentic agent and stopped (`AgentConfig::from_file`); the " +
      "plan asked oxyc to resolve `databases:`. At run time an unknown name is a warning and " +
      "an agent that cannot query."
  },
  "agentic-llm-ref-missing": {
    oxyc: [{ file: "a.agentic.yml", kind: "ref" }],
    why: "STRICTER, as above, for `llm.ref` against `config.yml`'s `models:`."
  },
  "view-outside-semantics": {
    oxyc: [{ file: "models/v.view.yml", kind: "parse" }],
    why:
      "STRICTER. Rust read views only from `semantics/views/` and `semantics/topics/`; the " +
      "compiler (`oxy-compile`'s walker) compiles a `.view.yml` anywhere, so oxyc checks it there."
  },
  "config-missing-nothing": {
    oxyc: [],
    why:
      'A directory with no config.yml and nothing to check: Rust failed to start ("Could not ' +
      'find config.yml"); oxyc reports nothing to check and exits 0, as it does for a ' +
      "custom-app repo with no workspace files."
  },
  "auto-in-build-dir": {
    oxyc: [],
    why:
      "Rust walked every directory; oxyc skips `build/`, `node_modules/`, `target/` and the " +
      "like, as the compiler's walker does — a stray copy there is never served."
  }
};

function oxycValidate(cwd: string, args: string[] = []) {
  if (!existsSync(BIN)) throw new Error(`${BIN} missing — run \`pnpm build\``);
  const r = spawnSync(process.execPath, [BIN, "validate", "--json", ...args], {
    cwd,
    encoding: "utf8",
    env: {
      ...process.env,
      OXYC_SCHEMAS_DIR: join(PKG, "json-schemas"),
      OXY_CREDENTIALS_PATH: join(BIN, "..", "__no_creds__.json"),
      OXYC_CACHE_DIR: join(BIN, "..", "__no_cache__"),
      OXY_TOKEN: "",
      NO_COLOR: "1"
    },
    timeout: CLI_KILL_TIMEOUT_MS
  });
  if (r.error || r.signal)
    throw cliRunError(["validate", "--json"], r.error?.message ?? `${r.signal}`);
  return { status: r.status ?? -1, stdout: r.stdout ?? "", stderr: r.stderr ?? "" };
}

function key(f: Failure): string {
  return `${f.file} [${f.kind}]`;
}

/** The distinct `(file, kind)` pairs, sorted, as one comparable list. */
function failing(list: Failure[]): string[] {
  return [...new Set(list.map(key))].sort();
}

function expectParity(name: string, dir: string, recorded: Recorded): void {
  const expected = DIVERGES[name]?.oxyc ?? recorded.failures;
  const r = oxycValidate(dir);
  const report = JSON.parse(r.stdout) as { findings: Failure[] };
  expect(failing(report.findings), r.stderr).toEqual(failing(expected));
  expect(r.status, r.stderr).toBe(expected.length > 0 ? ExitCode.CHECK_FAILED : 0);
}

describe("every case answers as the Rust oxy validate did", () => {
  const cases = readdirSync(join(PARITY, "cases")).sort();

  it("has a recorded Rust verdict for every case directory, and no other", () => {
    expect(Object.keys(RECORDED.cases).sort()).toEqual(cases);
  });

  it.each(cases)("%s", (name) => {
    const recorded = RECORDED.cases[name];
    if (!recorded) throw new Error(`no recorded verdict for ${name}`);
    expectParity(name, join(PARITY, "cases", name), recorded);
  });
});

describe("every real workspace answers as the Rust oxy validate did", () => {
  it.each(Object.keys(RECORDED.workspaces))("%s", (ws) => {
    const recorded = RECORDED.workspaces[ws];
    if (!recorded) throw new Error(`no recorded verdict for ${ws}`);
    expectParity(ws, join(PARITY, "workspaces", ws), recorded);
  });
});

describe("the divergences are real", () => {
  /**
   * An entry for a directory where oxyc and Rust in fact agree would be a
   * silent exemption: it states a difference, and the comparison above would
   * then never notice if the two drifted apart in a new way.
   */
  it.each(Object.keys(DIVERGES))("%s differs from what Rust recorded", (name) => {
    const recorded = RECORDED.cases[name] ?? RECORDED.workspaces[name];
    if (!recorded) throw new Error(`${name} is not in the corpus`);
    expect(failing(DIVERGES[name]?.oxyc ?? [])).not.toEqual(failing(recorded.failures));
  });
});

/**
 * PORTED FROM the `oxy` crate's integration test `validation`, which spawned the
 * Rust binary over these two fixtures; that file now pins the stub instead.
 * Both are also in the corpus above — named here so the port is traceable.
 */
describe("validation.rs, ported", () => {
  it("ok_on_valid_config: the example fixture is valid", () => {
    const r = oxycValidate(join(PARITY, "workspaces", "tests", "fixtures", "oxy_example"));
    expect(r.status, r.stderr).toBe(0);
  });

  it("failed_on_invalid_config: a config.yml the runtime cannot load fails the check", () => {
    const r = oxycValidate(
      join(PARITY, "workspaces", "crates", "core", "tests", "fixtures", "invalid_config")
    );
    expect(r.status, r.stderr).toBe(ExitCode.CHECK_FAILED);
    const report = JSON.parse(r.stdout) as { findings: Failure[] };
    expect(failing(report.findings)).toEqual(["config.yml [parse]"]);
  });
});

/**
 * `--file` reads the config above the cwd only for a kind that resolves
 * references against it. `oxy-app.json` and `*.agent.test.yml` never did, so a
 * broken `config.yml` beside them is not their finding.
 */
describe("--file and the owning config", () => {
  it("does not load config.yml for a kind that reads none", () => {
    const dir = mkdtempSync(join(tmpdir(), "oxyc-parity-"));
    writeFileSync(join(dir, "config.yml"), "not_a_field: 1\n");
    writeFileSync(join(dir, "t.agent.test.yml"), "cases: []\n");
    const r = oxycValidate(dir, ["--file", "t.agent.test.yml"]);
    expect(r.status, r.stderr).toBe(0);
  });

  it("does load it for an automation, and reports it when it does not load", () => {
    const dir = mkdtempSync(join(tmpdir(), "oxyc-parity-"));
    writeFileSync(join(dir, "config.yml"), "not_a_field: 1\n");
    writeFileSync(
      join(dir, "a.automation.yml"),
      "tasks:\n  - name: s\n    type: formatter\n    template: hi\n"
    );
    const r = oxycValidate(dir, ["--file", "a.automation.yml"]);
    expect(r.status, r.stderr).toBe(ExitCode.CHECK_FAILED);
    expect(failing((JSON.parse(r.stdout) as { findings: Failure[] }).findings)).toEqual([
      "config.yml [parse]"
    ]);
  });
});
