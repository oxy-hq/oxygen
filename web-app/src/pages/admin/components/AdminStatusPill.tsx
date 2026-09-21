import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE, type AdminTone } from "./adminTone";

/**
 * Operator-console-grade status indicator. Renders a 2-color dot + a short
 * label. Mid-density. Designed to be legible at a glance across a long
 * table of rows.
 *
 * Variants intentionally collapse "ok / needs-attention / critical" into a
 * small enumeration so cross-resource lists (orgs, users, workspaces) read
 * consistently. New resource statuses should map onto one of these tones
 * rather than introduce new ones. The tones and their colours live in
 * `adminTone.ts`; this component only lays them out.
 */
export type AdminStatusTone = AdminTone;

const TONE = ADMIN_TONE;

export const AdminStatusPill = ({
  tone,
  label,
  className,
  "data-testid": dataTestId
}: {
  tone: AdminStatusTone;
  label: string;
  className?: string;
  "data-testid"?: string;
}) => {
  const v = TONE[tone];
  return (
    <span
      data-testid={dataTestId}
      className={cn(
        // The neutral surface is deliberate, and measured. Swapping it for the tone's own
        // tint (`v.bg`) looks more consistent and reads *worse*: a pill sits inside a
        // card, so its tint composites over `--card` rather than the page, landing
        // lighter — the dark danger pill went from 4.38:1 to 3.97:1. The same tint under
        // a finding row on `--background` reaches 4.6:1, which is why the two differ.
        //
        // 4.38:1 still misses 4.5:1 for small text. The real fix is a brighter
        // `--status-error` in dark, which is a design-system change beyond this surface;
        // recorded here so the next person does not re-run the experiment.
        "inline-flex items-center gap-1.5 rounded-full bg-muted/40 px-2 py-0.5 font-medium text-xs ring-1 ring-inset",
        v.text,
        v.ring,
        className
      )}
    >
      <span className={cn("inline-block size-1.5 rounded-full", v.dot)} aria-hidden />
      {label}
    </span>
  );
};
