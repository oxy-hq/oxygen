import { ArrowDown, ArrowUp, Search, X } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { usePartnerAudit, usePartnerOrgs } from "@/hooks/api/partners";
import { cn } from "@/libs/shadcn/utils";
import { timeAgo } from "@/libs/utils/date";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";
import type { PartnerAuditEvent } from "@/types/partners";
import PageShell from "../components/PageShell";
import { usePartnerConsole } from "../context";

type SortKey = "created_at" | "actor_email" | "action";
type SortDir = "asc" | "desc";
const ALL = "__all__";

/**
 * The activity log, as something you can work with.
 *
 * It was a raw dump: no search, no sort, and a target column of bare UUIDs — so
 * "who touched Northwind last week?" was unanswerable without copying ids around.
 * An audit log nobody can query is a compliance artifact, not a tool.
 *
 * Filtering is client-side because the server already hands over the whole window
 * (capped at 1000), so it's instant and costs no round-trip per keystroke. If that
 * cap is ever raised meaningfully, this moves into the query.
 */
export default function PartnerActivity() {
  const { active } = usePartnerConsole();
  const partnerId = active.partner_id;

  const { data: events, isLoading, error } = usePartnerAudit(partnerId);
  const { data: clients } = usePartnerOrgs(partnerId);

  const [q, setQ] = useState("");
  const [client, setClient] = useState(ALL);
  const [actor, setActor] = useState(ALL);
  const [kind, setKind] = useState(ALL);
  const [sort, setSort] = useState<{ key: SortKey; dir: SortDir }>({
    key: "created_at",
    dir: "desc"
  });

  // org_id → name. The single change that makes this table readable.
  const clientName = useMemo(
    () => new Map((clients ?? []).map((c) => [c.org_id, c.name] as const)),
    [clients]
  );

  // Facets come from the data, so we never offer a filter that matches nothing.
  const actors = useMemo(
    () => [...new Set((events ?? []).map((e) => e.actor_email))].sort(),
    [events]
  );
  const kinds = useMemo(
    () => [...new Set((events ?? []).map((e) => e.action.split(".")[0]))].sort(),
    [events]
  );

  const visible = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const rows = (events ?? []).filter((e) => {
      if (client !== ALL && e.org_id !== client) return false;
      if (actor !== ALL && e.actor_email !== actor) return false;
      if (kind !== ALL && !e.action.startsWith(`${kind}.`)) return false;
      if (!needle) return true;
      const org = e.org_id ? (clientName.get(e.org_id) ?? "") : "";
      return [e.action, e.actor_email, e.target_label ?? "", org].some((f) =>
        f.toLowerCase().includes(needle)
      );
    });

    const dir = sort.dir === "asc" ? 1 : -1;
    return [...rows].sort((a, b) => {
      const av = a[sort.key] ?? "";
      const bv = b[sort.key] ?? "";
      return av < bv ? -dir : av > bv ? dir : 0;
    });
  }, [events, q, client, actor, kind, sort, clientName]);

  const filtering = !!q || client !== ALL || actor !== ALL || kind !== ALL;
  const clearAll = () => {
    setQ("");
    setClient(ALL);
    setActor(ALL);
    setKind(ALL);
  };

  return (
    <PageShell
      eyebrow={active.name}
      title='Activity'
      description='Every permission change and client action in your subtree, recorded against the person who made it.'
      testId='partner-activity-page'
      actions={
        events?.length ? (
          <span className='text-muted-foreground text-xs tabular-nums'>
            {filtering ? `${visible.length} of ${events.length}` : events.length} event
            {events.length === 1 ? "" : "s"}
          </span>
        ) : undefined
      }
    >
      {isLoading ? (
        <Skeleton className='h-64 w-full' />
      ) : error ? (
        <p className='text-destructive text-sm'>Failed to load the activity log.</p>
      ) : !events?.length ? (
        <p className='text-muted-foreground text-sm'>No activity has been recorded yet.</p>
      ) : (
        <div className='space-y-3'>
          <div className='flex flex-wrap items-center gap-2'>
            <div className='relative'>
              <Search className='absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground' />
              <Input
                value={q}
                onChange={(e) => setQ(e.target.value)}
                placeholder='Search action, person, target…'
                className='h-8 w-64 pl-7 text-xs'
                data-testid='partner-activity-search'
              />
            </div>
            <Facet
              id='client'
              value={client}
              onChange={setClient}
              placeholder='All clients'
              options={(clients ?? []).map((c) => ({ value: c.org_id, label: c.name }))}
            />
            <Facet
              id='actor'
              value={actor}
              onChange={setActor}
              placeholder='Anyone'
              options={actors.map((a) => ({ value: a, label: a }))}
            />
            <Facet
              id='kind'
              value={kind}
              onChange={setKind}
              placeholder='Any action'
              options={kinds.map((k) => ({ value: k, label: k }))}
            />
            {filtering && (
              <Button
                variant='ghost'
                size='sm'
                className='h-8 gap-1 text-xs'
                onClick={clearAll}
                data-testid='partner-activity-clear'
              >
                <X className='size-3.5' />
                Clear
              </Button>
            )}
          </div>

          <Table>
            <TableHeader>
              <TableRow className={ADMIN_HEADER_ROW_CLASS}>
                <SortTh sortKey='created_at' sort={sort} setSort={setSort}>
                  When
                </SortTh>
                <SortTh sortKey='actor_email' sort={sort} setSort={setSort}>
                  Who
                </SortTh>
                <SortTh sortKey='action' sort={sort} setSort={setSort}>
                  Did what
                </SortTh>
                <AdminTh>Client</AdminTh>
                <AdminTh>Target</AdminTh>
                <AdminTh>Outcome</AdminTh>
              </TableRow>
            </TableHeader>
            <TableBody>
              {visible.length === 0 ? (
                <TableRow>
                  <TableCell
                    colSpan={6}
                    className='py-10 text-center text-muted-foreground text-sm'
                  >
                    No events match these filters.
                  </TableCell>
                </TableRow>
              ) : (
                visible.map((e) => <EventRow key={e.id} event={e} clientName={clientName} />)
              )}
            </TableBody>
          </Table>
        </div>
      )}
    </PageShell>
  );
}

