import { useMemo } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle
} from "@/components/ui/shadcn/sheet";
import { useCompileDetail } from "@/hooks/api/compiles";
import type { CompiledEntity } from "@/services/api/compiles";
import { StatusBadge } from "./StatusBadge";

/** Plural labels for each compiled-entity kind the boundary tracks. */
const KIND_LABEL: Record<string, string> = {
  agent: "Agents",
  view: "Views",
  topic: "Topics",
  app: "Data apps",
  automation: "Automations",
  // Legacy kind key, kept so older compile responses still label correctly.
  procedure: "Automations",
  verified_query: "Verified queries",
  pipeline: "Pipelines"
};

/**
 * Per-revision compile detail: WHICH entities compiled (grouped by kind) and
 * WHICH didn't (`error_summary` failures, with the reason). The data already
 * ships from `GET /admin/compiles/{revision_id}`; this is the panel that
 * finally surfaces it. Polling pauses while closed.
 */
export function CompileDetailSheet({
  revisionId,
  open,
  onOpenChange
}: {
  revisionId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { data, isPending } = useCompileDetail(revisionId, { paused: !open });

  const grouped = useMemo(() => {
    const map = new Map<string, CompiledEntity[]>();
    for (const entity of data?.compiled_entities ?? []) {
      const list = map.get(entity.kind) ?? [];
      list.push(entity);
      map.set(entity.kind, list);
    }
    return [...map.entries()];
  }, [data?.compiled_entities]);

  const failures = data?.error_summary?.failures ?? [];
  const fatal = data?.error_summary?.fatal;
  const compiledCount = data?.compiled_entities.length ?? 0;

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className={`${DESIGN_REFERENCE} w-full gap-0 overflow-y-auto sm:max-w-lg`}>
        <SheetHeader>
          <SheetTitle className='flex items-center gap-2'>
            Compile detail
            {data ? <StatusBadge status={data.status} /> : null}
          </SheetTitle>
          <SheetDescription>
            {data
              ? `${data.file_count_compiled} compiled · ${data.file_count_failed} failed · ${data.file_count_seen} seen`
              : "Loading…"}
          </SheetDescription>
        </SheetHeader>

        <div className='space-y-6 px-4 pb-6'>
          {fatal ? (
            <div className=' border border-destructive/30 bg-destructive/5 p-3 text-destructive text-xs'>
              {fatal}
            </div>
          ) : null}

          {failures.length > 0 ? (
            <section>
              <h3 className='font-medium text-destructive text-xs'>Failed ({failures.length})</h3>
              <ul className='mt-2 space-y-2'>
                {failures.map((failure) => (
                  <li key={failure.path} className=' border border-border p-2'>
                    <p className='truncate font-mono text-foreground text-xs' title={failure.path}>
                      {failure.path}
                    </p>
                    <p className='mt-1 text-muted-foreground text-xs'>
                      <span className='bg-muted px-1 py-0.5 font-medium'>{failure.kind}</span>{" "}
                      {failure.message}
                    </p>
                  </li>
                ))}
              </ul>
            </section>
          ) : null}

          <section>
            <h3 className='font-medium text-foreground text-xs'>Compiled ({compiledCount})</h3>
            {isPending ? (
              <p className='mt-2 text-muted-foreground text-xs'>Loading…</p>
            ) : grouped.length === 0 ? (
              <p className='mt-2 text-muted-foreground text-xs'>
                No entities compiled in this revision.
              </p>
            ) : (
              <div className='mt-2 space-y-3'>
                {grouped.map(([kind, list]) => (
                  <div key={kind}>
                    <p className='font-medium text-[11px] text-muted-foreground uppercase tracking-wide'>
                      {KIND_LABEL[kind] ?? kind} ({list.length})
                    </p>
                    <ul className='mt-1 space-y-0.5'>
                      {list.map((entity) => (
                        <li
                          key={`${kind}:${entity.file_path}`}
                          className='flex items-baseline gap-2 text-xs'
                        >
                          <span className='text-foreground'>{entity.name}</span>
                          <span
                            className='truncate font-mono text-[11px] text-muted-foreground/70'
                            title={entity.file_path}
                          >
                            {entity.file_path}
                          </span>
                        </li>
                      ))}
                    </ul>
                  </div>
                ))}
              </div>
            )}
          </section>
        </div>
      </SheetContent>
    </Sheet>
  );
}
