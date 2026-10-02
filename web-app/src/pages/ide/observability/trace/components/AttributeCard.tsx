import { Check, Copy, Maximize2 } from "lucide-react";
import { useState } from "react";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import { Button } from "@/components/ui/shadcn/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/shadcn/dialog";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import usePrismTheme from "@/hooks/usePrismTheme";
import { deepParseJson } from "./utils";

interface AttributeCardProps {
  name: string;
  value: string;
}

export function AttributeCard({ name, value }: AttributeCardProps) {
  const [copied, setCopied] = useState(false);
  const [showRaw, setShowRaw] = useState(false);
  const [isExpanded, setIsExpanded] = useState(false);
  const prismTheme = usePrismTheme();

  const handleCopy = () => {
    // Show the check only once the write lands: the browser rejects it when
    // clipboard access is denied or the page is not focused.
    navigator.clipboard
      .writeText(value)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      })
      .catch((err) => console.error("Failed to copy attribute value:", err));
  };

  // Try to parse JSON for pretty display, resolving nested JSON strings
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

  const displayValue = showRaw || !isJson ? value : parsedValue!;

  return (
    <>
      <div className='overflow-hidden rounded-lg border'>
        {/* Header */}
        <div className='flex items-center justify-between border-b bg-card px-3 py-2'>
          <span className='font-semibold text-xs'>{name}</span>
          <div className='flex items-center gap-1'>
            {isJson && (
              <ToggleGroup
                type='single'
                value={showRaw ? "raw" : "json"}
                onValueChange={(val) => val && setShowRaw(val === "raw")}
                size='sm'
                className='h-7'
              >
                <ToggleGroupItem value='json' className='h-6 px-2 text-xs'>
                  JSON
                </ToggleGroupItem>
                <ToggleGroupItem value='raw' className='h-6 px-2 text-xs'>
                  Raw
                </ToggleGroupItem>
              </ToggleGroup>
            )}
            <Button
              variant='ghost'
              size='icon'
              className='h-7 w-7'
              onClick={handleCopy}
              title='Copy'
            >
              {copied ? (
                <Check className='h-3.5 w-3.5 text-success' />
              ) : (
                <Copy className='h-3.5 w-3.5' />
              )}
            </Button>
            <Button
              variant='ghost'
              size='icon'
              className='h-7 w-7'
              onClick={() => setIsExpanded(true)}
              title='Expand'
            >
              <Maximize2 className='h-3.5 w-3.5' />
            </Button>
          </div>
        </div>
        {/* Content */}
        <div className='scrollbar-gutter-auto max-h-80 overflow-auto'>
          {!showRaw && isJson ? (
            <SyntaxHighlighter
              language='json'
              style={prismTheme}
              className='m-0! bg-card! p-4! font-mono text-sm [&>code]:bg-transparent!'
              showLineNumbers
            >
              {displayValue}
            </SyntaxHighlighter>
          ) : (
            <pre className='whitespace-pre-wrap break-all bg-card! p-3 text-sm'>{displayValue}</pre>
          )}
        </div>
      </div>

      {/* Expanded Dialog */}
      <Dialog open={isExpanded} onOpenChange={setIsExpanded}>
        <DialogContent
          showCloseButton={false}
          className='flex max-h-[90vh] max-w-4xl flex-col'
          onPointerDownOutside={() => setIsExpanded(false)}
        >
          <DialogHeader className='flex-shrink-0'>
            <DialogTitle className='flex items-center justify-between'>
              <span>{name}</span>
              <div className='flex items-center gap-2'>
                {isJson && (
                  <ToggleGroup
                    type='single'
                    value={showRaw ? "raw" : "json"}
                    onValueChange={(val) => val && setShowRaw(val === "raw")}
                    size='sm'
                  >
                    <ToggleGroupItem value='json' className='px-2 text-xs'>
                      JSON
                    </ToggleGroupItem>
                    <ToggleGroupItem value='raw' className='px-2 text-xs'>
                      Raw
                    </ToggleGroupItem>
                  </ToggleGroup>
                )}
                <Button variant='ghost' size='icon' className='h-8 w-8' onClick={handleCopy}>
                  {copied ? (
                    <Check className='h-4 w-4 text-success' />
                  ) : (
                    <Copy className='h-4 w-4' />
                  )}
                </Button>
              </div>
            </DialogTitle>
          </DialogHeader>
          <div className='scrollbar-gutter-auto max-h-80 overflow-auto'>
            {!showRaw && isJson ? (
              <SyntaxHighlighter
                language='json'
                style={prismTheme}
                className='m-0! bg-card! p-4! font-mono text-sm [&>code]:bg-transparent!'
                showLineNumbers
              >
                {displayValue}
              </SyntaxHighlighter>
            ) : (
              <pre className='whitespace-pre-wrap break-all bg-card! p-3 text-sm'>
                {displayValue}
              </pre>
            )}
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
