/**
 * Wires `oxyc workspace …` onto the program, split out of `main.ts` for the
 * same reason `flags-cli.ts` is.
 */

import type { Command } from "commander";
import { createContext } from "../context/resolve.js";
import type { Wiring } from "./flags-cli.js";
import { runWorkspaceClone } from "./workspace-clone.js";

const HELP = `
Examples:
  oxyc workspace clone 3f2c9a1e-…            clone it next to where you are, open it in VS Code
  oxyc workspace clone Analytics --org acme  a name works inside --org
  oxyc workspace clone <id> --dir ~/code/analytics --open cursor
  oxyc workspace clone <id> --branch my-change --open none

Authoring happens in the repository: edit with your own tools or an agent and push.
A push to the default branch compiles and deploys.

The clone is plain \`git clone --branch <default branch> <url> <dir>\` with YOUR git and
credentials — no token is injected and the URL is not rewritten. A workspace that lives in a
subdirectory of its repository opens at that subdirectory. An editor is launched only from
an interactive terminal; anywhere else the command is printed instead.

Exit codes: 5 the workspace has no repository (connect one in Settings) or is not found,
6 git clone failed, 8 the target directory is not empty, 2 a bad flag.`;

export function registerWorkspace(program: Command, w: Wiring): void {
  const workspace = program
    .command("workspace")
    .description("a workspace's repository on your machine — clone it and edit with your own tools")
    .addHelpText("after", HELP);

  w.withGlobals(
    workspace
      .command("clone <workspace>")
      .description("clone the workspace's repository and open it in your editor")
      .option(
        "--dir <path>",
        "where to clone (default: the repository's name, in the current directory)"
      )
      .option(
        "--open <editor>",
        "code | cursor | none (default: code, when it is installed and you are at a terminal)"
      )
      .option("--branch <name>", "check out this branch instead of the default branch")
  ).action(async (id: string, opts: Record<string, unknown>) => {
    await runWorkspaceClone(createContext(w.globals(opts)), id, {
      dir: opts.dir as string | undefined,
      open: opts.open as string | undefined,
      branch: opts.branch as string | undefined
    });
  });
}
