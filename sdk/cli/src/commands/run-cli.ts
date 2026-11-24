/**
 * Wires `oxyc run` onto the program, split out of `main.ts` for the same
 * reason `airway-cli.ts` is. The Rust `oxy run` is a one-release stub that
 * prints this command (`internal-docs/oxy-cli-to-oxyc.md`, step 10).
 */

import type { Command } from "commander";
import { createContext } from "../context/resolve.js";
import { usageError } from "../util/errors.js";
import type { Wiring } from "./flags-cli.js";
import { runRun } from "./run.js";

type Opts = Record<string, unknown>;

const HELP = `
Runs on the deployment --env names, in the workspace --workspace names; nothing runs here.

  oxyc run queries/weekly.sql --database warehouse --workspace <id>
  oxyc run queries/weekly.sql -v start=2024-01-01 status=active      render {{ start }}, {{ status }}
  oxyc run queries/weekly.sql --json                                  rows as JSON, header row first
  oxyc run procedures/report.automation.yml --workspace <id>          run it, wait, print each task
  oxyc run procedures/report.automation.yml -v region=EU --detach     start it, print the run id

An automation runs the definition the deployment has COMPILED for its served revision, not
the file on this machine: push it first. To try a branch (writes held), run:
  oxyc preview run <branch> procedure procedures/report.automation.yml --workspace <id>
A SQL file's text is read here and sent; it runs against the DEPLOYED database of that name,
with the workspace's secrets and your identity. --database is required when the workspace
serves more than one database. -v renders only {{ name }} placeholders, and only when given;
for an automation, -v sets the run's variables. A result stops at 10,000 rows.

Exit codes: 1 the automation run failed, was cancelled or timed out, or --timeout ran out
(the run carries on: re-running starts another); 2 a bad flag, file kind,
template or -v pair, or no --database where one is needed; 4 not signed in, or not a member of
the workspace; 5 no such local SQL file, database, workspace or deployed automation; 6 the
server refused the request (the query failed: its message is printed); 7 retryable (the
warehouse or the deployment is unavailable before anything started).`;

export function registerRun(program: Command, w: Wiring): void {
  w.withGlobals(
    program
      .command("run <file>")
      .description(
        "run a .sql file or an automation (.automation.yml) on the deployment (--workspace <id>)"
      )
      .option("--database <name>", "the database a .sql file runs against")
      .option(
        "-v, --variables <KEY=VALUE...>",
        "values for a .sql file's {{ KEY }} placeholders, or an automation's variables"
      )
      .option("--json", "SQL: the route's rows as JSON; automation: the run's final snapshot")
      .option("--detach", "automation: print the run id and return without waiting")
      .option("--timeout <seconds>", "automation: how long to wait for the run to end", "3600")
      .addHelpText("after", HELP)
  ).action(async (file: string, opts: Opts) => {
    const timeout = Number(opts.timeout);
    if (!Number.isFinite(timeout) || timeout <= 0) {
      throw usageError("--timeout must be a positive number of seconds");
    }
    await runRun(createContext(w.globals(opts)), file, {
      database: opts.database as string | undefined,
      variables: (opts.variables as string[] | undefined) ?? [],
      json: Boolean(opts.json),
      detach: Boolean(opts.detach),
      timeoutSeconds: timeout
    });
  });
}
