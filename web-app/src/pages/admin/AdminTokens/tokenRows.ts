import { tokenState } from "@/libs/tokenState";
import type { AdminNavRule } from "@/pages/admin/AdminLayout/adminNav";
import {
  endedSentence,
  liveLead,
  type TokenSummaryLine
} from "@/pages/admin/components/TokenList/summary";
import type { Token } from "@/types/apiToken";
import type { PublishToken } from "@/types/publishTokens";

/**
 * One list of every long-lived credential staff can see — the three kinds that used to
 * be three pages, normalised to one row shape so they sort, count and filter together.
 *
 * Pure: the page reads three endpoints and hands them here; what is live, what order,
 * and what the sentence says are decided once and tested.
 */

export type TokenKindId = "publish" | "sandbox" | "standing";

export interface TokenKindInfo {
  id: TokenKindId;
  /** The chip and the Kind column. */
  label: string;
  /** Who may list this kind — the rule its endpoint enforces. */
  rule: AdminNavRule;
}

/**
 * The kinds, in the order the page lists them, each with the capability its list
 * endpoint gates on: `PlatformApps` for publish tokens, `PlatformOperate` for sandbox
 * agent tokens, and the grant-management capability for staff & partner tokens.
 */
export const TOKEN_KINDS: TokenKindInfo[] = [
  { id: "publish", label: "App publish", rule: { capability: "manage_apps" } },
  { id: "sandbox", label: "Sandbox agent", rule: { capability: "operate_platform" } },
  { id: "standing", label: "Staff & partner", rule: { capability: "manage_platform_grants" } }
];

export const kindInfo = (id: TokenKindId): TokenKindInfo =>
  TOKEN_KINDS.find((k) => k.id === id) as TokenKindInfo;

export type TokenRow =
  | {
      kind: "publish";
      id: string;
      name: string;
      live: boolean;
      createdAt: string;
      token: PublishToken;
    }
  | {
      kind: "sandbox" | "standing";
      id: string;
      name: string;
      live: boolean;
      createdAt: string;
      token: Token;
    };

export interface TokenSources {
  publish?: PublishToken[];
  sandbox?: Token[];
  standing?: Token[];
}

/**
 * Every row, live ones first, newest first within each. A key is `kind:id`, because the
 * three tables number their tokens independently.
 */
export function toRows({ publish, sandbox, standing }: TokenSources): TokenRow[] {
  const rows: TokenRow[] = [
    ...(publish ?? []).map(
      (t): TokenRow => ({
        kind: "publish",
        id: t.id,
        name: t.name,
        live: !t.revoked,
        createdAt: t.created_at,
        token: t
      })
    ),
    ...(sandbox ?? []).map(
      (t): TokenRow => ({
        kind: "sandbox",
        id: t.id,
        name: t.name,
        live: tokenState(t) === "active",
        createdAt: t.created_at,
        token: t
      })
    ),
    ...(standing ?? []).map(
      (t): TokenRow => ({
        kind: "standing",
        id: t.id,
        name: t.name,
        live: tokenState(t) === "active",
        createdAt: t.created_at,
        token: t
      })
    )
  ];
  return rows.sort(
    (a, b) =>
      Number(b.live) - Number(a.live) ||
      new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
  );
}

/** `?kind=` read back, or `"all"` for anything not a kind this viewer can list. */
export function parseKind(value: string | null, reachable: TokenKindId[]): TokenKindId | "all" {
  return reachable.includes(value as TokenKindId) ? (value as TokenKindId) : "all";
}

/**
 * The page's sentence: how many tokens work right now and of which kinds, then how many
 * more have ended. Kinds with nothing live are left out of the breakdown rather than
 * listed as "0 sandbox agent".
 */
export function summarizeRows(rows: TokenRow[]): TokenSummaryLine {
  const live = rows.filter((r) => r.live);
  const ended = rows.length - live.length;
  if (live.length === 0) {
    return { lead: "No token works right now.", rest: endedSentence(ended, false) };
  }
  const byKind = TOKEN_KINDS.map((k) => ({
    label: k.label.toLowerCase(),
    n: live.filter((r) => r.kind === k.id).length
  })).filter((k) => k.n > 0);
  const breakdown =
    byKind.length > 1 ? `: ${byKind.map((k) => `${k.n} ${k.label}`).join(", ")}.` : ".";
  return { lead: liveLead(live.length), rest: `${breakdown}${endedSentence(ended, true)}` };
}
