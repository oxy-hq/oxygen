/**
 * Wires `oxyc airway …` onto the program, split out of `main.ts` for the same
 * reason `flags-cli.ts` is. The Rust `oxy airway` was deleted when this landed
 * (`internal-docs/oxy-cli-to-oxyc.md`, step 6).
 */

import type { Command } from "commander";
import { createContext } from "../context/resolve.js";
import { runAirwayBackfill, runAirwayCoverage } from "./airway-backfill.js";
import { runAirwayLeases, runAirwayReleaseLease } from "./airway-leases.js";
import { runAirwayRun } from "./airway-run.js";
import type { Wiring } from "./flags-cli.js";

type Opts = Record<string, unknown>;

const HELP = `
Every verb needs --workspace <id>: pipelines, runs, ranges and leases belong to a workspace.

  oxyc airway run pipelines/toast.airway.yml --workspace <id>     start it and stream to the end
  oxyc airway run <ref> --resources orders,checks --detach        start two resources, print the run id
  oxyc airway backfill <ref> --from 2026-01-01T00:00:00Z --to 2026-04-01T00:00:00Z --wait 3600
  oxyc airway backfill --resume <range-id>                        re-drive a range's not-done chunks
  oxyc airway coverage <ref>                                      the pipeline's backfill ranges
  oxyc airway coverage --range <range-id>                         one range's missing periods
  oxyc airway leases                                              what holds each pipeline (admin)
  oxyc airway release-lease toast_orders                          free a dead holder, after a prompt

The server runs the pipeline it has COMPILED for the workspace's served revision, on the
worker fleet — not a .airway.yml on this machine. Nothing from your environment is sent:
{{ VAR }} templates render from --variables only (backfills take none). To try an unpushed edit, push a branch and run:
  oxyc preview run <branch> airway_sample <ref> --workspace <id>

Exit codes: 1 the run failed, was cancelled or timed out (read from the run row), or a
waited backfill left chunks not done; 2 a bad flag; 4 not signed in, or not allowed (the
lease verbs need a workspace admin); 5 no such workspace, run or range; 6 the server refused
the request — 409 when another run holds the pipeline, or when release-lease finds a
different (or re-acquired) holder; 7 retryable (the deployment cannot serve the pipeline
yet, the stream kept closing, or --wait ran out); 8 release-lease needs --yes or --force
with no terminal to ask.`;

export function registerAirway(program: Command, w: Wiring): void {
  const airway = program
    .command("airway")
    .description("airway ELT pipelines on a deployment: run, backfill, coverage and leases")
    .addHelpText("after", HELP);

  w.withGlobals(
    airway
      .command("run <pipeline_ref>")
      .description("start a pipeline (its compiled definition) and stream its events to the end")
      .option(
        "--resources <names>",
        "only these resources (comma-separated, repeatable)",
        commaList,
        []
      )
      .option(
        "--variables <json>",
        "values for the pipeline's {{ VAR }} templates: JSON, @file or - (stored with the run, so no secrets)"
      )
      .option("--detach", "print the run id and return without streaming")
      .option("--json", "one JSON object per event: {seq, event_type, payload}")
  ).action(async (ref: string, opts: Opts) => {
    await runAirwayRun(createContext(w.globals(opts)), ref, {
      json: Boolean(opts.json),
      resources: opts.resources as string[],
      variables: opts.variables as string | undefined,
      detach: Boolean(opts.detach)
    });
  });

  w.withGlobals(
    airway
      .command("backfill [pipeline_ref]")
      .description(
        "chunked, resumable backfill over [--from, --to), queued for the worker fleet; --resume re-drives a range"
      )
      .option("--from <rfc3339>", "inclusive window start, e.g. 2026-01-01T00:00:00Z")
      .option("--to <rfc3339>", "exclusive window end")
      .option("--granularity <grain>", "chunk size: month (default), week or day")
      .option(
        "--resources <names>",
        "only these resources (comma-separated, repeatable); stored on the range, so a resume keeps it",
        commaList,
        []
      )
      .option("--resume <range-id>", "re-drive the not-done chunks of an existing range")
      .option("--wait <seconds>", "wait for the range to settle, then print its coverage")
      .option("--json", "emit the server's response (or, with --wait, the coverage report)")
  ).action(async (ref: string | undefined, opts: Opts) => {
    await runAirwayBackfill(createContext(w.globals(opts)), ref, {
      from: opts.from as string | undefined,
      to: opts.to as string | undefined,
      granularity: opts.granularity as string | undefined,
      resources: opts.resources as string[],
      resume: opts.resume as string | undefined,
      waitSeconds: opts.wait === undefined ? undefined : Number(opts.wait),
      json: Boolean(opts.json)
    });
  });

  w.withGlobals(
    airway
      .command("coverage [pipeline_ref]")
      .description("a pipeline's backfill ranges, or with --range one range's missing periods")
      .option("--range <range-id>", "one range's chunks")
      .option("--json", "emit the server's response")
  ).action(async (ref: string | undefined, opts: Opts) => {
    await runAirwayCoverage(createContext(w.globals(opts)), ref, {
      range: opts.range as string | undefined,
      json: Boolean(opts.json)
    });
  });

  w.withGlobals(
    airway
      .command("leases")
      .description("the single-flight leases held in the workspace — why won't a pipeline start?")
      .option("--json", "emit the server's response")
  ).action(async (opts: Opts) => {
    await runAirwayLeases(createContext(w.globals(opts)), Boolean(opts.json));
  });

  w.withGlobals(
    airway
      .command("release-lease <pipeline_name>")
      .description(
        "force-release a pipeline's lease (its NAME, as `leases` prints it): shows the holder and asks first"
      )
      .option("--yes", "release the holder just listed without asking")
      .option(
        "--force",
        "release whatever holds the lease, without asking — only for a holder you know is dead"
      )
      .option("--json", "emit the server's response")
  ).action(async (name: string, opts: Opts) => {
    await runAirwayReleaseLease(createContext(w.globals(opts)), name, {
      yes: Boolean(opts.yes),
      force: Boolean(opts.force),
      json: Boolean(opts.json)
    });
  });
}

/** `--resources a,b --resources c` → `[a, b, c]`. */
function commaList(value: string, previous: string[]): string[] {
  return [
    ...previous,
    ...value
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean)
  ];
}
