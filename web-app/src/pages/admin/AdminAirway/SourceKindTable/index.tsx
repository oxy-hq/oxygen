import type { ReactNode } from "react";
import { cn } from "@/libs/shadcn/utils";
import { AdminInfo } from "@/pages/admin/components/AdminInfo";
import type { AirwaySourceKindConfig } from "@/services/api/airwayConfig";
import { SourceKindRow } from "../SourceKindRow";
import { SOURCE_KIND_COLUMNS, WIDE_ONLY } from "./layout";

/**
 * Every source kind as one line, expandable in place to edit it.
 *
 * One grid container, with the header and every row as `grid-cols-subgrid` children
 * (`pages/admin/CLAUDE.md`): each row has to own a full-width editor beneath its line,
 * which a `<table>` row cannot hold without a second, unaligned table, and per-row grids
 * would size their `auto` columns row by row. The columns, and how they reflow on a
 * narrow page, are in `layout.ts`.
 */
export function SourceKindTable({ kinds }: { kinds: AirwaySourceKindConfig[] }) {
  return (
    // The wrapper is the query container: the grid sizes its columns from its own width.
    <div className='@container'>
      <div
        className={cn(
          "grid gap-x-4 overflow-x-auto border border-border/60 bg-card",
          SOURCE_KIND_COLUMNS
        )}
        data-testid='admin-airway-source-kind-list'
      >
        <div className='col-span-full grid grid-cols-subgrid border-border/60 border-b px-3 py-1.5 font-medium text-[10px] text-muted-foreground uppercase tracking-wider'>
          <span className='pl-[1.125rem]'>Source kind</span>
          <Th info="What airway admits for this kind's resources, on every run. Permissive admits all; Forbid opaque rejects a confirmed opaque vendor contract; Require declared also rejects undeclared ones. A stricter policy halts every pipeline whose resources fail it.">
            Contract policy
          </Th>
          <Th info='The environment airway admits this kind under. Airway refuses some connectors under Sandbox, so this is an admission setting too.'>
            Environment
          </Th>
          <Th
            align='right'
            info="Workspaces that run this kind under a policy of their own. A field an override leaves unset follows this kind's row."
          >
            Overrides
          </Th>
          <Th align='right' className={WIDE_ONLY}>
            Updated
          </Th>
          <span className='sr-only'>Save state</span>
        </div>
        <ul className='col-span-full grid grid-cols-subgrid'>
          {kinds.map((kind) => (
            <SourceKindRow key={kind.source_kind} kind={kind} />
          ))}
        </ul>
      </div>
    </div>
  );
}

function Th({
  children,
  info,
  align = "left",
  className
}: {
  children: ReactNode;
  info?: ReactNode;
  align?: "left" | "right";
  className?: string;
}) {
  return (
    <span
      className={cn(
        // `overflow-hidden`: a column squeezed to nothing takes its ⓘ with it rather
        // than drawing it over the next column.
        "flex min-w-0 items-center gap-1 overflow-hidden whitespace-nowrap",
        align === "right" && "justify-end",
        className
      )}
    >
      {/* Truncates in the narrow layout, where the policy and environment columns can
          shrink below their label; the ⓘ stays. */}
      <span className='truncate'>{children}</span>
      {info ? <AdminInfo className='normal-case tracking-normal'>{info}</AdminInfo> : null}
    </span>
  );
}
