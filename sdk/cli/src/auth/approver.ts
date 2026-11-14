/**
 * `--approver <email>` (env `OXY_APPROVER`) on `oxyc login` and
 * `oxyc tokens create --agent|--sandbox-agent`: who must approve a
 * `--no-browser` request, so the platform can tell them.
 *
 * The platform sends the notice (push, email, Slack DM, the inbox); the person
 * still types the code this command prints, on the approval page. The flag
 * names a person to ask, never a way around the code, so it is valid only
 * where there is a code to type: with `--no-browser`.
 */

import { Option } from "commander";
import { usageError } from "../util/errors.js";

/** The longest address an email can be; the deployment refuses more. */
const EMAIL_MAX = 254;

export const APPROVER_ENV = "OXY_APPROVER";

/** The flag, with `scope` saying where it applies in the help. */
export function approverOption(scope: string): Option {
  return new Option(
    "--approver <email>",
    `${scope}: the person who must approve. They are told by push, email and Slack; the notice has no code, so send them the link this prints`
  ).env(APPROVER_ENV);
}

/** Plausibly an email: one `@` with something either side, no whitespace or control character. */
export function plausibleEmail(text: string): boolean {
  const at = text.indexOf("@");
  return (
    at > 0 &&
    at === text.lastIndexOf("@") &&
    at < text.length - 1 &&
    text.length <= EMAIL_MAX &&
    // biome-ignore lint/suspicious/noControlCharactersInRegex: refusing them is the point
    !/[\s\u0000-\u001f\u007f-\u009f]/.test(text)
  );
}

/**
 * Whether the `--approver` FLAG names anyone — before it is checked. On a CI
 * runner a named approver is what makes a device-code request worth starting:
 * someone is told about it.
 *
 * The flag only, never `OXY_APPROVER`: an exported approver is honoured only
 * where the device code was chosen explicitly (`approverFrom`'s `envHonored`),
 * so counting it here would start a request on a runner and then drop the one
 * person who would have been told.
 */
export function approverNamed(opts: Record<string, unknown>, source: string | undefined): boolean {
  return source === "cli" && typeof opts.approver === "string" && opts.approver.trim() !== "";
}

/**
 * The approver the flags (or `OXY_APPROVER`) name, or `undefined`. A usage
 * error — before anything is sent — when the FLAG is given without
 * `--no-browser`, or when the value cannot be an email. An empty value is no
 * value, and a value that came from the environment (`source === "env"`) is
 * simply not used by a browser round: an engineer who exports `OXY_APPROVER`
 * for their agents must still be able to run `oxyc login` themselves.
 *
 * `envHonored` is whether the device code was CHOSEN (`--no-browser`,
 * `--no-wait`, a Claude Code cloud session: `exportedApproverApplies`), not
 * merely detected. Where oxyc switched by itself — an engineer's own login
 * over SSH, in a Codespace — an exported approver is that engineer's setting
 * for their agents, and using it would hand their login to someone else to
 * approve. So it is dropped there, as in a browser round.
 */
export function approverFrom(
  opts: Record<string, unknown>,
  noBrowser: boolean,
  source: string | undefined = "cli",
  envHonored: boolean = noBrowser
): string | undefined {
  const email = typeof opts.approver === "string" ? opts.approver.trim() : "";
  if (email === "") return undefined;
  if (source === "env" && (!noBrowser || !envHonored)) return undefined;
  if (!noBrowser) {
    throw usageError(
      "--approver is only valid with --no-browser",
      "it names who approves a device-code request; a browser approval is the person at the browser"
    );
  }
  if (!plausibleEmail(email)) {
    throw usageError(
      "--approver must be an email address",
      "the person who must approve, such as `--approver ada@example.com`"
    );
  }
  return email;
}
