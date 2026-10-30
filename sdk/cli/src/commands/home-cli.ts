/**
 * Wires `oxyc home …` onto the program, beside `experiments-cli.ts` and for the
 * same reason: `main.ts` already holds every other group.
 */

import type { Command } from "commander";
import { createContext } from "../context/resolve.js";
import type { Wiring } from "./experiments-cli.js";
import { runHomeApply, runHomeGet, runHomeReset, runHomeShow } from "./home.js";

type Opts = Record<string, unknown>;

const HELP = `
All verbs take --org <slug|uuid|url> (or an --env that is an org URL).

LAYING OUT THE HOME
  oxyc home show --org acme                    what a person sees, as text
  oxyc home get --org acme > layout.json       { revision, layout }: edit the layout
  oxyc home apply --org acme -f layout.json --dry-run    the result as text, nothing saved
  oxyc home apply --org acme -f layout.json    save it
  oxyc home reset --org acme                   back to the default

  A layout is { version: 1, sections: [ { kind: widgets, items: [ { widget: app/widget,
  size?, hidden? } ], others?: show|hide }, { kind: apps }, { kind: insights } ] }.
  Writing needs an org owner or admin. apply sends the revision in the file get
  printed (or --revision N): a layout someone else saved since is exit 8, and
  --force overwrites it. A refused layout
  (exit 6) lists every problem as "path: message (fix)".`;

export function registerHome(program: Command, w: Wiring): void {
  const home = program
    .command("home")
    .description("the org's home — see it, and lay out its widgets")
    .addHelpText("after", HELP);

  const verb = (spec: string, description: string) =>
    w.withGlobals(home.command(spec).description(description));
  const ctx = (opts: Opts) => createContext(w.globals(opts));
  const org = (opts: Opts) => opts.org as string | undefined;

  verb("show", "the home as text: sections in order, each widget with its size and headline")
    .option("--json", "emit the description and the widgets feed, raw")
    .action(async (opts: Opts) => {
      await runHomeShow(ctx(opts), org(opts), { json: Boolean(opts.json) });
    });

  verb("get", "{ revision, layout }: the effective layout to edit and `apply` back").action(
    async (opts: Opts) => {
      await runHomeGet(ctx(opts), org(opts));
    }
  );

  verb("apply", "save a layout (checked on the server; every problem listed at once)")
    .option("-f, --file <path|->", "the layout: JSON or YAML, `-` for stdin")
    .option("--dry-run", "show the resulting home as text without saving")
    .option("--revision <n>", "the revision your edit started from (default: the file's)")
    .option("--force", "overwrite whatever is saved: send no revision")
    .option("--json", "emit the server's description")
    .action(async (opts: Opts) => {
      await runHomeApply(ctx(opts), org(opts), {
        file: opts.file as string | undefined,
        revision: opts.revision as string | undefined,
        dryRun: Boolean(opts.dryRun),
        force: Boolean(opts.force),
        json: Boolean(opts.json)
      });
    });

  verb("reset", "clear the layout: back to the default").action(async (opts: Opts) => {
    await runHomeReset(ctx(opts), org(opts));
  });
}
