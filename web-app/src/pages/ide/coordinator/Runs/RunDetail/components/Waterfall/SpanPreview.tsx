import { Brain, ChevronsRight, Code2, Database, GitBranch, Wrench } from "lucide-react";
import type React from "react";
import { cn } from "@/libs/shadcn/utils";
import { toText } from "@/libs/utils/string";
import { formatTokens } from "../../../../components/utils";
import { type ChildSpan, colorsFor, formatMs, type PhaseSpan } from "./model";

/** Side-panel detail for the currently focused span. The parent supplies
 *  the card frame + sticky positioning; this renders only the content. */
export const SpanPreview: React.FC<{ span: ChildSpan | null }> = ({ span }) => {
  if (!span) {
    return (
      <div className='flex h-32 items-center justify-center px-4 text-muted-foreground text-xs'>
        Hover a bar to inspect.
      </div>
    );
  }

  const Icon =
    span.kind === "llm"
      ? Brain
      : span.kind === "tool"
        ? Wrench
        : span.kind === "subrun"
          ? GitBranch
          : span.kind === "query"
            ? Database
            : span.kind === "step"
              ? ChevronsRight
              : Code2;

  return (
    <div className='p-4'>
      <div className='mb-2 flex items-center gap-2'>
        <Icon className='h-4 w-4 text-primary' />
        <span className='truncate font-semibold text-sm'>{span.label}</span>
        <span className='ml-auto text-muted-foreground text-xs tabular-nums'>
          {formatMs(span.durationMs)}
        </span>
        {span.status === "error" && (
          <span className='rounded bg-destructive/15 px-1.5 py-0.5 text-destructive text-xs'>
            error
          </span>
        )}
      </div>

      {span.llm && (
        <div className='flex flex-wrap gap-x-3 gap-y-1 text-muted-foreground text-xs'>
          <span>
            <span className='font-medium text-foreground'>model</span> {span.llm.model || "—"}
          </span>
          <span>
            <span className='font-medium text-foreground'>in</span>{" "}
            {formatTokens(span.llm.promptTokens)}
          </span>
          <span>
            <span className='font-medium text-foreground'>out</span>{" "}
            {formatTokens(span.llm.outputTokens)}
          </span>
          {span.llm.cacheCreationTokens > 0 && (
            <span>
              <span className='font-medium text-foreground'>cache w</span>{" "}
              {formatTokens(span.llm.cacheCreationTokens)}
            </span>
          )}
          {span.llm.cacheReadTokens > 0 && (
            <span>
              <span className='font-medium text-foreground'>cache r</span>{" "}
              {formatTokens(span.llm.cacheReadTokens)}
            </span>
          )}
        </div>
      )}

      {span.tool && (
        <div className='mt-2 space-y-2'>
          <PreviewBlock label='input' value={span.tool.input} />
          {span.tool.error ? (
            <PreviewBlock label='error' value={span.tool.error} variant='error' />
          ) : (
            <PreviewBlock label='output' value={span.tool.output} />
          )}
        </div>
      )}

      {span.thinking && (
        <div className='space-y-1'>
          <p className='text-muted-foreground text-xs'>
            Extended-thinking block during{" "}
            <span className='font-medium text-foreground capitalize'>
              {span.thinking.state || "—"}
            </span>
            . Content is encrypted by the provider; only state and duration are observable.
          </p>
        </div>
      )}

      {span.subrun && <SubrunPreview subrun={span.subrun} />}

      {span.query && <QueryPreview query={span.query} />}

      {span.step && (
        <p className='text-muted-foreground text-xs'>
          Automation step <span className='font-medium text-foreground'>{span.step.name}</span>{" "}
          {span.step.success ? "completed" : "failed"}
          {span.step.error ? ` — ${span.step.error}` : ""}.
        </p>
      )}
    </div>
  );
};

const QueryPreview: React.FC<{ query: NonNullable<ChildSpan["query"]> }> = ({ query }) => (
  <div className='space-y-2'>
    <div className='flex flex-wrap gap-x-3 gap-y-1 text-muted-foreground text-xs'>
      <span>
        <span className='font-medium text-foreground'>source</span> {query.source}
        {query.isPreagg && (
          <span className='ml-1 rounded bg-emerald-500/15 px-1 py-0.5 text-emerald-700 text-xs'>
            preagg
          </span>
        )}
      </span>
      {query.success ? (
        <span className='text-emerald-600'>
          ✓ {query.rowCount.toLocaleString()} row{query.rowCount === 1 ? "" : "s"}
          {query.columns.length > 0 ? ` · ${query.columns.length} cols` : ""}
        </span>
      ) : (
        <span className='text-destructive'>✗ failed</span>
      )}
    </div>

    {query.sql && <PreviewBlock label='sql' value={query.sql} />}
    {query.error && <PreviewBlock label='error' value={query.error} variant='error' />}

    {query.success && query.rowsPreview.length > 0 && (
      <ResultPreview columns={query.columns} rows={query.rowsPreview} />
    )}
  </div>
);

