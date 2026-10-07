/**
 * Is what the exchange returned THE AGENT TOKEN THAT WAS ASKED FOR? Decided
 * before a byte of it is printed, by `oxyc tokens create --agent`.
 *
 * The stakes are the sandbox mint's, turned up. A deployment that does not
 * know the kind may answer the same exchange with an ordinary login: the same
 * `oxy_pat_` prefix, ninety days long, carrying the approver's staff standing
 * whether or not anyone asked. The prefix cannot tell the two apart, so the
 * token is asked what it is.
 *
 * Accepted only when ALL of these hold:
 *
 *   1. the secret is a whole `oxy_pat_`;
 *   2. the token describes itself (`GET /api/auth/token`) as `kind:
 *      "personal"` with `source: "oxyc_agent"`;
 *   3. it expires, and no later than the lifetime asked for;
 *   4. it carries no staff or partner standing unless `--standing` was passed.
 *
 * LESS THAN ASKED IS FINE; MORE NEVER IS. The approver may leave standing out
 * on the page, so a token without it is accepted when it was asked for — and
 * the command says so. A shorter life than asked is accepted the same way.
 *
 * WHAT CANNOT BE READ IS REFUSED, as in `tokens-sandbox-verify.ts`: a standing
 * flag that is not a boolean, an expiry that does not parse, a description
 * that does not come back. The read-back is asked for every field. The
 * exchange's own description is checked too, wherever it speaks: it may say
 * less, and may never say otherwise.
 */

import { AGENT_TOKEN_SOURCE } from "../auth/agent-token.js";
import type { Minted } from "../auth/login.js";
import { introspectToken, type Token } from "../auth/token-api.js";
import { isWellFormedPersonalToken, PERSONAL_TOKEN_PREFIX } from "../auth/token-kind.js";
import { lifetimeMismatch, type Mismatch, notAsked, unconfirmed } from "./tokens-sandbox-verify.js";

export interface AgentAsked {
  hours: number;
  /** `--standing`: the token may carry the approver's staff or partner standing. */
  standing: boolean;
}

/** The standing a token carries, in the words a person reads. */
export type Standing = "staff" | "partner";

export type AgentVerdict =
  /** Refuse it: nothing is printed, and what was minted is revoked. */
  | { problem: Mismatch }
  /** It is the token asked for. `standing` is what it actually carries. */
  | { token: Token; standing: Standing[] };

/** The headline for a deployment that predates the kind altogether. */
export const unsupportedAgent = (target: string) => `${target} does not support agent tokens yet`;

type Described = Record<string, unknown>;

const quoted = (value: unknown) => JSON.stringify(value ?? null);

/** Why the row is not an agent token's, if it is not. `who` is whose word this is. */
function identityProblem(
  target: string,
  row: Described,
  who: string,
  strict: boolean
): Mismatch | undefined {
  if ((strict || row.kind !== undefined) && row.kind !== "personal") {
    return {
      what: unsupportedAgent(target),
      why: `${who} described the token as kind ${quoted(row.kind)}, not "personal"`
    };
  }
  if ((strict || row.source !== undefined) && row.source !== AGENT_TOKEN_SOURCE) {
    const login = row.source === "oxyc_login" ? " — an ordinary `oxyc login` token" : "";
    return {
      what: unsupportedAgent(target),
      why: `${who} described the token's source as ${quoted(row.source)}, not "${AGENT_TOKEN_SOURCE}"${login}`
    };
  }
  return undefined;
}

/** Why the standing is more than was asked for, or cannot be told, if either. */
function standingProblem(
  target: string,
  ask: AgentAsked,
  row: Described,
  strict: boolean
): Mismatch | undefined {
  const flags: [Standing, unknown][] = [
    ["staff", row.platform],
    ["partner", row.partner]
  ];
  for (const [name, flag] of flags) {
    if (flag === undefined && !strict) continue;
    if (typeof flag !== "boolean") {
      return {
        what: unconfirmed(target),
        why: `whether it carries ${name} standing cannot be read (${quoted(flag)})`
      };
    }
    if (flag && !ask.standing) {
      return {
        what: notAsked(target),
        why: `it carries ${name} standing, and none was asked for (--standing was not passed)`
      };
    }
  }
  return undefined;
}

/**
 * Everything one description says, against what was asked. `strict` is the
 * read-back: every field must be there. Not strict is the exchange's own
 * description, which is held only to what it does say.
 */
function describedProblem(
  target: string,
  ask: AgentAsked,
  row: Described,
  who: string,
  strict: boolean,
  now: number
): Mismatch | undefined {
  return (
    identityProblem(target, row, who, strict) ??
    (strict || row.expires_at !== undefined
      ? lifetimeMismatch(target, ask, row.expires_at, now)
      : undefined) ??
    standingProblem(target, ask, row, strict)
  );
}

/**
 * Whether the minted credential is the agent token asked for, and what
 * standing it carries. `now` is a parameter for the tests.
 */
export async function verifyAgentToken(
  target: string,
  ask: AgentAsked,
  minted: Minted,
  now: number = Date.now()
): Promise<AgentVerdict> {
  if (!minted.token.startsWith(PERSONAL_TOKEN_PREFIX)) {
    return {
      problem: {
        what: notAsked(target),
        why: "the secret it returned is not a personal access token (`oxy_pat_…`), which is what an agent token is"
      }
    };
  }
  // The prefix chose the path; the whole shape is what may be printed. The
  // secret ends up in a line a shell evaluates, so it is letters and digits or
  // it is refused, and it is never quoted back in the message.
  if (!isWellFormedPersonalToken(minted.token)) {
    return {
      problem: {
        what: notAsked(target),
        why: "the secret it returned is not a whole personal access token (`oxy_pat_` and 36 letters or digits)"
      }
    };
  }

  const exchanged = describedProblem(
    target,
    ask,
    minted.described ?? {},
    "its exchange",
    false,
    now
  );
  if (exchanged) return { problem: exchanged };

  // THE TOKEN'S OWN WORD, always: asked with the token itself, so it is also
  // proof the deployment accepts what it just minted.
  const found = await introspectToken(target, minted.token);
  if (found.kind !== "token") {
    const why =
      found.kind === "session"
        ? "the deployment says there is no token behind the secret it returned"
        : `its description could not be read back (GET /api/auth/token answered ${found.status || "nothing"})`;
    return { problem: { what: unconfirmed(target), why } };
  }
  const row = found.token as unknown as Described;
  const read = describedProblem(target, ask, row, "the token", true, now);
  if (read) return { problem: read };

  const standing: Standing[] = [
    ...(row.platform === true ? (["staff"] as const) : []),
    ...(row.partner === true ? (["partner"] as const) : [])
  ];
  return { token: found.token, standing };
}
