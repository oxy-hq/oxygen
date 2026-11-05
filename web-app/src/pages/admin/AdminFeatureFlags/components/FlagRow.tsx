import { ChevronRight } from "lucide-react";
import { useId, useState } from "react";
import { Switch } from "@/components/ui/shadcn/switch";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { cn } from "@/libs/shadcn/utils";
import type { FeatureFlag } from "@/services/api/featureFlags";
import { ADMIN_TONE } from "../../components/adminTone";
import { relativeTime } from "../../utils";
import { servingSummary } from "../rules";

/** The facts behind a flag, shown only when its row is open. */
function FlagDetails({ flag, id }: { flag: FeatureFlag; id: string }) {
  const facts: [string, string][] = [
    ["Variants", flag.variants.join(" · ")],
    ["Default", flag.default ? "on" : "off"],
    ["Split by", flag.unit],
    ["Rules", flag.per_org ? "per org" : "global only: read without an org"],
    ["Web app", flag.client ? "reads it" : "does not read it"]
  ];
  return (
    <div
      id={id}
      className='mt-2 space-y-2 pl-4.5'
      data-testid={`admin-feature-flags-details-${flag.key}`}
    >
      <p className='max-w-prose whitespace-pre-wrap break-words text-muted-foreground text-xs'>
        {flag.description}
      </p>
      <dl className='flex flex-wrap gap-x-5 gap-y-1 text-xs'>
        {facts.map(([term, value]) => (
          <div key={term} className='flex gap-1.5'>
            <dt className='text-muted-foreground'>{term}</dt>
            <dd className='font-mono'>{value}</dd>
          </div>
        ))}
      </dl>
      {flag.targeting_error && (
        <p
          className={cn("break-words font-mono text-xs", ADMIN_TONE.danger.text)}
          data-testid={`admin-feature-flags-error-${flag.key}`}
        >
          {flag.targeting_error}
        </p>
      )}
    </div>
  );
}

/**
 * One flag: its key and a one-line description that opens to the whole text and the
 * flag's facts, what it serves now, when it last changed, and its kill switch.
 */
export function FlagRow({
  flag,
  switchDisabled,
  onToggle,
  onEditRules
}: {
  flag: FeatureFlag;
  switchDisabled: boolean;
  onToggle: (next: boolean) => void;
  onEditRules: () => void;
}) {
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const serving = servingSummary(flag);

  return (
    <TableRow
      className='border-border/60 align-top hover:bg-muted/30'
      data-testid={`admin-feature-flags-row-${flag.key}`}
    >
      {/* `max-w-0` lets the description truncate instead of widening the table. */}
      <TableCell className='w-full max-w-0 py-2.5'>
        <button
          type='button'
          aria-expanded={open}
          aria-controls={open ? detailsId : undefined}
          onClick={() => setOpen(!open)}
          className='group flex w-full min-w-0 items-start gap-1.5 rounded-sm text-left outline-none focus-visible:ring-2 focus-visible:ring-ring'
          data-testid={`admin-feature-flags-expand-${flag.key}`}
        >
          <ChevronRight
            aria-hidden
            className={cn(
              "mt-0.5 size-3 shrink-0 text-muted-foreground transition-transform duration-(--duration-fast) group-hover:text-foreground motion-reduce:transition-none",
              open && "rotate-90"
            )}
          />
          <span className='min-w-0 flex-1'>
            <span className='block truncate font-medium font-mono text-xs'>{flag.key}</span>
            {!open && (
              <span className='mt-0.5 block truncate text-muted-foreground text-xs'>
                {flag.description}
              </span>
            )}
          </span>
        </button>
        {open && <FlagDetails flag={flag} id={detailsId} />}
      </TableCell>

      <TableCell className='w-36 py-2.5 sm:w-56'>
        <div
          className={cn("font-medium text-xs", serving.broken && ADMIN_TONE.danger.text)}
          data-testid={`admin-feature-flags-summary-${flag.key}`}
        >
          {serving.lead}
        </div>
        {serving.detail && (
          <div className='mt-0.5 truncate text-muted-foreground text-xs'>{serving.detail}</div>
        )}
        {flag.per_org && (
          <button
            type='button'
            className='mt-1 rounded-sm text-xs underline decoration-muted-foreground/50 underline-offset-2 outline-none transition-colors hover:decoration-foreground focus-visible:ring-2 focus-visible:ring-ring'
            onClick={onEditRules}
            data-testid={`admin-feature-flags-rules-${flag.key}`}
          >
            Edit rules
          </button>
        )}
      </TableCell>

      <TableCell
        className='hidden whitespace-nowrap py-2.5 text-muted-foreground text-xs tabular-nums sm:table-cell'
        title={flag.updated_at ? new Date(flag.updated_at).toLocaleString() : undefined}
      >
        {flag.updated_at ? relativeTime(flag.updated_at) : "never"}
      </TableCell>

      <TableCell className='py-2 text-right'>
        <Switch
          checked={flag.enabled}
          onCheckedChange={onToggle}
          disabled={switchDisabled}
          aria-label={`${flag.key} kill switch`}
          data-testid={`admin-feature-flags-switch-${flag.key}`}
        />
      </TableCell>
    </TableRow>
  );
}
