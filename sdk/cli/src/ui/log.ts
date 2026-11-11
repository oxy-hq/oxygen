/**
 * Everything that is *not* the answer goes to stderr.
 *
 * The rule that makes this tool pipeable: stdout carries the response body and
 * nothing else, so `oxyc api user | jq .email` works and `oxyc routes --json`
 * is valid JSON on the first byte. Progress, warnings, staleness notices and
 * the "what to run next" hints all go here.
 */

import { err, stderrIsTty } from "./tty.js";

function write(line: string): void {
  process.stderr.write(`${line}\n`);
}

/** Ordinary progress. Suppressed when `--quiet` set OXYC_QUIET. */
export function info(message: string): void {
  if (process.env.OXYC_QUIET) return;
  write(err.dim(message));
}

/**
 * Something is off but the run continues — a stale cache, a skipped file.
 *
 * Never suppressed by `--quiet`. A warning that a quiet flag can hide is a
 * warning nobody sees on the run where it mattered, and the loudest of these
 * ("serving a STALE customer list") exists precisely to stop a degraded answer
 * from passing as a good one.
 */
export function warn(message: string): void {
  write(`${err.yellow("warning:")} ${message}`);
}

/**
 * Something the person must act on — an approval URL and its code. Never
 * suppressed by `--quiet`: hiding it leaves a command waiting on an approval
 * nobody was told about.
 */
export function say(message: string): void {
  write(message);
}

/** A failure, on its way to a non-zero exit. */
export function error(message: string): void {
  write(`${err.red("error:")} ${message}`);
}

/**
 * A line elaborating the message above it — under a warning, an error or a
 * refusal, never on its own.
 *
 * Originally documented as "what to type next", which its callers outgrew: the
 * highest-volume one lists PATHS, another states a CONSEQUENCE, others do name
 * a command. What they share is that `→` points back at the line above, and
 * that is the accurate reading of the marker — not "listed item", which an
 * earlier version of `remedy`'s doc leaned on and which the path lists happen
 * to satisfy by coincidence.
 */
export function hint(message: string): void {
  write(`  ${err.dim("→")} ${message}`);
}

/**
 * What to do about what was just reported — set apart from it.
 *
 * A SIBLING OF `hint`, not `hint` itself, and the distinction is SEPARATION,
 * not "listed item versus instruction": `→` elaborates the line above, so a
 * remedy printed with it is one more elaboration among ten paths, read in
 * sequence with them. This is the one line that is not about what happened but
 * about what to do, and the blank lines are what say so. Not `info` either —
 * that is suppressed by `--quiet`, and this hangs off a warning that
 * deliberately is not.
 *
 * Takes the whole group, so N remedies are one bracketed block rather than N
 * blocks with doubled blanks between them — and SPLITS EACH ONE, so a remedy
 * that grows a newline keeps the marker on every line instead of printing a
 * bare continuation. One meaning for the parameter: each argument is a remedy.
 * A caller splitting its own string would have made it two.
 */
export function remedy(...messages: string[]): void {
  // Checked on the ARGUMENTS, not on the split: `split("\n")` always yields at
  // least one element, so `lines.length === 0` is the same test one derivation
  // away — and the caller this guard is for spreads a possibly-empty array.
  if (messages.length === 0) return;
  const lines = messages.flatMap((message) => message.split("\n"));
  // BLANK ON BOTH SIDES. With one only above, the block attached DOWNWARD to
  // whatever came next — and the common case is a `log.error` that follows
  // immediately, so `fix:` ended up closer to an unrelated error than to the
  // warning it belongs to, inverting the grouping the separator was added for.
  write("");
  for (const line of lines) write(`  ${err.dim("fix:")} ${line}`);
  write("");
}

/**
 * A step whose duration a human would otherwise wonder about.
 *
 * Deliberately not a spinner library: a spinner writes escape codes on a timer
 * and a non-TTY stderr — CI, an agent capturing output — collects every frame
 * as a separate line. Under a TTY this rewrites one line; otherwise it prints
 * the label once and the outcome once, which is exactly what a log wants.
 */
export function step(label: string): { done: (outcome?: string) => void } {
  if (process.env.OXYC_QUIET) return { done: () => {} };
  if (!stderrIsTty()) {
    write(err.dim(`${label}…`));
    return {
      done: (outcome) => {
        if (outcome) write(err.dim(`  ${outcome}`));
      }
    };
  }
  process.stderr.write(err.dim(`${label}… `));
  return {
    done: (outcome) => {
      process.stderr.write(`\r[2K${err.dim(`${label}… ${outcome ?? "done"}`)}\n`);
    }
  };
}
