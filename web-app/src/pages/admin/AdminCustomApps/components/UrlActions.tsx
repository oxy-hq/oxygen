import { Check, Copy } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { cn } from "@/libs/shadcn/utils";

/**
 * URL affordances — copy.
 *
 * Moved up out of `AppsTable/components/` when the registry table was deleted. It was
 * never table-specific: `AppInfo` imported it across three directory levels, which is
 * what made it survive its own folder. It sits beside the other shared app components
 * now, where a second consumer is not a reach.
 */

async function writeClipboard(value: string, label: string) {
  try {
    await navigator.clipboard.writeText(value);
    toast.success(`Copied ${label}`);
    return true;
  } catch {
    toast.error("Couldn't copy to clipboard");
    return false;
  }
}

/** Small icon button that copies `value` and flips to a check for a beat. */
export const CopyButton = ({
  value,
  label = "URL",
  className
}: {
  value: string;
  label?: string;
  className?: string;
}) => {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type='button'
      aria-label={`Copy ${label}`}
      title={`Copy ${label}`}
      className={cn(
        "inline-flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        className
      )}
      onClick={async (e) => {
        e.stopPropagation();
        if (await writeClipboard(value, label)) {
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        }
      }}
    >
      {copied ? <Check className='size-3.5 text-primary' /> : <Copy className='size-3.5' />}
    </button>
  );
};
