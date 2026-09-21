import { ScrollText } from "lucide-react";
import { useEffect, useState } from "react";
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
 */
export default function AdminAudit() {
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
    limit: LIMIT
  });

  const hasFilters = !!q || !!action || outcome !== "all";
  const clear = () => {
    setQInput("");
    setActionInput("");
    setOutcome("all");
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
        {(rows) => <AuditTable events={rows} limit={LIMIT} />}
      </AdminAsync>
    </AdminPage>
  );
}
