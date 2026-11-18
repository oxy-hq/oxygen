import { KeyRound, Plus } from "lucide-react";
import { useState } from "react";
import { useSearchParams } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import {
  usePublishTokens,
  useRevokePublishToken
} from "@/hooks/api/publishTokens/usePublishTokens";
import {
  useRevokeSandboxAgentToken,
  useSandboxAgentTokens
} from "@/hooks/api/sandboxAgentTokens/useSandboxAgentTokens";
import {
  useRevokeStandingToken,
  useStandingTokens
} from "@/hooks/api/standingTokens/useStandingTokens";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { apiErrorCode, apiStatus } from "@/libs/apiError";
import { cn } from "@/libs/shadcn/utils";
import { itemReachable } from "@/pages/admin/AdminLayout/adminNav";
import { SANDBOX_AGENT_TOKEN_LIST_LIMIT } from "@/services/api/sandboxAgentTokens";
import { STANDING_TOKEN_LIST_LIMIT } from "@/services/api/standingTokens";
import type { Token } from "@/types/apiToken";
import type { CreatedPublishToken, PublishToken } from "@/types/publishTokens";
import { AdminAsync } from "../components/AdminAsync";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";
import { ADMIN_TONE } from "../components/adminTone";
import { RevokeTokenConfirm } from "../components/TokenList/RevokeTokenConfirm";
import { TokenListSummary } from "../components/TokenList/TokenListSummary";
import { useRevokeConfirmation } from "../components/TokenList/useRevokeConfirmation";
import { useTokenTrailHref } from "../components/TokenList/useTokenTrailHref";
import CiInstructions from "./components/CiInstructions";
import { CreatedTokenDialog } from "./components/CreatedTokenDialog";
import { NewPublishTokenDialog } from "./components/NewPublishTokenDialog";
import { RevokeTokenDialog } from "./components/RevokeTokenDialog";
import { StandingTokenFilters } from "./components/StandingTokenFilters";
import { AREA, TokensTable } from "./components/TokensTable";
import { filterCounts, filterTokens, NO_FILTER, type TokenFilter } from "./tokenFilter";
import {
  kindInfo,
  parseKind,
  summarizeRows,
  TOKEN_KINDS,
  type TokenKindId,
  type TokenRow,
  toRows
} from "./tokenRows";

/**
 * `/admin/tokens` — every long-lived credential staff can mint or revoke, on one list.
 *
 * It was three pages: App publish tokens (no rail entry; reachable only from a count in an
 * app console), Sandbox agent tokens, and Staff & partner tokens — three answers to one
 * question, "what can act on the platform without a person signed in, and who holds it",
 * that an operator handling a lost laptop or a leaked CI secret had to ask three times.
 *
 * Each kind keeps its own gate. Publish tokens are open to every staff member; sandbox
 * tokens need `operate_platform`; staff & partner tokens `manage_platform_grants` and an
 * unbounded grant. A kind the viewer cannot list is not fetched; one the server refuses
 * anyway is named under the table, once, rather than blanking the page.
 */
const DESCRIPTION = "Credentials that act without a person signed in.";

const INFO =
  "CI publish tokens, the tokens AI agents build sandboxed apps with, and personal tokens carrying staff or partner standing. Revoke one here when a laptop is lost or a secret leaks.";

const REFUSALS: Record<"sandbox" | "standing", (error: unknown) => string> = {
  sandbox: () =>
    "Sandbox agent tokens are not shown: listing every staff member’s needs the operate_platform capability.",
  standing: (error) =>
    apiErrorCode(error) === "unbounded_grant_required"
      ? "Staff & partner tokens are not shown: they work across every organization, and your staff access is limited to some."
      : apiErrorCode(error) === "session_required"
        ? "Staff & partner tokens are not shown: this list needs a browser sign-in, not a session opened from an API token."
        : "Staff & partner tokens are not shown: listing them needs the manage_platform_grants capability."
};

const sandboxWarning = (token: Token | null): string => {
  const stops = "The agent holding it stops working at once, and the token can't be brought back.";
  return token?.owner.label ? `${token.owner.label} minted it. ${stops}` : stops;
};
const standingWarning = (token: Token | null): string => {
  const owner = token?.owner.label;
  return owner
    ? `${owner} owns it. It stops working at once, and they sign in or run oxyc login again to get a new one.`
    : "It stops working at once, and its owner signs in or runs oxyc login again to get a new one.";
};

