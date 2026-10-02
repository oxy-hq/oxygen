import { Check, Copy } from "lucide-react";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import { useCopyTimeout } from "@/components/automation/output/useCopyTimeout";
import { Button } from "@/components/ui/shadcn/button";
import usePrismTheme from "@/hooks/usePrismTheme";
import { deepParseJson } from "../../../trace/components/utils";

interface OutputDisplayProps {
  value: string;
  label: string;
}

export default function DataDisplay({ value, label }: OutputDisplayProps) {
  const { copied, handleCopy } = useCopyTimeout();
  const prismTheme = usePrismTheme();

  // `handleCopy` catches a refused clipboard write itself and only flips to the check
  // on success.
  const copyToClipboard = () => void handleCopy(value);

  let parsedValue: string | null = null;
  let isJson = false;
  try {
    const parsed = JSON.parse(value);
    const deepParsed = deepParseJson(parsed);
    parsedValue = JSON.stringify(deepParsed, null, 2);
    isJson = true;
  } catch {
    // Not JSON, use as-is
  }

  const displayValue = !isJson ? value : parsedValue!;

  return (
    <div className='space-y-2'>
      <div className='flex items-center justify-between'>
        <span className='font-medium text-muted-foreground text-xs'>{label}</span>
        <Button variant='ghost' size='sm' className='h-6 px-2' onClick={copyToClipboard}>
          {copied ? <Check className='h-3 w-3' /> : <Copy className='h-3 w-3' />}
        </Button>
      </div>
      {isJson ? (
        <SyntaxHighlighter
          language='json'
          style={prismTheme}
          customStyle={{
            margin: 0,
            borderRadius: "0.5rem",
            fontSize: "0.75rem"
          }}
          className='rounded-lg border bg-muted/30! font-mono text-xs [&>code]:bg-transparent!'
          wrapLines
          lineProps={{
            style: { wordBreak: "break-all", whiteSpace: "pre-wrap" }
          }}
        >
          {displayValue}
        </SyntaxHighlighter>
      ) : (
        <pre className='max-h-48 overflow-x-auto overflow-y-auto whitespace-pre-wrap rounded-lg border bg-muted/30 p-2 text-xs'>
          {displayValue}
        </pre>
      )}
    </div>
  );
}
