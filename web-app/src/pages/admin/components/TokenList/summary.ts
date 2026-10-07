import type { Token } from "@/types/apiToken";
import { tokenState } from "./tokenState";

/** A staff token list in one sentence, as `TokenListSummary` sets it. */
export interface TokenSummaryLine {
  /** What works right now: set in the foreground. */
  lead: string;
  /** Whose the live ones are, and how many have ended. May be empty. */
  rest: string;
}

type Tallied = Pick<Token, "status" | "expires_at" | "owner">;

export interface TokenTally<T> {
  /** The tokens that work now. One that lapsed since the fetch is not among them. */
  live: T[];
  /** How many of the listed tokens have expired or been revoked. */
  ended: number;
  /** How many different people own a live token. */
  people: number;
}

/** A list keeps its ended tokens, so "nothing is live" is not the same as "nothing listed". */
export const tallyTokens = <T extends Tallied>(tokens: T[]): TokenTally<T> => {
  const live = tokens.filter((token) => tokenState(token) === "active");
  return {
    live,
    ended: tokens.length - live.length,
    people: new Set(live.map((token) => token.owner.id)).size
  };
};

/** "1 token is live", "4 tokens are live". */
export const liveLead = (live: number): string =>
  live === 1 ? "1 token is live" : `${live} tokens are live`;

/** "one person", "3 people". */
export const peoplePhrase = (people: number): string =>
  people === 1 ? "one person" : `${people} people`;

/**
 * The sentence about the ended tokens, with its leading space. After the live ones it counts
 * "more"; with nothing live, the ended ones are all there is below. Empty when none has ended.
 */
export const endedSentence = (ended: number, anyLive: boolean): string => {
  if (ended === 0) return "";
  if (anyLive) return ` ${ended} more ${ended === 1 ? "has" : "have"} expired or been revoked.`;
  return ended === 1
    ? " The one below has expired or been revoked."
    : ` The ${ended} below have expired or been revoked.`;
};
