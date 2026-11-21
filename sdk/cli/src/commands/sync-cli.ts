/**
 * Wires `oxyc sync` onto the program, split out of `main.ts` for the same
 * reason `airway-cli.ts` is. The Rust `oxy sync` is a one-release stub that
 * prints this command (`internal-docs/oxy-cli-to-oxyc.md`, step 12).
 */

import type { Command } from "commander";
import { createContext } from "../context/resolve.js";
import { usageError } from "../util/errors.js";
import type { Wiring } from "./flags-cli.js";
import { runSync } from "./sync.js";

type Opts = Record<string, unknown>;

const HELP = `
Run it from the root of your checkout. The SERVER introspects the warehouses the DEPLOYED
config.yml names (with the workspace's secrets) and renders the files; oxyc writes them under
.databases/ here. Push a config change before syncing it. No warehouse credentials are needed
on this machine, only an oxyc login.

  oxyc sync --workspace <id>                            every database, one request each
  oxyc sync warehouse --workspace <id>                  one database
  oxyc sync warehouse -d analytics -d marts             only those datasets (schemas)
  oxyc sync warehouse --tables analytics.orders         only those tables
  oxyc sync --overwrite                                 replace existing files, prune stale models/
  oxyc sync --dry-run                                   print what would change, write nothing

Existing files are skipped unless --overwrite. With --tables, --overwrite replaces only those
tables' models: nothing is pruned and an existing dataset .sql is kept. A database whose type sync cannot introspect
(airhouse_managed, airhouse, mysql) is skipped with a warning.

Exit codes: 1 a database failed for another reason; 2 a bad flag; 4 not signed in, or not a
workspace admin; 5 no such database or workspace; 6 the database cannot connect as configured,
or (named) cannot be synced; 7 retryable (the warehouse failed, or the introspection outlasted
the server's limit: narrow it with -d); 8 the server returned a path outside .databases/
(nothing is written). With no database named, the remaining databases still sync and the exit
code is the first failure's.`;

export function registerSync(program: Command, w: Wiring): void {
  w.withGlobals(
    program
      .command("sync [database]")
      .description(
        "write the workspace's .databases/ schema files into this checkout, introspected by the server (--workspace <id>)"
      )
      .option("--database <name>", "the database to sync (same as the positional)")
      .option("-d, --datasets <name>", "only this dataset/schema (repeatable)", w.collect, [])
      .option(
        "--tables <schema.table>",
        "only this table (repeatable; wins over -d)",
        w.collect,
        []
      )
      .option("-o, --overwrite", "replace existing files and prune stale models/ files")
      .option("--dry-run", "print what would be written, write nothing")
      .option("--dir <path>", "the checkout root to write under (default: the working directory)")
      .option("--json", "emit {dry_run, databases, files} instead of the human summary")
      .addHelpText("after", HELP)
  ).action(async (positional: string | undefined, opts: Opts) => {
    const flag = opts.database as string | undefined;
    if (positional !== undefined && flag !== undefined && positional !== flag) {
      throw usageError(`two databases named: ${positional} and --database ${flag}`);
    }
    await runSync(createContext(w.globals(opts)), {
      database: positional ?? flag,
      datasets: opts.datasets as string[],
      tables: opts.tables as string[],
      overwrite: Boolean(opts.overwrite),
      dryRun: Boolean(opts.dryRun),
      dir: opts.dir as string | undefined,
      json: Boolean(opts.json)
    });
  });
}
