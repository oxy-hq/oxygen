/**
 * Wires `oxyc evals …` onto the program, split out of `main.ts` for the same
 * reason `airway-cli.ts` is. The Rust `oxy test` still runs evals on a local
 * checkout, as dev tooling moving under `oxy dev` (`internal-docs/oxy-cli-to-oxyc.md`,
 * steps 12 and 13).
 */

import type { Command } from "commander";
import { createContext } from "../context/resolve.js";
import { usageError } from "../util/errors.js";
import { type EvalFlags, runEvals, type ThresholdMode, waitEvals } from "./evals.js";
import type { Wiring } from "./flags-cli.js";

type Opts = Record<string, unknown>;

const HELP = `
Both verbs need --workspace <id>.

  oxyc evals run tests/sales.agentic.test.yml --workspace <id>         run one test file, wait, report
  oxyc evals run --workspace <id> --min-accuracy 0.8                  every test file; exit 9 under 0.8
  oxyc evals run <file> --case "total revenue" --output-json          one case; write <file>.results.json
  oxyc evals run <file> --regrade tests/x.test.results.json           grade recorded outputs again
  oxyc evals run <file> --detach                                      queue it and print the run id
  oxyc evals wait <run-id> --json                                     wait for a queued run

The deployment runs the eval on its worker fleet, against the test files and agents the
workspace has COMPILED (its promoted revision) — not files on this machine. A new or edited
.test.yml runs once it is pushed.

--output-json and --json carry the same document \`oxy test --output-json\` wrote, byte for
byte (skill oxy-evals, references/results-json.md).

Exit codes: 1 the run failed, was cancelled or timed out; 2 a bad flag; 4 not signed in, or
not a member of the workspace; 5 no such workspace or run, or the test file is not in the
served revision; 6 the server refused the request (the test file does not parse, a --case
that matches nothing); 7 retryable (the compiled revision could not be read, or --wait ran
out — the run goes on); 9 accuracy below --min-accuracy.`;

function threshold(value: unknown): ThresholdMode {
  const mode = typeof value === "string" ? value : "average";
  if (mode !== "average" && mode !== "all") {
    throw usageError(`--threshold-mode takes average or all, got: ${mode}`);
  }
  return mode;
}

function number(value: unknown, flag: string): number | undefined {
  if (value === undefined) return undefined;
  const n = Number(value);
  if (!Number.isFinite(n))
    throw usageError(`${flag} takes a number, got: ${JSON.stringify(value)}`);
  return n;
}

function flagsOf(opts: Opts): EvalFlags {
  return {
    case: opts.case as string | undefined,
    tag: opts.tag as string | undefined,
    regrade: opts.regrade as string | undefined,
    minAccuracy: number(opts.minAccuracy, "--min-accuracy"),
    thresholdMode: threshold(opts.thresholdMode),
    // A bare `--wait` waits to the end, which `run` does anyway; a number caps it.
    waitSeconds: opts.wait === true ? undefined : number(opts.wait, "--wait"),
    detach: Boolean(opts.detach),
    json: Boolean(opts.json),
    outputJson: opts.outputJson as boolean | string | undefined
  };
}

function withGate(command: Command): Command {
  return command
    .option("--min-accuracy <threshold>", "exit 9 when accuracy is below this (0.0-1.0)")
    .option(
      "--threshold-mode <mode>",
      "average: the mean of the test files' accuracies must meet it; all: each must",
      "average"
    )
    .option(
      "--wait [seconds]",
      "wait for the run to finish (the default); with seconds, stop waiting after that long (exit 7) — the run goes on"
    )
    .option("--json", "print the results document instead of the summary")
    .option(
      "--output-json [file]",
      "write the results document (default: the test file with .yml → .results.json, or test-results.json)"
    );
}

export function registerEvals(program: Command, w: Wiring): void {
  const evals = program
    .command("evals")
    .description(
      "LLM-judged evals over .test.yml, run on the deployment (`oxy test` runs them on a checkout)"
    )
    .addHelpText("after", HELP);

  w.withGlobals(
    withGate(
      evals
        .command("run [test_file]")
        .description(
          "run a .test.yml (or every one the workspace has compiled) on the deployment and report"
        )
        .option("--case <case>", "only this case: a 0-based index, its name, or its prompt")
        .option("--tag <tag>", "only the cases with this tag")
        .option(
          "--regrade <results.json>",
          "grade the outputs a results file recorded with the current grader, without running the agent"
        )
        .option("--detach", "queue the run, print its id, and return")
    )
  ).action(async (file: string | undefined, opts: Opts) => {
    await runEvals(createContext(w.globals(opts)), file, flagsOf(opts));
  });

  w.withGlobals(
    withGate(evals.command("wait <run_id>").description("wait for a queued eval run and report it"))
  ).action(async (runId: string, opts: Opts) => {
    await waitEvals(createContext(w.globals(opts)), runId, flagsOf(opts));
  });
}