const ResultPreview: React.FC<{ columns: string[]; rows: unknown[][] }> = ({ columns, rows }) => {
  const display = rows.slice(0, 8);
  return (
    <div>
      <p className='mb-0.5 text-muted-foreground text-xs uppercase tracking-wide'>result</p>
      <div className='overflow-x-auto rounded border border-border bg-card'>
        <table className='w-full text-xs'>
          <thead>
            <tr className='border-border border-b'>
              {columns.map((col) => (
                <th key={col} className='px-2 py-1 text-left font-medium text-muted-foreground'>
                  {col}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {display.map((row) => {
              const rowKey = JSON.stringify(row);
              return (
                <tr key={rowKey} className='border-border border-b last:border-0'>
                  {row.map((cell, j) => (
                    <td
                      key={columns[j] ?? `c${j}`}
                      className='whitespace-nowrap px-2 py-1 tabular-nums'
                    >
                      {cell === null || cell === undefined ? (
                        <span className='text-muted-foreground italic'>null</span>
                      ) : typeof cell === "object" ? (
                        JSON.stringify(cell)
                      ) : (
                        toText(cell)
                      )}
                    </td>
                  ))}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {rows.length > display.length && (
        <p className='mt-1 text-muted-foreground text-xs italic'>
          showing first {display.length} of {rows.length} preview rows
        </p>
      )}
    </div>
  );
};

const SubrunPreview: React.FC<{
  subrun: NonNullable<ChildSpan["subrun"]>;
}> = ({ subrun }) => (
  <div className='space-y-3'>
    <div className='flex flex-wrap gap-x-3 gap-y-1 text-muted-foreground text-xs'>
      <span>
        <span className='font-medium text-foreground'>target</span> {subrun.target}
      </span>
      <span>
        <span className='font-medium text-foreground'>phases</span> {subrun.nested.phases.length}
      </span>
      <span className={subrun.success ? "text-emerald-600" : "text-destructive"}>
        {subrun.success ? "✓ succeeded" : "✗ failed"}
      </span>
    </div>

    {subrun.request && <PreviewBlock label='request' value={subrun.request} />}
    {subrun.answer && <PreviewBlock label='answer' value={subrun.answer} />}
    {subrun.error && <PreviewBlock label='error' value={subrun.error} variant='error' />}

    {subrun.nested.phases.length > 0 && (
      <div>
        <p className='mb-1 text-muted-foreground text-xs uppercase tracking-wide'>
          nested timeline
        </p>
        <div className='space-y-0.5 rounded border border-border bg-muted/20 p-2'>
          {subrun.nested.phases.map((phase) => (
            <NestedPhaseRow
              key={`${phase.state}-${phase.index}`}
              phase={phase}
              totalMs={Math.max(subrun.nested.totalMs, 1)}
            />
          ))}
        </div>
      </div>
    )}
  </div>
);

const NestedPhaseRow: React.FC<{ phase: PhaseSpan; totalMs: number }> = ({ phase, totalMs }) => {
  const colors = colorsFor(phase.state);
  const leftPct = (phase.startMs / totalMs) * 100;
  const widthPct = Math.max((phase.durationMs / totalMs) * 100, 0.5);
  return (
    <div className='flex items-center gap-2'>
      <span className={cn("w-16 shrink-0 truncate text-xs capitalize", colors.text)}>
        {phase.state}
      </span>
      <div className='relative h-2 flex-1 rounded bg-muted/40'>
        <div
          className={cn("absolute inset-y-0 rounded", colors.bg)}
          style={{ left: `${leftPct}%`, width: `${widthPct}%` }}
        />
      </div>
      <span className='w-20 shrink-0 text-right text-muted-foreground text-xs tabular-nums'>
        {formatMs(phase.durationMs)}
      </span>
    </div>
  );
};

const PreviewBlock: React.FC<{
  label: string;
  value: unknown;
  variant?: "error";
}> = ({ label, value, variant }) => {
  const rendered =
    typeof value === "string"
      ? value
      : value === null || value === undefined
        ? "—"
        : JSON.stringify(value, null, 2);
  return (
    <div>
      <p className='mb-0.5 text-muted-foreground text-xs uppercase tracking-wide'>{label}</p>
      <pre
        className={`max-h-64 overflow-y-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-2 font-mono text-xs ${variant === "error" ? "text-destructive" : ""}`}
      >
        {rendered}
      </pre>
    </div>
  );
};
