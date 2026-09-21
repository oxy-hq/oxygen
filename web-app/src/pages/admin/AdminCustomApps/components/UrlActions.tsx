import { Check, Copy, ExternalLink, Link2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { cn } from "@/libs/shadcn/utils";
import { resolveBundleUrl } from "../resolveBundleUrl";

/**
 * URL affordances — copy, open, the pretty domain label.
 *
 * Moved up out of `AppsTable/components/` when the registry table was deleted. It was
 * never table-specific: `AppInfo` imported it across three directory levels, which is
 * what made it survive its own folder. It sits beside the other shared app components
 * now, where a second consumer is not a reach.
 */

/** Strip scheme + trailing slash for a compact, Vercel-style domain label. */
export const prettyUrl = (absolute: string): string =>
  absolute.replace(/^https?:\/\//, "").replace(/\/$/, "");

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

/** Open the resolved bundle URL in a new tab. */
export const OpenAppButton = ({ url, className }: { url: string; className?: string }) => (
  <button
    type='button'
    aria-label='Open app in new tab'
    title='Open app in new tab'
    className={cn(
      "inline-flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
      className
    )}
    onClick={(e) => {
      e.stopPropagation();
      window.open(resolveBundleUrl(url), "_blank", "noopener");
    }}
  >
    <ExternalLink className='size-3.5' />
  </button>
);

/**
 * A domain line: link glyph, the compact domain as a real anchor that opens
 * the app in a new tab, and a copy button that confirms with a check. `href`
 * must already be absolute+correct — the caller resolves the canonical path
 * via `resolveBundleUrl`, but passes a subdomain URL verbatim (rewriting its
 * host would break it). This is the copy/open affordance the list was missing,
 * surfaced rather than buried.
 */
export const UrlLine = ({
  href,
  label,
  copyLabel = "URL",
  className
}: {
  href: string;
  label?: string;
  copyLabel?: string;
  className?: string;
}) => (
  <div className={cn("flex min-w-0 items-center gap-1.5", className)}>
    <Link2 className='size-3.5 shrink-0 text-muted-foreground/70' />
    <a
      href={href}
      target='_blank'
      rel='noopener noreferrer'
      title={href}
      onClick={(e) => e.stopPropagation()}
      className='min-w-0 flex-1 truncate font-mono text-muted-foreground text-xs hover:text-foreground hover:underline focus-visible:underline focus-visible:outline-none'
    >
      {label && <span className='text-muted-foreground/50'>{label} </span>}
      {prettyUrl(href)}
    </a>
    <CopyButton value={href} label={copyLabel} />
  </div>
);