export default function AdminTokens() {
  const { data: user } = useCurrentUser();
  const standing = {
    isOwner: user?.is_owner ?? false,
    capabilities: user?.platform_capabilities ?? []
  };
  const reachable = TOKEN_KINDS.filter((k) => itemReachable(k.rule, standing)).map((k) => k.id);
  const can = (k: TokenKindId) => reachable.includes(k);

  const [params, setParams] = useSearchParams();
  const kind = parseKind(params.get("kind"), reachable);
  const setKind = (next: TokenKindId | "all") => {
    const update = new URLSearchParams(params);
    if (next === "all") update.delete("kind");
    else update.set("kind", next);
    setParams(update, { replace: true });
  };

  const publish = usePublishTokens({ enabled: can("publish") });
  const sandbox = useSandboxAgentTokens({ enabled: can("sandbox") });
  const standingTokens = useStandingTokens({ enabled: can("standing") });

  const revokePublish = useRevokePublishToken();
  const revokeSandbox = useRevokeSandboxAgentToken();
  const revokeStanding = useRevokeStandingToken();
  const sandboxConfirm = useRevokeConfirmation(revokeSandbox.mutate);
  const standingConfirm = useRevokeConfirmation(revokeStanding.mutate);
  const [publishToRevoke, setPublishToRevoke] = useState<PublishToken | null>(null);

  const trailHref = useTokenTrailHref();
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<CreatedPublishToken | null>(null);
  const [standingFilter, setStandingFilter] = useState<TokenFilter>(NO_FILTER);

  // One query for the frame: pending until every list it asked for has answered; failed
  // only when none answered, so one refused or broken kind never hides the others.
  const sources = [
    ...(can("publish") ? [{ id: "publish" as const, query: publish }] : []),
    ...(can("sandbox") ? [{ id: "sandbox" as const, query: sandbox }] : []),
    ...(can("standing") ? [{ id: "standing" as const, query: standingTokens }] : [])
  ];
  const settled = sources.every((s) => !s.query.isPending);
  const answered = sources.filter((s) => s.query.data);
  const broken = sources.filter((s) => s.query.isError && apiStatus(s.query.error) !== 403);
  const refused = sources.filter((s) => apiStatus(s.query.error) === 403);

  const allRows = toRows({
    publish: can("publish") ? publish.data : undefined,
    sandbox: can("sandbox") ? sandbox.data : undefined,
    standing: can("standing") ? standingTokens.data : undefined
  });
  const counts = Object.fromEntries(
    TOKEN_KINDS.map((k) => [k.id, allRows.filter((r) => r.kind === k.id).length])
  ) as Record<TokenKindId, number>;

  let rows = kind === "all" ? allRows : allRows.filter((r) => r.kind === kind);
  if (kind === "standing") {
    const kept = new Set(
      filterTokens(
        rows.map((r) => r.token as Token),
        standingFilter
      ).map((t) => t.id)
    );
    rows = rows.filter((r) => kept.has(r.id));
  }

  const frame = {
    isPending: !settled,
    isError: settled && answered.length === 0 && broken.length > 0,
    error: broken[0]?.query.error,
    data: settled ? allRows : undefined,
    refetch: () => {
      for (const s of sources) void s.query.refetch();
    }
  };

  const revokingKey = revokePublish.isPending
    ? `publish:${revokePublish.variables}`
    : revokeSandbox.isPending
      ? `sandbox:${revokeSandbox.variables?.id}`
      : revokeStanding.isPending
        ? `standing:${revokeStanding.variables?.id}`
        : null;

  const onRevoke = (row: TokenRow) => {
    if (row.kind === "publish") setPublishToRevoke(row.token);
    else if (row.kind === "sandbox") sandboxConfirm.ask(row.token);
    else standingConfirm.ask(row.token);
  };

  const visibleKinds = TOKEN_KINDS.filter((k) => can(k.id) && !refused.some((r) => r.id === k.id));

  return (
    <AdminPage
      width='wide'
      description={DESCRIPTION}
      info={INFO}
      actions={
        can("publish") ? (
          <Button
            size='sm'
            className='h-7 gap-1.5 px-2.5 text-xs'
            onClick={() => setCreating(true)}
            data-testid={`${AREA}-new`}
          >
            <Plus className='size-3.5' />
            New publish token
          </Button>
        ) : null
      }
      data-testid={AREA}
    >
      <AdminAsync
        query={frame}
        noun='tokens'
        rows={5}
        isEmpty={(data) => data.length === 0 && refused.length === 0}
        empty={
          <AdminEmptyState
            icon={KeyRound}
            title='No tokens yet.'
            description='A publish token appears when someone creates one for CI, a sandbox token when an engineer approves an agent, and a staff token when someone runs oxyc login.'
            data-testid={`${AREA}-empty`}
          />
        }
      >
        {() => (
          <div className='space-y-3'>
            <TokenListSummary area={AREA} summary={summarizeRows(allRows)} />

            {visibleKinds.length > 1 ? (
              <fieldset
                className='m-0 flex min-w-0 flex-wrap items-center gap-1 border-0 p-0'
                data-testid={`${AREA}-kinds`}
              >
                <legend className='sr-only'>Kind</legend>
                <KindChip
                  label='All'
                  count={allRows.length}
                  on={kind === "all"}
                  onClick={() => setKind("all")}
                  testId='all'
                />
                {visibleKinds.map((k) => (
                  <KindChip
                    key={k.id}
                    label={k.label}
                    count={counts[k.id]}
                    on={kind === k.id}
                    onClick={() => setKind(k.id)}
                    testId={k.id}
                  />
                ))}
              </fieldset>
            ) : null}

            {kind === "standing" && standingTokens.data ? (
              <StandingTokenFilters
                filter={standingFilter}
                counts={filterCounts(standingTokens.data, standingFilter)}
                onChange={setStandingFilter}
              />
            ) : null}

            {rows.length === 0 ? (
              <p
                className='rounded-md border border-border/60 border-dashed px-4 py-6 text-center text-muted-foreground text-xs'
                data-testid={`${AREA}-none`}
              >
                {kind === "all"
                  ? "No tokens to show."
                  : `No ${kindInfo(kind).label.toLowerCase()} tokens.`}
              </p>
            ) : (
              <TokensTable
                rows={rows}
                fetched={Math.max(sandbox.data?.length ?? 0, standingTokens.data?.length ?? 0)}
                limit={Math.min(SANDBOX_AGENT_TOKEN_LIST_LIMIT, STANDING_TOKEN_LIST_LIMIT)}
                revokingKey={revokingKey}
                onRevoke={onRevoke}
                trailHref={trailHref}
              />
            )}

            {refused.length > 0 || broken.length > 0 ? (
              <ul className='space-y-0.5 text-xs' data-testid={`${AREA}-notes`}>
                {refused.map((s) => (
                  <li
                    key={s.id}
                    className='text-muted-foreground'
                    data-testid={`${AREA}-refused-${s.id}`}
                  >
                    {s.id === "publish"
                      ? "App publish tokens are not shown."
                      : REFUSALS[s.id](s.query.error)}
                  </li>
                ))}
                {broken.map((s) => (
                  <li
                    key={s.id}
                    className={ADMIN_TONE.warn.text}
                    data-testid={`${AREA}-broken-${s.id}`}
                  >
                    Couldn’t load {kindInfo(s.id).label.toLowerCase()} tokens.{" "}
                    <button
                      type='button'
                      className='font-medium underline-offset-2 hover:underline'
                      onClick={() => s.query.refetch()}
                    >
                      Retry
                    </button>
                  </li>
                ))}
              </ul>
            ) : null}

            {kind === "publish" ? <CiInstructions /> : null}
          </div>
        )}
      </AdminAsync>

      <NewPublishTokenDialog open={creating} onOpenChange={setCreating} onCreated={setCreated} />
      <CreatedTokenDialog token={created} onClose={() => setCreated(null)} />
      <RevokeTokenDialog
        token={publishToRevoke}
        isRevoking={revokePublish.isPending}
        onOpenChange={(open) => {
          if (!open && !revokePublish.isPending) setPublishToRevoke(null);
        }}
        onConfirm={() => {
          if (!publishToRevoke) return;
          revokePublish.mutate(publishToRevoke.id, { onSettled: () => setPublishToRevoke(null) });
        }}
      />
      <RevokeTokenConfirm
        area={`${AREA}-sandbox`}
        confirmation={sandboxConfirm}
        warning={sandboxWarning}
      />
      <RevokeTokenConfirm
        area={`${AREA}-standing`}
        confirmation={standingConfirm}
        warning={standingWarning}
      />
    </AdminPage>
  );
}

const KindChip = ({
  label,
  count,
  on,
  onClick,
  testId
}: {
  label: string;
  count: number;
  on: boolean;
  onClick: () => void;
  testId: string;
}) => (
  <button
    type='button'
    onClick={onClick}
    aria-pressed={on}
    data-testid={`${AREA}-kind-${testId}`}
    className={cn(
      "rounded-md px-2 py-1 font-medium text-[11px] transition-colors",
      on
        ? "bg-foreground text-background"
        : "text-muted-foreground hover:bg-muted hover:text-foreground"
    )}
  >
    {label} <span className='tabular-nums opacity-70'>{count}</span>
  </button>
);
