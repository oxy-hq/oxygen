import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import usePrismTheme from "@/hooks/usePrismTheme";
import type { TimelineSpan } from "@/services/api/traces";
import { KeyValueGrid, type KeyValueRow } from "./InspectorKeyValue";
import { extractSpanSql } from "./spanInspect";

interface InspectorSqlProps {
  span: TimelineSpan;
}

/** The compiled query a span ran, syntax-highlighted, plus DB execution facts. */
export function InspectorSql({ span }: InspectorSqlProps) {
  const prismTheme = usePrismTheme();
  const sql = extractSpanSql(span);

  if (!sql) {
    return (
      <p className='text-muted-foreground text-xs'>
        This span ran no SQL. The compiled query is carried by the SQL execution span.
      </p>
    );
  }

  const facts: KeyValueRow[] = [];
  const engine = span.attributes["db.system"];
  const rows = span.attributes["db.rows"];
  const bytes = span.attributes["db.bytes_read"];
  const statement = span.attributes["db.statement_id"];
  if (engine) facts.push({ label: "engine", value: engine });
  if (rows) facts.push({ label: "rows", value: rows });
  if (bytes) facts.push({ label: "bytes read", value: bytes });
  if (statement) facts.push({ label: "statement", value: statement });

  return (
    <div className='space-y-3'>
      <SyntaxHighlighter
        language='sql'
        style={prismTheme}
        customStyle={{ margin: 0, borderRadius: "0.5rem", fontSize: "0.75rem" }}
        className='border bg-muted/30! font-mono text-xs [&>code]:bg-transparent!'
        wrapLines
        lineProps={{ style: { wordBreak: "break-all", whiteSpace: "pre-wrap" } }}
      >
        {sql}
      </SyntaxHighlighter>
      {facts.length > 0 && <KeyValueGrid rows={facts} />}
    </div>
  );
}
