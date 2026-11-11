/**
 * Whether a browser on this machine can complete the loopback login.
 *
 * The loopback flow (`login.ts`) sends the browser back to `127.0.0.1:<port>`
 * on THIS machine. In a Claude Code remote session, over SSH, in a Codespace
 * or on a Linux box with no display, the person's browser is on another
 * machine, so the callback never arrives and `oxyc login` waits five minutes
 * for nothing. There `oxyc login` approves by device code instead, the way
 * `gh auth login` does: it prints a URL and a code, and the person opens the
 * URL on any device.
 *
 * A signal, not a proof. `OXYC_HEADLESS=1` forces the device code where none
 * of these signals is present; `OXYC_HEADLESS=0` (or `--browser`) forces the
 * loopback where one is but a browser can still reach it.
 *
 * CI IS DIFFERENT FROM HEADLESS. A cloud session has a person on the other end
 * of the chat to open the URL; a CI runner has nobody, so a device-code
 * request there just polls for ten minutes and fails. There an approval
 * command refuses at once and names the credentials that need no person
 * (`approveByDeviceCode`) — unless an approver is named, who is told.
 */

import * as log from "../ui/log.js";
import { CliError, ExitCode } from "../util/errors.js";

export const HEADLESS_ENV = "OXYC_HEADLESS";

type Env = Record<string, string | undefined>;

function set(value: string | undefined): boolean {
  return value !== undefined && value !== "" && value !== "0" && value.toLowerCase() !== "false";
}

/**
 * Why no browser here can reach the loopback, or `undefined` when one can.
 * The reason is what `oxyc login` prints when it switches to the device code.
 */
export function headlessReason(
  env: Env = process.env,
  platform: NodeJS.Platform = process.platform
): string | undefined {
  const forced = env[HEADLESS_ENV];
  if (forced !== undefined && forced !== "") {
    return set(forced) ? `${HEADLESS_ENV} is set` : undefined;
  }
  // Before `BROWSER`: a remote session's opener, if it has one, opens nothing
  // the person can see.
  if (set(env.CLAUDE_CODE_REMOTE)) return "this is a Claude Code remote session";
  if (set(env.CODESPACES)) return "this is a GitHub Codespace";
  // An explicit opener is somebody's arrangement for exactly this — VS Code
  // Remote-SSH sets one that opens the laptop's browser and forwards the port.
  if (env.BROWSER) return undefined;
  if (env.SSH_CONNECTION || env.SSH_TTY) return "this is an SSH session";
  // WSL has no X display, yet opens the Windows browser, which shares localhost.
  if (platform === "linux" && !env.DISPLAY && !env.WAYLAND_DISPLAY && !env.WSL_DISTRO_NAME) {
    return "this machine has no display";
  }
  return undefined;
}

/**
 * Whether an exported `OXY_APPROVER` applies to this approval: the device code
 * was CHOSEN, not merely detected. `--no-browser` and `--no-wait` choose it;
 * so does a Claude Code cloud session, whose environment the owner configured
 * (the oxy-custom-apps skill tells them to export the approver there). An
 * engineer's own login over SSH or in a Codespace was only detected, and the
 * variable there is their setting for their agents.
 */
export function exportedApproverApplies(
  browser: boolean | undefined,
  noWait: boolean,
  env: Env = process.env
): boolean {
  if (browser !== undefined) return !browser;
  return noWait || set(env.CLAUDE_CODE_REMOTE);
}

/** The CI system this is, when it is one. Every major one sets `CI`. */
export function ciName(env: Env = process.env): string | undefined {
  if (set(env.GITHUB_ACTIONS)) return "GitHub Actions";
  if (set(env.GITLAB_CI)) return "GitLab CI";
  if (set(env.BUILDKITE)) return "Buildkite";
  if (set(env.CI)) return "CI";
  return undefined;
}

/** What `approveByDeviceCode` is deciding for, in the words it prints. */
export interface ApprovalAsk {
  /** `--no-wait`: a device-code request by definition. */
  noWait?: boolean;
  /** An approver is named (`--approver` or `OXY_APPROVER`): someone will be told. */
  approverNamed?: boolean;
  /** What needs approving: "the login", "the token". */
  what: string;
}

/**
 * Whether an approval (`oxyc login`, `oxyc tokens create --agent`) goes by
 * device code. `browser` is the tri-state flag: `--browser` (true),
 * `--no-browser` (false), neither (undefined: the machine decides).
 *
 * Throws AUTH on a CI runner when nothing was chosen and nobody is named to
 * approve: no one there can open a browser or a URL.
 */
export function approveByDeviceCode(
  browser: boolean | undefined,
  ask: ApprovalAsk,
  env: Env = process.env,
  platform: NodeJS.Platform = process.platform
): boolean {
  if (browser !== undefined) return !browser;
  if (ask.noWait) return true;
  const forced = env[HEADLESS_ENV] !== undefined && env[HEADLESS_ENV] !== "";
  const ci = ciName(env);
  if (ci && !forced && !set(env.CLAUDE_CODE_REMOTE) && !ask.approverNamed) {
    throw new CliError(`nobody on this ${ci} runner can approve ${ask.what}`, {
      code: ExitCode.AUTH,
      detail: "it needs a person to open a browser or a URL, and would wait ten minutes for one.",
      hint:
        "give the job a credential instead: OXY_TOKEN from a CI secret, or in GitHub Actions a service account " +
        "(--service-account / OXY_SERVICE_ACCOUNT, with `permissions: id-token: write`). " +
        "To have a person approve from their device, pass --no-browser --approver <email>"
    });
  }
  const reason = headlessReason(env, platform);
  if (!reason) return false;
  log.info(
    `${reason}, so a browser here cannot finish ${ask.what} — approving by device code instead (\`--browser\` forces the browser)`
  );
  return true;
}
