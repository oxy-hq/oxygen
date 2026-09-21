import { Search } from "lucide-react";
import { useEffect, useState } from "react";
import { Input } from "@/components/ui/shadcn/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import TablePagination from "@/components/ui/TablePagination";
import { useExplorerRuns, useExplorerThreads } from "@/hooks/api/adminExplorer";
import { cn } from "@/libs/shadcn/utils";
// Reuse the Explorer's row tables + filter vocabulary — same shape, now scoped
// server-side to one org via the `orgId` filter. The "· Org" column reads as a
// constant here, but the workspace half still varies and is useful at a glance.
import { RunsTable } from "@/pages/admin/AdminExplorer/components/RunsTable";
import { ThreadsTable } from "@/pages/admin/AdminExplorer/components/ThreadsTable";
import {
  ALL,
  filterValue,
  RUN_SOURCE_TYPES,
  RUN_STATUSES,
  THREAD_SOURCE_TYPES,
  THREAD_STATUSES
} from "@/pages/admin/AdminExplorer/constants";
import { useDebounced } from "@/pages/admin/AdminExplorer/useDebounced";
import { AdminAsync } from "../../../components/AdminAsync";

const PAGE_SIZE = 15;
type Resource = "runs" | "threads";
const RESOURCES: Resource[] = ["runs", "threads"];

/**
 * The org-360 Activity tab: an Explorer scoped to this tenant. Answers "what
 * has this customer been running, and what broke?" without leaving the org
 * page or hand-filtering the cross-tenant Explorer. Correctness depends on the
 * server-side `orgId` filter — a client-side filter would miss a dormant
 * tenant's older rows.
 */
export const OrgActivityTab = ({ orgId }: { orgId: string }) => {
  const [resource, setResource] = useState<Resource>("runs");
  const [raw, setRaw] = useState("");
  const [status, setStatus] = useState(ALL);
  const [sourceType, setSourceType] = useState(ALL);
  const [page, setPage] = useState(1);
  const search = useDebounced(raw, 300);

  // Any filter/tab change invalidates the current page — jump back to page 1.
  // biome-ignore lint/correctness/useExhaustiveDependencies: intentional reset on filter change
  useEffect(() => {
    setPage(1);
  }, [resource, search, status, sourceType]);

  const params = {
    search,
    orgId,
    status: filterValue(status),
    sourceType: filterValue(sourceType),
    page,
    pageSize: PAGE_SIZE
  };
  const threads = useExplorerThreads(params, { enabled: resource === "threads" });
  const runs = useExplorerRuns(params, { enabled: resource === "runs" });
  const active = resource === "threads" ? threads : runs;
  const total = active.data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE));

  const statusOptions = resource === "threads" ? THREAD_STATUSES : RUN_STATUSES;
  const sourceOptions = resource === "threads" ? THREAD_SOURCE_TYPES : RUN_SOURCE_TYPES;

  // Clamp back into range if the match set shrank under us while paging deep.
  useEffect(() => {
    if (page > totalPages) setPage(totalPages);
  }, [page, totalPages]);

  const switchResource = (r: Resource) => {
    setResource(r);
    // Status/source vocabularies differ between threads and runs — reset both.
    setStatus(ALL);
    setSourceType(ALL);
  };

  return (
    <div className='space-y-4'>
      <div className='flex flex-wrap items-center justify-between gap-3'>
        <div className='flex items-center gap-1'>
          {RESOURCES.map((r) => (
            <button
              key={r}
              type='button'
              onClick={() => switchResource(r)}
              className={cn(
                "rounded-md px-3 py-1.5 font-medium text-xs uppercase tracking-wide transition-colors",
                resource === r
                  ? "bg-foreground text-background"
                  : "text-muted-foreground hover:bg-muted hover:text-foreground"
              )}
            >
              {r}
            </button>
          ))}
        </div>
        <div className='flex flex-wrap items-center gap-2'>
          {!active.isPending && active.data ? (
            <span className='text-muted-foreground text-xs tabular-nums'>
              {total} {total === 1 ? "result" : "results"}
            </span>
          ) : null}
          <Select value={status} onValueChange={setStatus}>
            <SelectTrigger size='sm' className='w-36 text-xs' aria-label='Status filter'>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {statusOptions.map((opt) => (
                <SelectItem key={opt.value} value={opt.value}>
                  {opt.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select value={sourceType} onValueChange={setSourceType}>
            <SelectTrigger size='sm' className='w-32 text-xs' aria-label='Source filter'>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {sourceOptions.map((opt) => (
                <SelectItem key={opt.value} value={opt.value}>
                  {opt.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <div className='relative'>
            <Search className='absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground' />
            <Input
              value={raw}
              onChange={(e) => setRaw(e.target.value)}
              placeholder={resource === "threads" ? "Search threads…" : "Search runs…"}
              className='h-8 w-64 pl-7 font-mono text-[11px]'
              aria-label='Search activity'
            />
          </div>
        </div>
      </div>

      {/* Two queries behind one region, only one of them enabled at a time — so the
          gate is handed the ACTIVE one's three states explicitly, which is the shape
          `AdminAsync` documents for a composed source. The tables still read their own
          query, so the render callback ignores the data it is passed. */}
      <AdminAsync
        query={{
          isPending: active.isPending,
          isError: active.isError,
          data: active.data,
          refetch: active.refetch,
          // `error` too: leaving it off is what made this the one call site that showed
          // the generic failure without the server's own message — the detail the kit
          // exists to surface.
          error: active.error
        }}
        noun={resource}
        skeleton={<Skeleton className='h-64 w-full' />}
      >
        {() => (
          <>
            {resource === "threads" ? (
              <ThreadsTable rows={threads.data?.items ?? []} />
            ) : (
              <RunsTable rows={runs.data?.items ?? []} />
            )}
            <TablePagination
              currentPage={page}
              totalPages={totalPages}
              totalItems={total}
              pageSize={PAGE_SIZE}
              onPageChange={setPage}
              itemLabel={resource}
            />
          </>
        )}
      </AdminAsync>
    </div>
  );
};
