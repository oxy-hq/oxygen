/**
 * `--jq`: the `jq` binary when it is on PATH, an embedded jq when it is not.
 *
 * The embedded engine is `jq-wasm` — jq 1.8.2 itself compiled to WebAssembly,
 * not a reimplementation, so a filter means exactly what it means to jq. That
 * is the property that rules out the pure-JS ports: a `--jq` that silently
 * evaluates a filter differently is worse than one that refuses. It is what
 * `gh api -q` does too (gh embeds gojq), and why `oxyc api … -q length` now
 * works in a bare CI runner or a cloud VM where nobody installed jq.
 *
 * Why the binary still goes FIRST: the WASM heap aborts on a body somewhere
 * past ~50 MB, which the native jq handles fine. Preferring the binary keeps
 * every machine that had jq on exactly the behaviour it had before, and the
 * embedded engine only ever serves a machine on which `--jq` used to be a hard
 * error.
 *
 * The WASM module is imported lazily, on the first `--jq` that needs it, so a
 * command without `--jq` pays nothing for it at start-up. The `/inline` build
 * carries the wasm bytes inside the JavaScript: there is no `.wasm` file to
 * locate on disk, which is what makes it work unchanged in the Bun-compiled
 * single-file binaries.
 */

import { spawnSync } from "node:child_process";
import type { Jq } from "jq-wasm/inline";
import { CliError, ExitCode } from "../util/errors.js";

export interface JqOptions {
  /** The jq executable to try first. Tests point it at a name that does not exist. */
  binary?: string;
}

export async function runJq(input: string, program: string, opts: JqOptions = {}): Promise<string> {
  const viaBinary = runBinaryJq(input, program, opts.binary ?? "jq");
  if (viaBinary !== undefined) return viaBinary;
  return runEmbeddedJq(input, program);
}

/**
 * Spawn errors that mean "this binary cannot run here", as opposed to "your
 * filter is wrong".
 *
 * `ENOENT` is jq not installed, and falling back to the embedded engine for it
 * was always the behaviour. The rest are the machine being out of processes,
 * file descriptors or memory, which is not the caller's fault either and which
 * the embedded engine is unaffected by — so answering the query beats failing
 * it. Before this, a `-q` on a loaded machine died with `jq failed: spawn
 * EAGAIN` and no answer, while a perfectly good engine sat in the same
 * process; CI hit it intermittently with 89 concurrent test workers forking.
 *
 * Anything outside this set still throws: a binary that spawns and refuses is
 * a fact worth surfacing, not something to paper over.
 */
const UNUSABLE_BINARY = new Set([
  "ENOENT",
  "EACCES",
  "EPERM",
  "EAGAIN",
  "EMFILE",
  "ENFILE",
  "ENOMEM"
]);

/** `undefined` when the binary cannot be run at all; throws on a failing filter. */
function runBinaryJq(input: string, program: string, binary: string): string | undefined {
  const result = spawnSync(binary, ["-r", program], {
    input,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024
  });

  if (result.error) {
    const code = (result.error as NodeJS.ErrnoException).code;
    if (code !== undefined && UNUSABLE_BINARY.has(code)) return undefined;
    throw new CliError(`jq failed: ${result.error.message}`, { code: ExitCode.FAILURE });
  }
  if (result.status !== 0) throw badFilter(result.status, result.stderr);
  return result.stdout;
}

/**
 * Brackets every run's output, because `jq-wasm` `.trim()`s stdout and `-r`
 * output is not safe to trim: `-q '.[] | "  - \(.name)"'` would lose the first
 * row's indent. RS (U+001E) is not whitespace, so trim stops at it, and jq's
 * `-r` prints each one followed by a newline we then remove with it.
 */
const SENTINEL = "\u001e";

let loaded: Promise<Jq> | undefined;

export async function runEmbeddedJq(input: string, program: string): Promise<string> {
  if (!loaded) loaded = import("jq-wasm/inline").then((m) => m.loadJq());
  const jq = await loaded;

  // The program goes on its own lines so a trailing `# comment` cannot swallow
  // the closing paren, and in parens so `a, b` and `def f: …; f` stay one term.
  const bracketed = `"${SENTINEL}", (\n${program}\n), "${SENTINEL}"`;
  let result: ReturnType<Jq["raw"]>;
  try {
    result = jq.raw(input, bracketed, ["-r"]);
  } catch (cause) {
    // An Emscripten abort — in practice the heap running out on a very large
    // body — and it leaves the instance unusable, so the next call reloads.
    loaded = undefined;
    throw new CliError(`the embedded jq could not process this response (${megabytes(input)} MB)`, {
      code: ExitCode.FAILURE,
      detail: cause instanceof Error ? cause.message : String(cause),
      hint: "install jq (brew install jq, or apt-get install jq) — oxyc uses it when it is on PATH"
    });
  }

  const { stdout, stderr, exitCode } = result;
  if (exitCode !== 0) {
    // No leading sentinel means the program never ran: a compile error, whose
    // message quotes the program with line and column numbers. Those are the
    // BRACKETED program's, so ask again unbracketed for the message the
    // person can actually match against what they typed.
    const message = stdout.startsWith(SENTINEL) ? stderr : jq.raw(input, program, ["-r"]).stderr;
    throw badFilter(exitCode, message.replaceAll("/dev/stdin", "<stdin>"));
  }

  return unbracket(stdout);
}

/**
 * `RS\n<jq's own -r output>RS` — the trim took the final newline — back to
 * jq's own output. A body of several JSON texts runs the program once per
 * text, so each boundary between two runs is `RS\nRS\n` and goes too.
 */
function unbracket(stdout: string): string {
  const opened = `${SENTINEL}\n`;
  let out = stdout.startsWith(opened) ? stdout.slice(opened.length) : stdout;
  out = out.replaceAll(`${SENTINEL}\n${opened}`, "");
  if (out.endsWith(SENTINEL)) return out.slice(0, -SENTINEL.length);
  // `halt` stops before the closing sentinel, and the trim took jq's newline.
  return out === "" ? "" : `${out}\n`;
}

function badFilter(status: number | null, stderr: string | undefined): CliError {
  return new CliError(`jq exited ${status}`, {
    code: ExitCode.USAGE,
    detail: stderr?.trim() || undefined,
    hint: "check the jq program — `oxyc api <path>` without --jq shows the shape"
  });
}

function megabytes(text: string): string {
  return (Buffer.byteLength(text) / 1024 / 1024).toFixed(1);
}
