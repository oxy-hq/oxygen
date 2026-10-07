import { type AgentStanding, agentLimits, heldStanding } from "@/libs/agentToken";
import { hoursProblem } from "@/libs/sandboxAgentToken";
import type { AgentTokenLimits, CliAgentMintRequest, TokenOptions } from "@/types/apiToken";
import type { AgentAsk } from "./cliAuthRequest";

/** Why a part of the request can't be granted, or `null`. Each sits under the value it is about. */
interface AgentProblems {
  hours: string | null;
  name: string | null;
}

/**
 * What the approval of an agent token shows, and whether it may be given: oxyc's request read
 * against what the signed-in person holds and the server's limits.
 */
export interface AgentReview {
  /** `null` when `hours` was sent and is no whole number, or the server names no default. */
  hours: number | null;
  /** The name the token will carry: the one asked for, or the server's own for this computer. */
  name: string;
  /** Whether oxyc asked for standing, and what the person holds that it would carry. */
  standing: { asked: boolean; held: AgentStanding[] };
  problems: AgentProblems;
  /**
   * The server names no limits for an agent token, so it can't mint one. Running oxyc again
   * changes nothing, which is why this is not one of `problems`.
   */
  unsupported: boolean;
  approvable: boolean;
}

/** The server's limit on any token's name (as `TokenName` has it for a rename). */
const NAME_MAX = 100;

/** What the server names a token oxyc sent no name for. */
const defaultName = (hostname: string): string => `agent on ${hostname}`;

const hoursAsked = (ask: AgentAsk, defaultHours: number | undefined): number | null => {
  if (ask.hours === null) return defaultHours ?? null;
  const text = ask.hours.trim();
  return /^\d+$/.test(text) ? Number(text) : null;
};

/** Hours oxyc sent that are no number, or out of the server's range. None sent is no problem. */
const lifetimeProblem = (
  ask: AgentAsk,
  hours: number | null,
  limits: AgentTokenLimits | undefined
): string | null => {
  if (hours === null) return ask.hours === null ? null : "Not a whole number of hours.";
  return limits ? hoursProblem(hours, limits, "An agent token") : null;
};

/**
 * Read oxyc's request against what the person may approve. Nothing is corrected silently: a
 * lifetime out of range or a name too long is a problem to show, and the request is then not
 * approvable as it stands. Standing is never a problem: asked for by someone who holds none, the
 * token simply carries none, and the approval says so.
 */
export const reviewAgentAsk = (
  ask: AgentAsk,
  hostname: string,
  options: Pick<TokenOptions, "agent" | "can_platform" | "can_partner">
): AgentReview => {
  const limits = agentLimits(options);
  const hours = hoursAsked(ask, limits?.default_hours);
  const asked = ask.name?.trim() || null;

  const problems: AgentProblems = {
    hours: lifetimeProblem(ask, hours, limits),
    // Counted as the rename box's `maxLength` counts it. A name the server picks is its own.
    name:
      asked && asked.length > NAME_MAX
        ? `The token's name is longer than ${NAME_MAX} characters.`
        : null
  };

  return {
    hours,
    name: asked ?? defaultName(hostname),
    standing: { asked: ask.standing, held: heldStanding(options) },
    problems,
    unsupported: !limits,
    approvable: Boolean(limits) && hours !== null && !problems.hours && !problems.name
  };
};

/**
 * The `mint` of the authorize body, or `undefined` when the request can't be approved.
 *
 * `standing` is sent only when oxyc asked for it, the person holds some, and they ticked the box
 * themselves: it starts empty, so approving without reading sends `false`. Someone who holds
 * none sends `false` too, so what the approval says the token carries is what was asked of the
 * server, whatever they come to hold between reading it and the click.
 *
 * `name` goes only when oxyc sent one: the default is the server's to give.
 */
export const agentMint = (
  review: AgentReview,
  ask: AgentAsk,
  includeStanding: boolean
): CliAgentMintRequest | undefined => {
  if (!review.approvable || review.hours === null) return undefined;
  const name = ask.name?.trim();
  return {
    kind: "agent",
    standing: review.standing.asked && review.standing.held.length > 0 && includeStanding,
    expires_in_hours: review.hours,
    ...(name ? { name } : {})
  };
};
