import { ArrowRight } from "lucide-react";
import type { ComponentType } from "react";
import { cn } from "@/libs/shadcn/utils";
import { AdminSectionLabel } from "../../../components/AdminSectionLabel";

export type OrgAlert = {
  icon: ComponentType<{ className?: string }>;
  /** Pre-formatted, grammar-correct text (callers bake in any count). */
  text: string;
  severity?: "warn" | "danger";
  /** Jumps to the tab where the operator can act on this signal. */
  onSelect?: () => void;
};

/**
 * Org-scoped triage feed for the overview tab. Mirrors the cross-tenant
 * "Needs attention" feed on the tenants overview, but every row jumps to a
 * tab *within this org's 360* instead of a top-level list — keeping the
 * operator inside the tenant they're investigating. Renders nothing when the
 * org is healthy, so a clean tenant shows no chrome.
 */
export const NeedsAttention = ({ alerts }: { alerts: OrgAlert[] }) => {
  if (alerts.length === 0) return null;
  return (
    <section className='space-y-3'>
      <AdminSectionLabel trailing={`${alerts.length} signal${alerts.length === 1 ? "" : "s"}`}>
        Needs attention
      </AdminSectionLabel>
      <ul className='divide-y divide-border/60 overflow-hidden border border-border/60 bg-card'>
        {alerts.map((a) => (
          <li key={a.text}>
            <button
              type='button'
              onClick={a.onSelect}
              className='group flex w-full items-center gap-3 px-3 py-2.5 text-left transition-colors hover:bg-muted/40'
            >
              <a.icon
                className={cn(
                  "size-4 shrink-0",
                  a.severity === "danger" ? "text-destructive" : "text-warning"
                )}
              />
              <span className='text-xs'>{a.text}</span>
              <ArrowRight className='ml-auto size-3.5 text-muted-foreground/60 transition-colors group-hover:text-foreground' />
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
};
