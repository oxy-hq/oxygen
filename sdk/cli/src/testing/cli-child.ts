/**
 * What a test that runs the built CLI needs so that a slow machine fails no case and a hung
 * run fails its own case by name.
 *
 * The suites that drive `dist/main.mjs` pay for one cold `node` start per run: ~0.2s on an
 * idle machine and ~1s at a load of 40. The cost is the CPU, not the CLI, so those files
 * set a file-wide test budget (`CLI_TEST_TIMEOUT_MS`) in place of vitest's 5s wall-clock
 * default. The budget is not what catches a hang, because a blocking `spawnSync` cannot be
 * interrupted and an async run outlives a timed-out case. The run's own kill timeout
 * (`CLI_KILL_TIMEOUT_MS`) catches it, and `cliRunError` names the command that hung.
 *
 * Test-only: nothing in the shipped bundle imports this.
 */

import type { ChildProcess } from "node:child_process";
import { afterEach } from "vitest";

/** A file-wide test budget for cases that run the CLI, possibly many times in a row. */
export const CLI_TEST_TIMEOUT_MS = 120_000;

/** How long one run of the CLI may take before it is killed and its case fails. */
export const CLI_KILL_TIMEOUT_MS = 20_000;

/** The failure for a run that the kill timeout ended or that never started. */
export function cliRunError(args: readonly string[], why: string): Error {
  return new Error(
    `oxyc ${args.join(" ")} did not exit on its own within ${CLI_KILL_TIMEOUT_MS}ms (${why})`
  );
}

/**
 * Kills every child a case started when the case ends, and waits until each one has exited.
 *
 * A case that times out does not stop the CLI runs it started. Vitest goes on to the next
 * case, and the abandoned run keeps talking to the file's fake server, which by then holds
 * the NEXT case's state: `flags.test.ts`'s "--clear sends targeting: null" failed on a PATCH
 * that the timed-out case before it sent. A case can now see only the requests its own runs
 * sent.
 *
 * Call once at the top level of a file. Then pass each child to the function this returns.
 */
export function reapChildrenAfterEach(): <C extends ChildProcess>(child: C) => C {
  const live = new Set<ChildProcess>();
  afterEach(async () => {
    const left = [...live];
    live.clear();
    await Promise.all(left.map(stop));
  });
  return <C extends ChildProcess>(child: C): C => {
    live.add(child);
    child.once("close", () => live.delete(child));
    return child;
  };
}

function stop(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve();
  return new Promise((done) => {
    child.once("close", () => done());
    child.kill("SIGKILL");
  });
}
