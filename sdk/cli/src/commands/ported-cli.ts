/**
 * The verbs ported from the Rust `oxy` binary (`internal-docs/oxy-cli-to-oxyc.md`),
 * registered in this one place: each port adds its line here, not to `main.ts`,
 * whose size the complexity gate holds still. The order is the order `--help`
 * lists them in.
 */

import type { Command } from "commander";
import { registerAgentic } from "./agentic-cli.js";
import { registerAirway } from "./airway-cli.js";
import { registerEvals } from "./evals-cli.js";
import type { Wiring } from "./flags-cli.js";
import { registerLooker } from "./looker.js";
import { registerRun } from "./run-cli.js";
import { registerSync } from "./sync-cli.js";

export function registerPorted(program: Command, w: Wiring): void {
  registerAirway(program, w);
  registerAgentic(program, w);
  registerEvals(program, w);
  registerRun(program, w);
  registerSync(program, w);
  registerLooker(program, w);
}
