/**
 * How one approval (`oxyc login`, `oxyc tokens create --agent|--sandbox-agent`)
 * is routed: by browser or device code, and to which approver. One function,
 * so the two commands cannot drift, and so the decision is tested without the
 * argument parser.
 */

import { approverFrom, approverNamed } from "./approver.js";
import { approveByDeviceCode, exportedApproverApplies } from "./headless.js";

type Env = Record<string, string | undefined>;

export interface Approval {
  /** Approve by device code rather than the browser loopback. */
  noBrowser: boolean;
  /** Who the platform tells, when anyone. */
  approver?: string;
}

/**
 * `opts` are the command's parsed options (`browser` tri-state, `approver`);
 * `source` is where commander found `approver` (`"cli"`, `"env"`). Throws the
 * usage and CI refusals `approveByDeviceCode` and `approverFrom` throw.
 */
export function resolveApproval(
  opts: Record<string, unknown>,
  source: string | undefined,
  ask: { noWait?: boolean; what: string },
  env: Env = process.env,
  platform: NodeJS.Platform = process.platform
): Approval {
  const browser = opts.browser as boolean | undefined;
  const noWait = Boolean(ask.noWait);
  const noBrowser = approveByDeviceCode(
    browser,
    { noWait, approverNamed: approverNamed(opts, source), what: ask.what },
    env,
    platform
  );
  // An exported OXY_APPROVER only where the device code was chosen, not detected.
  const approver = approverFrom(
    opts,
    noBrowser,
    source,
    exportedApproverApplies(browser, noWait, env)
  );
  return { noBrowser, approver };
}
