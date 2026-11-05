/**
 * Wires `oxyc flags …` onto the program, split out of `main.ts` for the same
 * reason `experiments-cli.ts` is.
 */

import type { Command } from "commander";
import { createContext, type GlobalFlags } from "../context/resolve.js";
import {
  runFlagsEval,
  runFlagsList,
  runFlagsOrg,
  runFlagsRules,
  runFlagsSet,
  runFlagsShow
} from "./flags.js";

type Opts = Record<string, unknown>;

export interface Wiring {
  withGlobals: (command: Command) => Command;
  globals: (opts: Opts) => GlobalFlags;
  collect: (value: string, previous: string[]) => string[];
}

const HELP = `
Staff only (platform operate). Every write hits the deployment --env names — production by
default — and is deployment-wide. Writes ask first; pass --yes when stdin is not a terminal.

  oxyc flags list                                       every flag: kill switch, rules, unit
  oxyc flags show apps_first_experience                 full detail, rules and any rules error
  oxyc flags set my_flag --off --yes                    the kill switch: off serves every org the off variant
  oxyc flags rules my_flag --set-org acme=on            one org override, the other rules untouched
  oxyc flags rules my_flag --set-org acme=             remove that override
  oxyc flags rules my_flag --split control=50,treatment=50    A/B everyone else (percent; must total 100)
  oxyc flags rules my_flag --everyone off               everyone not overridden gets one variant
  oxyc flags rules my_flag --file rules.json | --clear  replace wholesale | back to the plain switch
  oxyc flags eval --org acme [--user <uuid>]            what each flag serves them, with reason and bucket
  oxyc flags org acme                                   the web-visible values a member of acme gets

Orgs are a slug or a UUID. Users are UUIDs. \`rules\` always reads the flag first and edits
the rules it has, so adding one override never retypes the rest; --dry-run shows the result
without sending. The override flags are --set-org / --set-user because --org is the global
"which org" flag.`;

export function registerFlags(program: Command, w: Wiring): void {
  const flags = program
    .command("flags")
    .alias("flag")
    .description("feature flags and A/B splits — kill switch, per-org overrides, weighted rollouts")
    .addHelpText("after", HELP);

  const verb = (spec: string, description: string) =>
    w
      .withGlobals(flags.command(spec).description(description))
      .option("--json", "emit the server's response");
  const ctx = (opts: Opts) => createContext(w.globals(opts));
  const json = (opts: Opts) => Boolean(opts.json);
  const yes = (command: Command) =>
    command.option("--yes", "do it without asking (required when stdin is not a terminal)");

  verb("list", "every flag with its kill switch and a summary of its rules").action(
    async (opts: Opts) => {
      await runFlagsList(ctx(opts), { json: json(opts) });
    }
  );

  verb("show <key>", "one flag in full: rules, unit, and any unreadable-rules error").action(
    async (key: string, opts: Opts) => {
      await runFlagsShow(ctx(opts), key, { json: json(opts) });
    }
  );

  yes(
    verb("set <key>", "the kill switch only: --off serves every org the off variant")
      .option("--on", "turn the switch on (the rules apply)")
      .option("--off", "turn the switch off (every org gets the first variant)")
  ).action(async (key: string, opts: Opts) => {
    await runFlagsSet(ctx(opts), key, {
      on: Boolean(opts.on),
      off: Boolean(opts.off),
      yes: Boolean(opts.yes),
      json: json(opts)
    });
  });

  yes(
    verb("rules <key>", "change a flag's targeting rules, keeping the ones you do not mention")
      .option(
        "--set-org <org=variant>",
        "serve an org a variant (slug or UUID); `org=` removes it. Repeatable",
        w.collect,
        []
      )
      .option(
        "--set-user <uuid=variant>",
        "serve a user a variant; `uuid=` removes it. Repeatable",
        w.collect,
        []
      )
      .option("--remove-org <org>", "drop an org's override. Repeatable", w.collect, [])
      .option("--remove-user <uuid>", "drop a user's override. Repeatable", w.collect, [])
      .option("--everyone <variant>", "everyone not overridden gets this variant")
      .option(
        "--split <variant=percent,…>",
        "everyone not overridden is split, e.g. control=50,treatment=50 (must total 100)"
      )
      .option("--file <path|->", "replace the rules wholesale from JSON/YAML (see `show --json`)")
      .option("--clear", "remove every rule")
      .option("--dry-run", "print the rules that would be sent; send nothing")
  ).action(async (key: string, opts: Opts) => {
    await runFlagsRules(ctx(opts), key, {
      setOrg: opts.setOrg as string[],
      setUser: opts.setUser as string[],
      removeOrg: opts.removeOrg as string[],
      removeUser: opts.removeUser as string[],
      everyone: opts.everyone as string | undefined,
      split: opts.split as string | undefined,
      file: opts.file as string | undefined,
      clear: Boolean(opts.clear),
      dryRun: Boolean(opts.dryRun),
      yes: Boolean(opts.yes),
      json: json(opts)
    });
  });

  verb("eval", "what every flag serves an org / a person, with the reason and bucket (a preview)")
    .option("--user <uuid>", "evaluate for this user too")
    .action(async (opts: Opts) => {
      await runFlagsEval(ctx(opts), {
        org: opts.org as string | undefined,
        user: opts.user as string | undefined,
        json: json(opts)
      });
    });

  verb(
    "org [org]",
    "the flags a member of this org sees (client flags only; any member can read)"
  ).action(async (org: string | undefined, opts: Opts) => {
    await runFlagsOrg(ctx(opts), org ?? (opts.org as string | undefined), { json: json(opts) });
  });
}
