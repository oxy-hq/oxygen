import { useMemo } from "react";
import { cn } from "@/libs/shadcn/utils";
import { lineDiff } from "./lineDiff";

interface CodeDiffViewProps {
  original: string;
  modified: string;
  /** The first line's number, for a snippet of a larger file. */
  startLine?: number;
  className?: string;
}

const MARK = { same: " ", removed: "-", added: "+" } as const;

/** A read-only inline diff: removed lines, then added ones, between the lines both keep. */
export default function CodeDiffView({
  original,
  modified,
  startLine = 1,
  className
}: CodeDiffViewProps) {
  const lines = useMemo(
    () => lineDiff(original, modified, startLine),
    [original, modified, startLine]
  );
  return (
    <div
      className={cn("h-full overflow-auto bg-editor-background py-2 font-mono text-xs", className)}
      data-testid='code-diff-view'
    >
      {lines.map((line, i) => (
        <div
          // oxlint-disable-next-line react/no-array-index-key -- lines have no identity but their order
          key={i}
          data-kind={line.kind}
          className={cn(
            "flex whitespace-pre-wrap break-all",
            line.kind === "removed" && "bg-destructive/10",
            line.kind === "added" && "bg-success/10"
          )}
        >
          <span className='w-10 shrink-0 select-none pr-2 text-right text-muted-foreground'>
            {line.kind === "removed" ? line.oldNo : line.kind === "added" ? line.newNo : line.newNo}
          </span>
          <span className='w-4 shrink-0 select-none text-muted-foreground'>{MARK[line.kind]}</span>
          <span className='min-w-0 flex-1 pr-3'>{line.text || " "}</span>
        </div>
      ))}
    </div>
  );
}