function EventRow({
  event: e,
  clientName
}: {
  event: PartnerAuditEvent;
  clientName: Map<string, string>;
}) {
  return (
    <TableRow className='border-border-subtle'>
      <TableCell className='whitespace-nowrap text-muted-foreground text-xs'>
        {/* Relative time is what you scan; the exact stamp is what you cite. */}
        <Tooltip>
          <TooltipTrigger asChild>
            <span>{timeAgo(e.created_at)}</span>
          </TooltipTrigger>
          <TooltipContent className={DESIGN_REFERENCE}>
            {new Date(e.created_at).toLocaleString()}
          </TooltipContent>
        </Tooltip>
      </TableCell>
      <TableCell className='max-w-0 truncate text-sm'>{e.actor_email}</TableCell>
      <TableCell className='whitespace-nowrap font-mono text-xs'>{e.action}</TableCell>
      <TableCell className='text-sm'>
        {e.org_id ? (clientName.get(e.org_id) ?? <ShortId id={e.org_id} />) : "—"}
      </TableCell>
      <TableCell className='max-w-0 truncate text-muted-foreground text-sm'>
        {e.target_label || "—"}
      </TableCell>
      <TableCell>
        <Badge
          variant={e.outcome === "success" ? "secondary" : "destructive"}
          className='font-normal'
        >
          {e.outcome}
        </Badge>
      </TableCell>
    </TableRow>
  );
}

/** An org with events but no name here — e.g. one you're no longer assigned to. */
function ShortId({ id }: { id: string }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className='text-muted-foreground'>{id.slice(0, 8)}</span>
      </TooltipTrigger>
      <TooltipContent className={DESIGN_REFERENCE}>{id}</TooltipContent>
    </Tooltip>
  );
}

function Facet({
  id,
  value,
  onChange,
  placeholder,
  options
}: {
  /** Stable name for the testid — the placeholder is copy. */
  id: string;
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
  options: { value: string; label: string }[];
}) {
  if (options.length === 0) return null;
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger className='h-8 w-40 text-xs' data-testid={`partner-activity-facet-${id}`}>
        <SelectValue placeholder={placeholder} />
      </SelectTrigger>
      <SelectContent className={DESIGN_REFERENCE}>
        <SelectItem value={ALL} data-testid={`partner-activity-facet-${id}-all`}>
          {placeholder}
        </SelectItem>
        {options.map((o) => (
          <SelectItem
            key={o.value}
            value={o.value}
            data-testid={`partner-activity-facet-${id}-${o.value}`}
          >
            {o.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

function SortTh({
  sortKey,
  sort,
  setSort,
  children
}: {
  sortKey: SortKey;
  sort: { key: SortKey; dir: SortDir };
  setSort: (s: { key: SortKey; dir: SortDir }) => void;
  children: ReactNode;
}) {
  const active = sort.key === sortKey;
  return (
    <TableHead
      className={cn("p-0 font-medium text-[10px] text-muted-foreground uppercase tracking-wider")}
    >
      <button
        type='button'
        data-testid={`partner-activity-sort-${sortKey}`}
        className='flex h-full w-full items-center gap-1 px-2 py-2 hover:text-foreground'
        // Re-clicking the active column flips it; a fresh column starts newest /
        // A-first, which is what you want the first time you click it.
        onClick={() =>
          setSort({ key: sortKey, dir: active && sort.dir === "desc" ? "asc" : "desc" })
        }
      >
        {children}
        {active &&
          (sort.dir === "desc" ? <ArrowDown className='size-3' /> : <ArrowUp className='size-3' />)}
      </button>
    </TableHead>
  );
}
