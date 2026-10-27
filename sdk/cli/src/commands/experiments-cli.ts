/**
 * Wires `oxyc experiments …` onto the program. Kept out of `main.ts`, which
 * already holds every other group: this one has fifteen verbs.
 */

import type { Command } from "commander";
import { createContext, type GlobalFlags } from "../context/resolve.js";
import {
  runExperimentsAbort,
  runExperimentsCreate,
  runExperimentsDesign,
  runExperimentsDuplicate,
  runExperimentsEdit,
  runExperimentsInterim,
  runExperimentsList,
  runExperimentsPrice,
  runExperimentsProposals,
  runExperimentsRegister,
  runExperimentsResult,
  runExperimentsStatus,
  runExperimentsUse,
  runExperimentsWriteBack
} from "./experiments.js";

type Opts = Record<string, unknown>;

export interface Wiring {
  withGlobals: (command: Command) => Command;
  globals: (opts: Opts) => GlobalFlags;
  collect: (value: string, previous: string[]) => string[];
}

const HELP = `
Every verb needs the workspace: --workspace <id> (\`oxyc api {org}/workspaces\` lists them).

LIFECYCLE
  oxyc experiments create --file promo.experiment.yml      a draft
  oxyc experiments price promo --candidate '{shape: waves, wave_sizes: [4, 2], first_switch_on: 2026-11-02, spacing_days: 14}' --wait
  oxyc experiments use promo <proposal-id>                  pricing's keys onto the draft
  oxyc experiments register promo --yes                     freeze it; it becomes the run
  oxyc experiments status promo                             flips, guardrails, compliance
  oxyc experiments interim promo [--refresh --wait]         the reading so far, badged not final
  oxyc experiments result promo                             sealed until reported
  oxyc experiments abort promo --reason "…" [--levers keep] --yes

  Windows, spacing and periods count periods of design.grain (day by default; hour,
  week, month). At week grain a date is a Monday, at month grain the 1st, at hour
  grain a local whole hour: first_switch_on: 2026-10-26T14:00.

  A registered design never changes: \`duplicate\` it, edit the copy.
  Past dates only? \`retrospective\` analyses the draft as its one run.`;

export function registerExperiments(program: Command, w: Wiring): void {
  const exp = program
    .command("experiments")
    .alias("exp")
    .description("an experiment's lifecycle — draft, price, register, watch, abort, report")
    .addHelpText("after", HELP);

  const verb = (spec: string, description: string) =>
    w
      .withGlobals(exp.command(spec).description(description))
      .option("--json", "emit the server's response");
  const ctx = (opts: Opts) => createContext(w.globals(opts));
  const json = (opts: Opts) => Boolean(opts.json);

  verb("list", "every experiment in the workspace, with its status").action(async (opts: Opts) => {
    await runExperimentsList(ctx(opts), { json: json(opts) });
  });

  verb("design <name>", "the design's config as YAML — the file create/edit read")
    .option("--full", "the whole design record (hash, frozen_at, …) as JSON")
    .action(async (name: string, opts: Opts) => {
      await runExperimentsDesign(ctx(opts), name, { json: json(opts), full: Boolean(opts.full) });
    });

  verb("create", "a new experiment and its draft, from a design file")
    .option("--file <path|->", "the design: YAML or JSON, `-` for stdin")
    .action(async (opts: Opts) => {
      await runExperimentsCreate(ctx(opts), {
        file: opts.file as string | undefined,
        json: json(opts)
      });
    });

  verb("edit <name>", "overwrite the draft from a design file (a frozen design refuses)")
    .option("--file <path|->", "the design: YAML or JSON, `-` for stdin")
    .option("--base-hash <hash>", "the hash you read; default: read it now")
    .action(async (name: string, opts: Opts) => {
      await runExperimentsEdit(ctx(opts), name, {
        file: opts.file as string | undefined,
        baseHash: opts.baseHash as string | undefined,
        json: json(opts)
      });
    });

  verb(
    "duplicate <name> <new-name>",
    "a new draft from this design — how a registered one is changed"
  ).action(async (name: string, newName: string, opts: Opts) => {
    await runExperimentsDuplicate(ctx(opts), name, newName, { json: json(opts) });
  });

  verb("price <name>", "price 1–6 candidate designs against the draft")
    .option(
      "--candidate <yaml|json|@file>",
      "one design (repeatable); a list or {candidates} adds each",
      w.collect,
      []
    )
    .option("--wait", "poll until every candidate is priced, refused or failed")
    .option("--timeout <seconds>", "how long --wait polls", "600")
    .action(async (name: string, opts: Opts) => {
      await runExperimentsPrice(ctx(opts), name, {
        candidate: opts.candidate as string[],
        wait: Boolean(opts.wait),
        timeoutSec: Number(opts.timeout),
        json: json(opts)
      });
    });

  verb("proposals <name>", "every candidate ever priced, newest first").action(
    async (name: string, opts: Opts) => {
      await runExperimentsProposals(ctx(opts), name, { json: json(opts) });
    }
  );

  verb("use <name> <proposal-id>", "write a priced proposal's keys onto the draft").action(
    async (name: string, id: string, opts: Opts) => {
      await runExperimentsUse(ctx(opts), name, id, { json: json(opts) });
    }
  );

  for (const [spec, description, retrospective] of [
    ["register <name>", "freeze the draft and make it the run — irreversible", false],
    ["retrospective <name>", "analyse the draft, whose dates are past, as its one run", true]
  ] as const) {
    verb(spec, description)
      .option("--yes", "do it without asking (required when stdin is not a terminal)")
      .action(async (name: string, opts: Opts) => {
        await runExperimentsRegister(ctx(opts), name, {
          retrospective,
          yes: Boolean(opts.yes),
          json: json(opts)
        });
      });
  }

  verb("status <name>", "the run: state, flips, guardrails, compliance, estimate").action(
    async (name: string, opts: Opts) => {
      await runExperimentsStatus(ctx(opts), name, { json: json(opts) });
    }
  );

  verb("abort <name>", "stop a registered or live run")
    .option("--reason <text>", "what the owner is told (required)")
    .option("--levers <off|keep>", "turn treated units' levers off, or keep them", "off")
    .option("--yes", "do it without asking (required when stdin is not a terminal)")
    .action(async (name: string, opts: Opts) => {
      await runExperimentsAbort(ctx(opts), name, {
        reason: opts.reason as string | undefined,
        levers: String(opts.levers),
        yes: Boolean(opts.yes),
        json: json(opts)
      });
    });

  verb("result <name>", "the sealed report — or that it is still sealed").action(
    async (name: string, opts: Opts) => {
      await runExperimentsResult(ctx(opts), name, { json: json(opts) });
    }
  );

  verb("interim <name>", "a live run's reading so far — a peek, never the result")
    .option("--refresh", "ask for a fresh reading (one at a time per run)")
    .option("--wait", "with --refresh, poll until the reading is computed")
    .option("--timeout <seconds>", "how long --wait polls", "600")
    .action(async (name: string, opts: Opts) => {
      await runExperimentsInterim(ctx(opts), name, {
        refresh: Boolean(opts.refresh),
        wait: Boolean(opts.wait),
        timeoutSec: Number(opts.timeout),
        json: json(opts)
      });
    });

  verb(
    "write-back <name>",
    "open a branch writing the measured coefficient into the World Model"
  ).action(async (name: string, opts: Opts) => {
    await runExperimentsWriteBack(ctx(opts), name, { json: json(opts) });
  });
}
