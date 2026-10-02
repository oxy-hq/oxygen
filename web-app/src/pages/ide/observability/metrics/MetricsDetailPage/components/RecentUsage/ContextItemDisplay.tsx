import { Check, Copy } from "lucide-react";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import { useCopyTimeout } from "@/components/automation/output/useCopyTimeout";
import { Button } from "@/components/ui/shadcn/button";
import usePrismTheme from "@/hooks/usePrismTheme";
import { cn } from "@/libs/shadcn/utils";
import type { ContextItem, SemanticContent } from "@/services/api/metrics";
import { CONTEXT_TYPE_CONFIG } from "../../constants";
import HighlightedText from "./HighlightedText";

interface ContextItemDisplayProps {
  item: ContextItem;
  metricName: string;
}

export default function ContextItemDisplay({ item, metricName }: ContextItemDisplayProps) {
  const { copied, handleCopy } = useCopyTimeout();
  const prismTheme = usePrismTheme();
  const config = CONTEXT_TYPE_CONFIG[item.type] || CONTEXT_TYPE_CONFIG.question;
  const isSQL = item.type === "sql" || item.type === "SQL";
  const isSemantic = item.type === "semantic";
  const content = typeof item.content === "string" ? item.content : JSON.stringify(item.content);

  // `handleCopy` catches a refused clipboard write itself and only flips to the check
  // on success.
  const copyToClipboard = () => void handleCopy(content);

  if (isSemantic) {
    const semanticContent = Array.isArray(item.content)
      ? (item.content as SemanticContent[])
      : [item.content as SemanticContent];

    return (
      <div className='space-y-2'>
        <div className='flex items-center justify-between'>
          <p className={cn("flex items-center gap-1 font-medium text-xs", config.color)}>
            {config.icon} {config.label}
          </p>
          <Button variant='ghost' size='sm' className='h-6 px-2' onClick={copyToClipboard}>
            {copied ? <Check className='h-3 w-3' /> : <Copy className='h-3 w-3' />}
          </Button>
        </div>
        <div className='space-y-2 rounded-lg border border-vis-orange/20 bg-gradient-to-br from-vis-orange/5 to-warning/5 p-3'>
          {semanticContent.map((semantic, idx) => (
            <div key={idx} className='space-y-1'>
              {semantic.topic && (
                <div className='flex items-center gap-2'>
                  <span className='text-muted-foreground text-xs'>Topic:</span>
                  <span className='font-mono text-vis-orange text-xs'>{semantic.topic}</span>
                </div>
              )}
              {semantic.measures && semantic.measures.length > 0 && (
                <div className='flex flex-wrap items-center gap-2'>
                  <span className='text-muted-foreground text-xs'>Measures:</span>
                  {semantic.measures.map((m, i) => (
                    <span
                      key={i}
                      className={cn(
                        "rounded px-1.5 py-0.5 font-mono text-xs",
                        m.includes(metricName)
                          ? "border border-highlight/30 bg-highlight/20 text-highlight"
                          : "bg-info/10 text-info"
                      )}
                    >
                      {m}
                    </span>
                  ))}
                </div>
              )}
              {semantic.dimensions && semantic.dimensions.length > 0 && (
                <div className='flex flex-wrap items-center gap-2'>
                  <span className='text-muted-foreground text-xs'>Dimensions:</span>
                  {semantic.dimensions.map((d, i) => (
                    <span
                      key={i}
                      className={cn(
                        "rounded px-1.5 py-0.5 font-mono text-xs",
                        d === metricName
                          ? "border border-highlight/30 bg-highlight/20 text-highlight"
                          : "bg-success/10 text-success"
                      )}
                    >
                      {d}
                    </span>
                  ))}
                </div>
              )}
            </div>
          ))}
        </div>
      </div>
    );
  }

  return (
    <div className='space-y-1'>
      <div className='flex items-center justify-between'>
        <p className={cn("flex items-center gap-1 font-medium text-xs", config.color)}>
          {config.icon} {config.label}
        </p>
        <Button variant='ghost' size='sm' className='h-6 px-2' onClick={copyToClipboard}>
          {copied ? <Check className='h-3 w-3' /> : <Copy className='h-3 w-3' />}
        </Button>
      </div>
      {isSQL ? (
        <SyntaxHighlighter
          language='sql'
          style={prismTheme}
          customStyle={{
            margin: 0,
            borderRadius: "0.5rem",
            fontSize: "0.75rem"
          }}
          wrapLines
          className='rounded-lg border bg-muted/30! font-mono text-xs [&>code]:bg-transparent!'
          lineProps={{
            style: { wordBreak: "break-all", whiteSpace: "pre-wrap" }
          }}
        >
          {content}
        </SyntaxHighlighter>
      ) : (
        <div className='rounded-lg border bg-muted/30 p-2 text-xs'>
          <HighlightedText text={content} highlight={metricName} />
        </div>
      )}
    </div>
  );
}
