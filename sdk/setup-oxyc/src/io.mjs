/**
 * How the action talks to the runner — and nothing about Oxygen.
 *
 * GitHub gives an action two channels: WORKFLOW COMMANDS, lines on stdout
 * (`::add-mask::`, `::error::`), and FILE COMMANDS, appends to the files named
 * by `GITHUB_ENV`, `GITHUB_PATH`, `GITHUB_OUTPUT` and `GITHUB_STATE`. This is
 * the whole of `@actions/core` this action needs, written out so the action
 * ships as the source in this directory: no dependency, no bundle, no `dist/`
 * to drift from what was reviewed.
 *
 * Every effect goes through an `Io`, so the tests run the real logic against a
 * fake runner instead of a real one.
 */

import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { appendFileSync } from "node:fs";

/**
 * @typedef {object} ExecResult
 * @property {number | null} status
 * @property {string} stdout
 * @property {string} stderr
 */

/**
 * @typedef {object} Io
 * @property {NodeJS.ProcessEnv} env
 * @property {typeof globalThis.fetch} fetch
 * @property {(line: string) => void} write One line to stdout.
 * @property {(path: string, text: string) => void} append Append to a file.
 * @property {(command: string, args: string[]) => ExecResult} exec
 * @property {(ms: number) => Promise<void>} sleep
 */

/** The runner, for real. @returns {Io} */
export function realIo() {
  return {
    env: process.env,
    fetch: globalThis.fetch,
    write: (line) => {
      process.stdout.write(`${line}\n`);
    },
    append: (path, text) => {
      appendFileSync(path, text, "utf8");
    },
    exec: (command, args) => {
      // `npm` is `npm.cmd` on a Windows runner, which node will only spawn
      // through a shell. Every argument is validated against an allowlist
      // before it gets here, so the shell has nothing to interpret.
      const result = spawnSync(command, args, {
        encoding: "utf8",
        shell: process.platform === "win32"
      });
      return {
        status: result.error ? null : result.status,
        stdout: result.stdout ?? "",
        stderr: result.error ? String(result.error.message) : (result.stderr ?? "")
      };
    },
    sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms))
  };
}

/** An input, as GitHub passes it: `INPUT_<NAME>`, upper-cased, hyphens kept. */
export function getInput(/** @type {Io} */ io, /** @type {string} */ name) {
  return (io.env[`INPUT_${name.replace(/ /g, "_").toUpperCase()}`] ?? "").trim();
}

/** A workflow command's message: `%`, CR and LF would end or forge a command. */
function escapeData(/** @type {string} */ text) {
  return text.replace(/%/g, "%25").replace(/\r/g, "%0D").replace(/\n/g, "%0A");
}

/**
 * Tell the runner to redact `secret` from every log line from here on.
 *
 * Call it BEFORE the value goes anywhere else. A mask registered after the
 * value was printed redacts nothing that already scrolled past.
 */
export function mask(/** @type {Io} */ io, /** @type {string} */ secret) {
  if (secret) io.write(`::add-mask::${escapeData(secret)}`);
}

export function info(/** @type {Io} */ io, /** @type {string} */ message) {
  io.write(message);
}

export function warning(/** @type {Io} */ io, /** @type {string} */ message) {
  io.write(`::warning::${escapeData(message)}`);
}

export function error(/** @type {Io} */ io, /** @type {string} */ message) {
  io.write(`::error::${escapeData(message)}`);
}

/**
 * `name<<delimiter / value / delimiter`, GitHub's multiline-safe file format.
 *
 * The delimiter is random so a value cannot contain it by accident or design:
 * one that did would end the block early and let the rest of the value be read
 * as further assignments.
 */
function fileCommand(/** @type {string} */ name, /** @type {string} */ value) {
  const delimiter = `ghadelimiter_${randomUUID()}`;
  if (name.includes(delimiter) || value.includes(delimiter)) {
    throw new Error("a value contained the file-command delimiter");
  }
  return `${name}<<${delimiter}\n${value}\n${delimiter}\n`;
}

/** Append to the file a `GITHUB_*` variable names, or fail saying which. */
function appendTo(
  /** @type {Io} */ io,
  /** @type {string} */ variable,
  /** @type {string} */ text
) {
  const path = io.env[variable];
  if (!path) throw new Error(`${variable} is not set — is this running inside GitHub Actions?`);
  io.append(path, text);
}

/** Set an environment variable for every LATER step in the job. */
export function exportVariable(
  /** @type {Io} */ io,
  /** @type {string} */ name,
  /** @type {string} */ value
) {
  appendTo(io, "GITHUB_ENV", fileCommand(name, value));
}

/** Prepend a directory to `PATH` for every later step. */
export function addPath(/** @type {Io} */ io, /** @type {string} */ directory) {
  appendTo(io, "GITHUB_PATH", `${directory}\n`);
}

export function setOutput(
  /** @type {Io} */ io,
  /** @type {string} */ name,
  /** @type {string} */ value
) {
  appendTo(io, "GITHUB_OUTPUT", fileCommand(name, value));
}

/** Hand a value from `main` to `post`, where it arrives as `STATE_<name>`. */
export function saveState(
  /** @type {Io} */ io,
  /** @type {string} */ name,
  /** @type {string} */ value
) {
  appendTo(io, "GITHUB_STATE", fileCommand(name, value));
}

export function getState(/** @type {Io} */ io, /** @type {string} */ name) {
  return io.env[`STATE_${name}`] ?? "";
}
