import { Check, Copy, Loader2, Play } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import { oneDark } from "react-syntax-highlighter/dist/esm/styles/prism";
import { Button } from "@/components/ui/shadcn/button";
import type { NodeColumnDef, NodeSummary } from "@/types/modeling";
import LineageGraph from "./LineageGraph";

type Tab = "raw" | "compiled" | "columns" | "lineage";

interface NodeDetailProps {
  node: NodeSummary;
  dbtProjectName: string;
  onRunStream: (selector?: string) => Promise<void>;
  isStreaming: boolean;
}

const ColumnsTable: React.FC<{ columns: NodeColumnDef[] }> = ({ columns }) => {
  if (columns.length === 0) {
    return (
      <p className='px-4 py-3 text-muted-foreground text-sm'>
        No column definitions found in schema.yml.
      </p>
    );
  }
  return (
    <div className='overflow-auto'>
      <table className='w-full text-sm'>
        <thead>
          <tr className='border-b bg-muted/40'>
            <th className='px-4 py-2 text-left font-medium text-muted-foreground text-xs uppercase tracking-wider'>
              Column
            </th>
            <th className='px-4 py-2 text-left font-medium text-muted-foreground text-xs uppercase tracking-wider'>
              Type
            </th>
            <th className='px-4 py-2 text-left font-medium text-muted-foreground text-xs uppercase tracking-wider'>
              Description
            </th>
          </tr>
        </thead>
        <tbody>
          {columns.map((col) => (
            <tr key={col.name} className='border-b hover:bg-muted/20'>
              <td className='px-4 py-2 font-mono text-xs'>{col.name}</td>
              <td className='px-4 py-2 text-muted-foreground text-xs'>
                {col.data_type ?? <span className='italic opacity-50'>—</span>}
              </td>
              <td className='px-4 py-2 text-muted-foreground text-xs'>
                {col.description ?? <span className='italic opacity-50'>—</span>}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
};

const NodeDetail: React.FC<NodeDetailProps> = ({
  node,
  dbtProjectName,
  onRunStream,
  isStreaming
}) => {
  const hasSql = !!node.raw_sql;
  const hasColumns = node.columns.length > 0;
  const defaultTab: Tab = hasSql ? "raw" : hasColumns ? "columns" : "raw";
  const [tab, setTab] = useState<Tab>(defaultTab);
  const [copyState, setCopyState] = useState<"idle" | "copied">("idle");

  const sql = (tab === "compiled" ? node.compiled_sql : node.raw_sql)?.trim();

  return (
    <div className='flex h-full flex-col'>
      <div className='flex items-center gap-3 border-b px-4 py-2'>
        <h2 className='font-medium'>{node.name}</h2>
        <span className='rounded bg-muted px-1.5 py-0.5 text-muted-foreground text-xs'>
          {node.resource_type}
        </span>
        {node.materialization && (
          <span className='rounded bg-muted px-1.5 py-0.5 text-muted-foreground text-xs'>
            {node.materialization}
          </span>
        )}
        <div className='ml-auto'>
          <Button
            size='sm'
            variant='outline'
            onClick={() => onRunStream(`+${node.name}`)}
            disabled={isStreaming}
          >
            {isStreaming ? (
              <Loader2 className='mr-1.5 h-3.5 w-3.5 animate-spin' />
            ) : (
              <Play className='mr-1.5 h-3.5 w-3.5' />
            )}
            Run
          </Button>
        </div>
      </div>

      {node.description && (
        <p className='border-b px-4 py-2 text-muted-foreground text-sm'>{node.description}</p>
      )}

      <div className='flex gap-0 border-b'>
        {hasSql && (
          <button
            type='button'
            onClick={() => setTab("raw")}
            className={`px-4 py-1.5 text-sm ${tab === "raw" ? "border-primary border-b-2 font-medium" : "text-muted-foreground"}`}
          >
            Raw SQL
          </button>
        )}
        {hasSql && node.compiled_sql && (
          <button
            type='button'
            onClick={() => setTab("compiled")}
            className={`px-4 py-1.5 text-sm ${tab === "compiled" ? "border-primary border-b-2 font-medium" : "text-muted-foreground"}`}
          >
            Compiled SQL
          </button>
        )}
        {hasColumns && (
          <button
            type='button'
            onClick={() => setTab("columns")}
            className={`px-4 py-1.5 text-sm ${tab === "columns" ? "border-primary border-b-2 font-medium" : "text-muted-foreground"}`}
          >
            Columns
            {node.columns.length > 0 && (
              <span className='ml-1.5 rounded bg-muted px-1 py-0.5 text-xs'>
                {node.columns.length}
              </span>
            )}
          </button>
        )}
        <button
          type='button'
          onClick={() => setTab("lineage")}
          className={`px-4 py-1.5 text-sm ${tab === "lineage" ? "border-primary border-b-2 font-medium" : "text-muted-foreground"}`}
        >
          Lineage
        </button>
      </div>

      {tab === "lineage" ? (
        <div className='flex-1'>
          <LineageGraph nodeId={node.unique_id} dbtProjectName={dbtProjectName} />
        </div>
      ) : tab === "columns" ? (
        <div className='flex-1 overflow-auto'>
          <ColumnsTable columns={node.columns} />
        </div>
      ) : (
        <div className='[&_pre]:!m-0 [&_pre]:!rounded-none relative flex-1 overflow-auto text-xs [&_pre]:h-full'>
          {sql ? (
            <>
              <Button
                variant='ghost'
                size='icon'
                className='absolute top-2 right-2 z-10 h-6 w-6 bg-muted/50 opacity-60 hover:opacity-100'
                onClick={() => {
                  // "Copied!" only once the write lands: the browser rejects it when
                  // clipboard access is denied or the page is not focused.
                  navigator.clipboard
                    .writeText(sql)
                    .then(() => {
                      setCopyState("copied");
                      setTimeout(() => setCopyState("idle"), 1500);
                    })
                    .catch((err) => console.error("Failed to copy SQL:", err));
                }}
                tooltip={{
                  content: copyState === "copied" ? "Copied!" : "Copy SQL",
                  side: "left"
                }}
              >
                {copyState === "copied" ? (
                  <Check className='h-3.5 w-3.5 text-emerald-500' />
                ) : (
                  <Copy className='h-3.5 w-3.5' />
                )}
              </Button>
              <SyntaxHighlighter
                language='sql'
                style={oneDark}
                wrapLines={true}
                customStyle={{ margin: 0, borderRadius: 0, height: "100%" }}
                lineProps={{ style: { wordBreak: "break-all", whiteSpace: "pre-wrap" } }}
                className='font-mono text-xs'
              >
                {sql}
              </SyntaxHighlighter>
            </>
          ) : (
            <p className='p-4 text-muted-foreground text-xs'>
              Not compiled yet. Click Compile to generate compiled SQL.
            </p>
          )}
        </div>
      )}

      {node.depends_on.length > 0 && (
        <div className='border-t px-4 py-3'>
          <p className='mb-1.5 font-medium text-muted-foreground text-xs uppercase tracking-wider'>
            Depends on
          </p>
          <div className='flex flex-wrap gap-1.5'>
            {node.depends_on.map((dep) => (
              <span key={dep} className='rounded bg-muted px-2 py-0.5 font-mono text-xs'>
                {dep}
              </span>
            ))}
          </div>
        </div>
      )}

      {node.tags.length > 0 && (
        <div className='border-t px-4 py-3'>
          <p className='mb-1.5 font-medium text-muted-foreground text-xs uppercase tracking-wider'>
            Tags
          </p>
          <div className='flex flex-wrap gap-1.5'>
            {node.tags.map((tag) => (
              <span key={tag} className='rounded bg-primary/10 px-2 py-0.5 text-primary text-xs'>
                {tag}
              </span>
            ))}
          </div>
        </div>
      )}
    </div>
  );
};

export default NodeDetail;
