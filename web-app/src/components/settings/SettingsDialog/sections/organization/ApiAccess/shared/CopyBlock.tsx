import { Check, Copy } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { cn } from "@/libs/shadcn/utils";

/** How long the button reads "Copied" before it offers to copy again. */
const COPIED_MS = 2000;

/** Copies `text` and confirms on the button itself, so the eye doesn't leave it. */
export function CopyButton({
  text,
  label,
  testId,
  className
}: {
  text: string;
  /** What is being copied, for the accessible name: "Copy token". */
  label: string;
  testId: string;
  className?: string;
}) {
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), COPIED_MS);
    return () => clearTimeout(timer);
  }, [copied]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch (error) {
      console.error("Failed to copy to clipboard:", error);
      toast.error("Couldn't copy. Select the text and copy it by hand.");
    }
  };

  return (
    <Button
      type='button'
      variant='outline'
      size='sm'
      className={cn("h-7 gap-1 px-2 text-xs", className)}
      onClick={copy}
      aria-label={label}
      data-testid={testId}
    >
      {copied ? <Check className='size-3' aria-hidden /> : <Copy className='size-3' aria-hidden />}
      {copied ? "Copied" : "Copy"}
    </Button>
  );
}

interface CopyBlockProps {
  /** What the copy button puts on the clipboard. */
  text: string;
  /** What is shown, when it differs from what is copied (a masked secret). */
  display?: string;
  label: string;
  testId: string;
  /** Keep long single lines on one line and scroll, instead of wrapping. */
  nowrap?: boolean;
}

/**
 * Text someone will paste somewhere else: a secret, a shell line, a workflow.
 * Monospace because every character matters, and selectable in one click.
 */
export function CopyBlock({ text, display, label, testId, nowrap = false }: CopyBlockProps) {
  return (
    <div className='relative rounded-md border bg-muted/40'>
      <pre
        className={cn(
          "overflow-x-auto p-3 pr-20 font-mono text-xs leading-relaxed",
          nowrap ? "whitespace-pre" : "whitespace-pre-wrap break-all"
        )}
        data-testid={testId}
      >
        <code className='select-all'>{display ?? text}</code>
      </pre>
      <CopyButton
        text={text}
        label={label}
        testId={`${testId}-copy`}
        className='absolute top-2 right-2 bg-background'
      />
    </div>
  );
}
