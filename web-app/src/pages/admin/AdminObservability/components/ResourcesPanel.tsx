import { CircleHelp, Divide, Target } from "lucide-react";
import type { ComponentType } from "react";
import { cn } from "@/libs/shadcn/utils";
import type { Attribution, Meter, OrgResources } from "@/services/api/adminObservability";
import { AdminSectionLabel } from "../../components/AdminSectionLabel";
import { ADMIN_TONE } from "../../components/adminTone";
import { compact, usd } from "../format";

/**
 * What this org consumes beyond tokens, and how far each figure can be trusted.
 *
 * The panel's whole design is the **attribution column**. A storage figure we
 * can stand behind and an OLTP figure we cannot must not sit in one list
 * looking alike, because the reader would average them into a single
 * confidence. So each row says which it is, in the order direct → apportioned →
 * unmeasured, and the total is marked a floor when anything is missing.
 *
 * This is the correction to the first cut's anti-goal. Refusing to *smear a
 * shared bill* across orgs was right; refusing to *meter* was not, and a
 * per-org storage meter had been sitting in `oxy-custom-apps` unused by any
 * cost surface the whole time.
 */
export const ResourcesPanel = ({ resources }: { resources: OrgResources }) => (
  <section className='space-y-4' data-testid='admin-observability-resources'>
    <AdminSectionLabel trailing={`last ${resources.window_days}d`}>
      Infrastructure
    </AdminSectionLabel>

    <div className=' border border-border/60 bg-card'>
      <div className='flex flex-wrap items-end justify-between gap-4 border-border/60 border-b p-4'>
        <div>
          <span className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
            {resources.partial ? "Measured infrastructure" : "Infrastructure"}
          </span>
          <div
            className='font-semibold text-2xl tabular-nums tracking-tight'
            data-testid='admin-observability-resources-total'
          >
            {usd(resources.infra_cost_usd)}
          </div>
        </div>
        <p className='max-w-md text-[11px] text-muted-foreground'>
          {/* Said once, here, rather than implied by an asterisk: the LLM
              figure above is not included, and the total below is a floor. */}
          Excludes LLM, which is the panel above.
          {resources.partial
            ? " Some resources cannot be measured yet, so this is a floor rather than a bill."
            : null}
        </p>
      </div>

      <div className='divide-y divide-border/50'>
        {resources.meters.map((m) => (
          <MeterRow key={m.key} meter={m} />
        ))}
      </div>

      <BasisFooter resources={resources} />
    </div>
  </section>
);

const ATTRIBUTION: Record<
  Attribution,
  { label: string; icon: ComponentType<{ className?: string }>; tone: "ok" | "info" | "muted" }
> = {
  // Not a status colour: this is *confidence*, a different axis, so direct
  // takes the plain foreground and only the weakest class is tinted.
  direct: { label: "Measured", icon: Target, tone: "muted" },
  apportioned: { label: "Apportioned", icon: Divide, tone: "info" },
  unmeasured: { label: "Not measured", icon: CircleHelp, tone: "muted" }
};

const MeterRow = ({ meter }: { meter: Meter }) => {
  const a = ATTRIBUTION[meter.attribution];
  const Icon = a.icon;
  const unmeasured = meter.attribution === "unmeasured";

  return (
    <div className='space-y-1 p-4' data-testid={`admin-observability-meter-${meter.key}`}>
      <div className='flex flex-wrap items-baseline gap-x-3 gap-y-1'>
        <span className={cn("font-medium text-xs", unmeasured && "text-muted-foreground")}>
          {meter.label}
        </span>
        <span
          className={cn(
            "inline-flex items-center gap-1 rounded-full px-1.5 py-0.5 text-[10px]",
            ADMIN_TONE[a.tone].text,
            a.tone !== "muted" && ADMIN_TONE[a.tone].bg
          )}
        >
          <Icon className='size-3' />
          {a.label}
        </span>
        <span className='ml-auto text-muted-foreground text-xs tabular-nums'>
          {/* A dash, never a zero: `quantity: null` means nobody measured it. */}
          {meter.quantity === null ? "—" : `${compact(meter.quantity)} ${meter.unit}`}
        </span>
        <span className='w-20 text-right font-medium text-xs tabular-nums'>
          {usd(meter.cost_usd)}
        </span>
      </div>
      {meter.note ? (
        <p className='max-w-3xl text-[11px] text-muted-foreground'>{meter.note}</p>
      ) : null}
    </div>
  );
};

/**
 * The rates and ceilings every figure above rests on.
 *
 * Shown, not hidden behind a tooltip: a dollar figure whose rate a reader
 * cannot see is one they have to either trust blindly or ignore, and this
 * panel's whole claim is that its numbers can be checked.
 */
const BasisFooter = ({ resources }: { resources: OrgResources }) => {
  const b = resources.basis;
  return (
    <div
      className='flex flex-wrap gap-x-5 gap-y-1 border-border/60 border-t bg-muted/20 px-4 py-2.5 text-[10px] text-muted-foreground'
      data-testid='admin-observability-resources-basis'
    >
      <span>Storage ${b.storage_usd_per_gb_month}/GiB-month (S3 list)</span>
      <span>Egress ${b.egress_usd_per_gb}/GB (AWS list)</span>
      <span>
        {b.provisioned_memory_mb === null
          ? "Isolate ceiling disabled — no provisioned basis for GB·s"
          : `GB·s at a ${b.provisioned_memory_mb} MiB isolate ceiling`}
      </span>
      <span>
        {b.fleet_cost_usd_month === null ? (
          <>
            No fleet cost configured — set <code className='font-mono'>{b.fleet_cost_env}</code>
          </>
        ) : (
          `Fleet ${usd(b.fleet_cost_usd_month)}/mo, apportioned by wall time`
        )}
      </span>
    </div>
  );
};
