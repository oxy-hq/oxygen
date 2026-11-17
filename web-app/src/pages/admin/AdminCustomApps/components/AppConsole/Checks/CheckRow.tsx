import { useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import { timeAgo } from "@/libs/utils/date";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { CustomApp } from "@/types/apps";
import { InvocationRow } from "../../AppDetail/components/Functions/InvocationRow";
import { environmentLabel } from "../../AppDetail/components/Secrets/environmentLabel";
import type { CheckCell, CheckRow as CheckRowModel } from "./checks";

/**
 * One check: its latest run per environment, side by side. A cell with a run
 * opens that invocation — its error, its waterfall, Copy for an agent — under
 * the row. Only a failure is coloured.
 */
export const CheckRow = ({ app, row }: { app: CustomApp; row: CheckRowModel }) => {
  const [open, setOpen] = useState<string | null>(null);
  const opened = row.cells.find(
    (c): c is Extract<CheckCell, { state: "read" }> => c.environment === open && c.state === "read"
  );
  return (
    <>
      <div
        className='col-span-full grid grid-cols-subgrid items-baseline border-border/40 border-b py-1.5 text-xs'
        data-testid={`admin-app-check-${row.name}`}
      >
        <span className='truncate font-medium font-mono' title={row.name}>
          {row.name}
        </span>
        {row.cells.map((cell) => (
          <Cell
            key={cell.environment}
            cell={cell}
            open={open === cell.environment}
            onToggle={() => setOpen((was) => (was === cell.environment ? null : cell.environment))}
          />
        ))}
      </div>
      {opened?.latest && (
        <div className='col-span-full border-border/40 border-b bg-muted/20 px-2 py-1'>
          <span className='text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
            {row.name} · latest in {environmentLabel(opened.environment).toLowerCase()}
          </span>
          <ul>
            <InvocationRow appId={app.id} iv={opened.latest} appRef={app} />
          </ul>
        </div>
      )}
    </>
  );
};

const Cell = ({
  cell,
  open,
  onToggle
}: {
  cell: CheckCell;
  open: boolean;
  onToggle: () => void;
}) => {
  const testid = `admin-app-check-cell-${cell.environment}`;
  if (cell.state === "loading") {
    return (
      <span className='text-muted-foreground' data-testid={testid}>
        …
      </span>
    );
  }
  if (cell.state === "unreadable") {
    return (
      <span className='text-muted-foreground' data-testid={testid}>
        {cell.forbidden ? "no access" : "couldn't read"}
      </span>
    );
  }
  if (!cell.latest) {
    return (
      <span className='text-muted-foreground' data-testid={testid} data-verdict='never'>
        never run
      </span>
    );
  }
  const iv = cell.latest;
  return (
    <button
      type='button'
      onClick={onToggle}
      aria-expanded={open}
      className='flex min-w-0 flex-wrap items-baseline gap-x-1.5 text-left hover:underline'
      data-testid={testid}
      data-verdict={cell.verdict}
    >
      <span
        className={cn(
          "font-medium",
          cell.verdict === "failed"
            ? ADMIN_TONE.danger.text
            : cell.verdict === "passed"
              ? "text-foreground"
              : "text-muted-foreground"
        )}
      >
        {cell.verdict === "other" ? iv.status : cell.verdict}
      </span>
      <span className='text-muted-foreground' title={new Date(iv.created_at).toLocaleString()}>
        {timeAgo(iv.created_at)}
      </span>
      {iv.build_id && (
        <span
          className='max-w-[8rem] truncate font-mono text-[10px] text-muted-foreground'
          title={`build ${iv.build_id}`}
        >
          {iv.build_id}
        </span>
      )}
    </button>
  );
};
