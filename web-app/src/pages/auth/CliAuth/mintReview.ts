import { appCountProblem, findSandboxApp, hoursProblem } from "@/libs/sandboxAgentToken";
import type { CliSandboxMintRequest, SandboxAgentLimits, SandboxApp } from "@/types/apiToken";
import type { MintAsk } from "./cliAuthRequest";

/** One app oxyc named: found among those the person may mint for, or not. */
export interface MintAppLine {
  /** The `<org>/<app>` reference as oxyc sent it. */
  ref: string;
  /**
   * Absent when the reference names nothing the person may mint for. The server answers 404
   * alike for an app that isn't there and one the person may not build, so nothing says which.
   */
  app?: SandboxApp;
}

/**
 * Why a part of the request can't be granted, or `null`. Each is worded to sit under the value
 * it is about, which the approval shows as asked.
 */
interface MintProblems {
  /** No app named, or more than the limit. An app that doesn't resolve is marked on its line. */
  apps: string | null;
  hours: string | null;
  name: string | null;
}

/**
 * What the approval shows, and whether it may be given: oxyc's request read against the apps the
 * signed-in person may mint for and the server's limits.
 */
export interface MintReview {
  apps: MintAppLine[];
  /** `null` when `hours` was sent and is no whole number. */
  hours: number | null;
  name: string;
  problems: MintProblems;
  /** The `mint` of the authorize body. Present exactly when nothing is wrong. */
  mint?: CliSandboxMintRequest;
}

/** The server's limit on any token's name (as `TokenName` has it for a rename). */
const NAME_MAX = 100;

/** Each app once, in the order asked: two spellings of one app are one grant, not a repeat. */
const resolveApps = (refs: readonly string[], apps: readonly SandboxApp[]): MintAppLine[] => {
  const seen = new Set<string>();
  const lines: MintAppLine[] = [];
  for (const ref of refs) {
    const app = findSandboxApp(apps, ref);
    const key = app ? app.id : ref.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    lines.push({ ref, app });
  }
  return lines;
};

const hoursAsked = (ask: MintAsk, limits: SandboxAgentLimits): number | null => {
  if (ask.hours === null) return limits.default_hours;
  const text = ask.hours.trim();
  return /^\d+$/.test(text) ? Number(text) : null;
};

/**
 * Read oxyc's request against what the person may mint. Nothing is corrected silently: an app
 * that doesn't resolve, a lifetime out of range or too many apps is a problem to show, and the
 * request is then not approvable as it stands. oxyc is run again with the request put right.
 */
export const reviewMintAsk = (
  ask: MintAsk,
  hostname: string,
  apps: readonly SandboxApp[],
  limits: SandboxAgentLimits
): MintReview => {
  const lines = resolveApps(ask.apps, apps);
  const hours = hoursAsked(ask, limits);
  const name = ask.name?.trim() || `Sandbox agent on ${hostname}`;

  const problems: MintProblems = {
    apps:
      lines.length === 0
        ? "A sandbox agent token covers at least one app."
        : appCountProblem(lines.length, limits),
    hours: hours === null ? "Not a whole number of hours." : hoursProblem(hours, limits),
    // Counted as the rename box's `maxLength` counts it, so the two never disagree.
    name: name.length > NAME_MAX ? `The token's name is longer than ${NAME_MAX} characters.` : null
  };

  const resolved = lines.flatMap((line) => line.app ?? []);
  const approvable =
    resolved.length === lines.length && Object.values(problems).every((problem) => !problem);
  return {
    apps: lines,
    hours,
    name,
    problems,
    mint:
      approvable && hours !== null
        ? {
            kind: "sandbox_agent",
            apps: resolved.map((app) => app.id),
            expires_in_hours: hours,
            name
          }
        : undefined
  };
};
