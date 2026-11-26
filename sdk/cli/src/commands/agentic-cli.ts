/**
 * Wires `oxyc agentic …` onto the program, split out of `main.ts` for the same
 * reason `airway-cli.ts` is. The Rust `oxy agentic` became a one-release stub
 * that printed these commands when this landed, deleted 2026-11-26
 * (`internal-docs/oxy-cli-to-oxyc.md`, step 9).
 */

import type { Command } from "commander";
import { createContext } from "../context/resolve.js";
import {
  runAgenticAnswer,
  runAgenticCancel,
  runAgenticEvents,
  runAgenticRun,
  THINKING_MODES
} from "./agentic-run.js";
import { runAgenticInspect, runAgenticList, runAgenticStatus } from "./agentic-runs.js";
import type { Wiring } from "./flags-cli.js";

type Opts = Record<string, unknown>;

const HELP = `
Every verb needs --workspace <id>: agents and their runs belong to a workspace.

  oxyc agentic run agents/sales.agentic.yml -q "Revenue by region last quarter?" --workspace <id>
  oxyc agentic run agents/sales -q "..." --json        one {seq, event_type, payload} line per event
  oxyc agentic answer <run_id> --answer "Q3 2026"      answer the question a run stopped on
  oxyc agentic events <run_id> [--after-seq 12]        replay a run, and follow it if it is live
  oxyc agentic cancel <run_id>
  oxyc agentic list | inspect <run_id> | status [--active] [--watch 5]

The server runs the agent it has COMPILED for the workspace's served revision, with the
workspace's own secrets and LLM keys — not a .agentic.yml in this checkout. Push a change
before running it. Each run is a chat: it opens a thread in your history, as the web app does.
list, inspect and status read the Orchestrator Dashboard, which is for org owners and admins,
Oxy staff, and members Oxy granted the workspace.

Exit codes: 1 the run ended with error or cancelled (run only; a run that asks a question exits
0, waiting for answer); 2 a bad flag; 4 not signed in, or not allowed; 5 no such workspace or
run; 6 the server refused the request (the agent could not start, or the run is not waiting
for an answer); 7 retryable (the workspace is not compiled yet, or the stream kept closing).`;

export function registerAgentic(program: Command, w: Wiring): void {
  const agentic = program
    .command("agentic")
    .description("agentic agents on a deployment: run one, follow, answer, cancel and read runs")
    .addHelpText("after", HELP);
  registerRunVerbs(agentic, w);
  registerReadVerbs(agentic, w);
}

/** `run | events | answer | cancel`: the chat's routes, any workspace member. */
function registerRunVerbs(agentic: Command, w: Wiring): void {
  w.withGlobals(
    agentic
      .command("run <agent>")
      .description(
        "ask the deployed agent (its path in the workspace) and stream the run to the end"
      )
      .requiredOption("-q, --question <text>", "the question to ask")
      .option("--thinking-mode <mode>", `${THINKING_MODES.join(" or ")}`, "auto")
      .option("--json", "one JSON object per event: {seq, event_type, payload}")
  ).action(async (agent: string, opts: Opts) => {
    await runAgenticRun(createContext(w.globals(opts)), agent, {
      question: (opts.question as string | undefined) ?? "",
      thinkingMode: (opts.thinkingMode as string | undefined) ?? "auto",
      json: Boolean(opts.json)
    });
  });

  w.withGlobals(
    agentic
      .command("events <run_id>")
      .description("replay a run's events; a live run is followed to its end or its next question")
      .option("--after-seq <n>", "only the events after this seq", "-1")
      .option("--json", "one JSON object per event: {seq, event_type, payload}")
  ).action(async (runId: string, opts: Opts) => {
    await runAgenticEvents(createContext(w.globals(opts)), runId, {
      afterSeq: Number(opts.afterSeq),
      json: Boolean(opts.json)
    });
  });

  w.withGlobals(
    agentic
      .command("answer <run_id>")
      .description("answer the question a suspended run asked; the run resumes on the deployment")
      .requiredOption("-a, --answer <text>", "the answer")
  ).action(async (runId: string, opts: Opts) => {
    await runAgenticAnswer(
      createContext(w.globals(opts)),
      runId,
      (opts.answer as string | undefined) ?? ""
    );
  });

  w.withGlobals(
    agentic.command("cancel <run_id>").description("cancel a running or suspended run")
  ).action(async (runId: string, opts: Opts) => {
    await runAgenticCancel(createContext(w.globals(opts)), runId);
  });
}

/** `list | inspect | status`: the Orchestrator Dashboard's reads. */
function registerReadVerbs(agentic: Command, w: Wiring): void {
  w.withGlobals(
    agentic
      .command("list")
      .description("the workspace's recent runs")
      .option("--limit <n>", "how many (1-200)", "20")
      .option("--json", "a JSON array of {run_id, status, source_type, question, created_at}")
  ).action(async (opts: Opts) => {
    await runAgenticList(createContext(w.globals(opts)), {
      limit: Number(opts.limit),
      json: Boolean(opts.json)
    });
  });

  w.withGlobals(
    agentic
      .command("inspect <run_id>")
      .description("one run's details: status, agent, question, answer or error, attempt")
      .option("--json", "emit the details as JSON")
  ).action(async (runId: string, opts: Opts) => {
    await runAgenticInspect(createContext(w.globals(opts)), runId, { json: Boolean(opts.json) });
  });

  w.withGlobals(
    agentic
      .command("status")
      .description("recent (or only active) runs, each with its current stage and delegations")
      .option("--active", "only runs still going: running, suspended, delegating, resuming")
      .option("-w, --watch <seconds>", "refresh every N seconds, at least 2 (0 = once)", "0")
      .option("--json", "emit the rows as JSON")
  ).action(async (opts: Opts) => {
    await runAgenticStatus(createContext(w.globals(opts)), {
      watch: Number(opts.watch),
      active: Boolean(opts.active),
      json: Boolean(opts.json)
    });
  });
}
