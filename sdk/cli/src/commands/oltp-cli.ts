/**
 * Wires `oxyc oltp audit | rotate | apply | changes resync` onto the `oltp` group, split out of
 * `main.ts` for the reason `airway-cli.ts` is. These are the three verbs that
 * run as the tenant's owner, over the routes that replaced the Rust
 * `oxy oltp audit | rotate | apply` (`internal-docs/oxy-cli-to-oxyc.md`, step 12).
 */

import type { Command } from "commander";
import { createContext } from "../context/resolve.js";
import type { Wiring } from "./flags-cli.js";
import { runOltpApply } from "./oltp-apply.js";
import { runOltpAudit, runOltpRotate } from "./oltp-audit.js";
import { runOltpChangesResync } from "./oltp-changes.js";

type Opts = Record<string, unknown>;

export function registerOltpOwnerVerbs(
  oltp: Command,
  { withGlobals, globals }: Pick<Wiring, "withGlobals" | "globals">
): void {
  withGlobals(
    oltp
      .command("audit")
      .description(
        "every role's real authority in the org's database, and its grants outside its own schema — exit 9 on a violation"
      )
      .option("--json", "emit the server's report (still exits 9 on a violation)")
  ).action(async (opts: Opts) => {
    await runOltpAudit(createContext(globals(opts)), opts.org as string | undefined, {
      json: opts.json as boolean | undefined
    });
  });

  withGlobals(
    oltp
      .command("rotate")
      .description(
        "rotate a writer's password and reseal it (--role <w> --yes) — the old one stops working at once"
      )
      .option("--role <app:slug|pipeline:source>", "the writer to rotate (required)")
      .option("--yes", "required: without it nothing is sent (exit 8)")
      .option("--json", "emit the server's response: {writer, role, schema}")
  ).action(async (opts: Opts) => {
    await runOltpRotate(createContext(globals(opts)), opts.org as string | undefined, {
      role: opts.role as string | undefined,
      yes: opts.yes as boolean | undefined,
      json: opts.json as boolean | undefined
    });
  });

  withGlobals(
    oltp
      .command("apply")
      .description(
        "apply the workspace's compiled schemas/*.sql the org's database has not run, and re-grant every writer (--yes; --workspace <id> when the org has several) — exit 9 when a migration fails"
      )
      // `--workspace <id>` is a global (`withGlobals`): here it names the
      // workspace that supplies the migrations, required when the org has
      // more than one.
      .option("--yes", "required: without it nothing is sent (exit 8)")
      .option("--json", "emit the server's response (still exits 9 when a migration fails)")
  ).action(async (opts: Opts) => {
    await runOltpApply(createContext(globals(opts)), opts.org as string | undefined, {
      workspace: opts.workspace as string | undefined,
      yes: opts.yes as boolean | undefined,
      json: opts.json as boolean | undefined
    });
  });

  const changes = oltp.command("changes").description("an app's OLTP change stream, for staff");
  withGlobals(
    changes
      .command("resync <app>")
      .description(
        "re-baseline an app's captured tables and remove the holds that resolves (--yes) — a held table's unshipped rows are discarded"
      )
      .option("--table <name>", "one table (default: every captured table)")
      .option("--app-env <environment>", "only production has a change stream (default)")
      .option("--yes", "required: without it nothing is sent (exit 8)")
      .option("--json", "emit the server's report")
  ).action(async (app: string, opts: Opts) => {
    await runOltpChangesResync(createContext(globals(opts)), app, {
      table: opts.table as string | undefined,
      env: opts.appEnv as string | undefined,
      yes: opts.yes as boolean | undefined,
      json: opts.json as boolean | undefined
    });
  });
}
