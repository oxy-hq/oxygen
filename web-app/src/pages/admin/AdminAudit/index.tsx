import { KeyRound, ScrollText, X } from "lucide-react";
import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useAuditSearch } from "@/hooks/api/audit";
import { AdminAsync } from "../components/AdminAsync";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";
import { auditCredential, tokenIdParam } from "./auditCredential";
import AuditTable from "./components/AuditTable";

const LIMIT = 200;

/** Debounce a fast-changing string (e.g. a search box) by `ms`. */
function useDebounced(value: string, ms = 300): string {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setDebounced(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return debounced;
}

/**
 * Platform audit log (`/admin/audit`, Oxy staff). Free-text search + action /
 * outcome facets over the append-only `audit_events` stream, newest first.
 *
 * `?token_id=` narrows it to one API token: what was done with it, and its own
 * lifecycle events. It lives in the URL so a row elsewhere can link straight to
 * a token's trail, and a row here sets it from its detail.
 */
export default function AdminAudit() {
  const [searchParams, setSearchParams] = useSearchParams();
  const tokenId = tokenIdParam(searchParams.get("token_id"));
  const setTokenId = (id: string | null) => {
    setSearchParams(
      (current) => {
        const next = new URLSearchParams(current);
        if (id) next.set("token_id", id);
        else next.delete("token_id");
        return next;
      },
      { replace: true }
    );
  };

  const [qInput, setQInput] = useState("");
  const [actionInput, setActionInput] = useState("");
  const [outcome, setOutcome] = useState("all");

  const q = useDebounced(qInput);
  const action = useDebounced(actionInput);

  // The whole query: an audit search that failed and one that legitimately
  // matched nothing must not read the same on a console used for incident
  // triage. `keepPreviousData` on the hook means a filter change keeps the old
  // page on screen rather than flashing the skeleton, exactly as before.
  const events = useAuditSearch({
    q: q || undefined,
    action: action || undefined,
    outcome: outcome === "all" ? undefined : outcome,
    token_id: tokenId,
    limit: LIMIT
  });

  // The filter is an id. Its name is read off a loaded row that names the same token, and
  // until one does the chip shows the start of the id.
  const tokenName = tokenId
    ? events.data
        ?.map(auditCredential)
        .find((credential) => credential?.id === tokenId && credential.name)?.name
    : undefined;

  const hasFilters = !!q || !!action || outcome !== "all" || !!tokenId;
  const clear = () => {
    setQInput("");
    setActionInput("");
    setOutcome("all");
    setTokenId(null);
  };

  return (
    <AdminPage
      // Was `max-w-[100rem]`, an arbitrary width off the kit's scale; `full` is
      // the closest role — this is a six-column stream an operator scans wide.
      width='full'
      description={
        <>
          Every privileged action across the platform — partner grants, member changes, custom-app
          deploys — newest first.
        </>
      }
      data-testid='admin-audit'
    >
      <div className='flex flex-wrap items-center gap-2'>
        <Input
          placeholder='Search action, actor, or target…'
          value={qInput}
          onChange={(e) => setQInput(e.target.value)}
          className='max-w-xs'
        />
        <Input
          placeholder='Action (e.g. partner.member.added)'
          value={actionInput}
          onChange={(e) => setActionInput(e.target.value)}
          className='max-w-xs'
        />
        <Select value={outcome} onValueChange={setOutcome}>
          <SelectTrigger className='w-36'>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value='all'>All outcomes</SelectItem>
            <SelectItem value='success'>Success</SelectItem>
            <SelectItem value='failure'>Failure</SelectItem>
          </SelectContent>
        </Select>
        {tokenId && (
          <Badge
            variant='secondary'
            className='h-8 gap-1.5 pr-1 font-normal text-xs'
            title={tokenId}
            data-testid='admin-audit-token-filter'
          >
            <KeyRound className='size-3 text-muted-foreground' aria-hidden />
            <span className='text-muted-foreground'>Token</span>
            <span className={tokenName ? "font-medium" : "font-mono"}>
              {tokenName ?? tokenId.slice(0, 8)}
            </span>
            <Button
              variant='ghost'
              size='icon'
              className='size-5'
              onClick={() => setTokenId(null)}
              aria-label='Show every token again'
              data-testid='admin-audit-token-filter-clear'
            >
              <X className='size-3' />
            </Button>
          </Badge>
        )}
        {hasFilters && (
          <Button variant='ghost' size='sm' onClick={clear}>
            Clear
          </Button>
        )}
      </div>

      <AdminAsync
        query={events}
        noun='the audit log'
        // One block, matching the table it replaces.
        skeleton={<Skeleton className='h-64 w-full' />}
        isEmpty={(rows) => rows.length === 0}
        empty={<AdminEmptyState icon={ScrollText} title='No events match these filters.' />}
      >
        {(rows) => (
          <AuditTable events={rows} limit={LIMIT} tokenId={tokenId} onFilterToken={setTokenId} />
        )}
      </AdminAsync>
    </AdminPage>
  );
}
