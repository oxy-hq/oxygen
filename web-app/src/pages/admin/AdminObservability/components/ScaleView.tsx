import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import type { Fleet, FleetRow } from "@/services/api/adminObservability";
import { AdminSectionLabel } from "../../components/AdminSectionLabel";
import { ADMIN_TONE } from "../../components/adminTone";
import { bytes, compact, usd } from "../format";
import {
  cellOf,
  maxIndex,
  type ScaleDimension,
  type ScaleDimensionId,
  shownDimensions
} from "../scaleDimensions";

/**
 * How big each org is against the typical one, on every axis at once.
 *
 * The table beside this answers "what does each org cost us". This answers a
 * different question — **which org is an outlier, and on which axis** — and no
 * column of absolute figures can: the biggest tenant is simply biggest
 * everywhere, so the *shape* of an account is invisible. An org at 12× the
 * median on LLM and 1.2× on storage is a different business to serve from one
 * where that is reversed.
 *
 * ## Why bars and not a heatmap
 *
 * Magnitude read by length, on a shared baseline, is the comparison people are
 * most accurate at; colour-as-magnitude is the one they are worst at, and on a
 * console whose rule is that colour means *something is wrong* it would also
 * be saying the wrong thing. So every cell is one hue, length-encoded, with
 * its multiple written beside it — nothing here depends on telling two colours
 * apart.
 *
 * ## One baseline for the whole grid
 *
 * Every bar is scaled against the widest index anywhere in it, so a row reads
 * across *and* a column reads down. Per-row scaling would make every org's
 * largest axis look identical, erasing exactly the difference being shown.
 */
export const ScaleView = ({ fleet }: { fleet: Fleet }) => {
  const { shown, hidden } = shownDimensions(fleet);
  const rows = [...fleet.rows].sort(
    (a, b) => b.cost_usd - a.cost_usd || a.org_name.localeCompare(b.org_name)
  );
  const max = maxIndex(rows, fleet.medians, shown);

  if (shown.length === 0 || rows.length === 0) {
    return (
      <p className='text-muted-foreground text-xs' data-testid='admin-observability-scale-empty'>
        Nothing measured in this window, so there is nothing to compare.
      </p>
    );
  }

  return (
    <div className='space-y-4' data-testid='admin-observability-scale'>
      <p className='max-w-3xl text-muted-foreground text-xs'>
        Each bar is the org&rsquo;s multiple of the <strong>median</strong> org on that axis — the
        typical account, which one dominant tenant cannot drag. Absolute figures sit beside the
        bars, because a large multiple over a tiny median is not a large number.
      </p>

      <div className='space-y-4'>
        {rows.map((row) => (
          <OrgCard key={row.org_id} row={row} fleet={fleet} dimensions={shown} max={max} />
        ))}
      </div>

      {!fleet.storage_read ? (
        <p
          className={cn("text-xs", ADMIN_TONE.warn.text)}
          data-testid='admin-observability-scale-storage-failed'
        >
          Couldn&rsquo;t read storage for this window, so the storage axis is blank for every
          organization. That is a fault, not an empty bucket.
        </p>
      ) : null}
      {hidden.length > 0 ? (
        <p className='text-muted-foreground text-xs' data-testid='admin-observability-scale-hidden'>
          {hidden.map((d) => d.label).join(", ")} {hidden.length === 1 ? "is" : "are"} not shown
          because no organization can answer {hidden.length === 1 ? "it" : "them"} on this
          deployment.
        </p>
      ) : null}
    </div>
  );
};

const OrgCard = ({
  row,
  fleet,
  dimensions,
  max
}: {
  row: FleetRow;
  fleet: Fleet;
  dimensions: ScaleDimension[];
  max: number;
}) => (
  <div
    className='rounded-lg border border-border/60 bg-card p-4'
    data-testid='admin-observability-scale-org'
  >
    <AdminSectionLabel
      className='mb-3'
      trailing={
        <Link
          to={`${ROUTES.ADMIN.OBSERVABILITY_ORG(row.org_id)}?days=${fleet.window_days}`}
          className='underline underline-offset-2 hover:text-foreground'
        >
          Open
        </Link>
      }
    >
      {row.org_name}
    </AdminSectionLabel>

    <div className='space-y-2'>
      {dimensions.map((d) => {
        const { value, index } = cellOf(row, fleet.medians, d.id);
        return (
          <div
            key={d.id}
            className='grid grid-cols-[7rem_1fr_5rem_4rem] items-center gap-3'
            data-testid={`admin-observability-scale-cell-${d.id}`}
          >
            <span className='truncate text-[11px] text-muted-foreground' title={d.hint}>
              {d.label}
            </span>
            <div className='h-1.5 w-full overflow-hidden rounded-full bg-muted'>
              {index === null ? null : (
                <div
                  className={cn(
                    "h-full rounded-full",
                    // One hue. A bar above the typical account is solid; at or
                    // below it, lighter — a lightness step, not a second hue,
                    // and never the only thing carrying the meaning.
                    index >= 1 ? "bg-primary/80" : "bg-primary/35"
                  )}
                  // A real but tiny multiple still draws a sliver: "almost
                  // nothing" and "nothing" are different answers.
                  style={{ width: `${Math.max((index / max) * 100, value ? 1.5 : 0)}%` }}
                />
              )}
            </div>
            <span className='text-right text-[11px] tabular-nums'>{format(d.id, value)}</span>
            <span
              className={cn(
                "text-right text-[11px] tabular-nums",
                index !== null && index >= 1 ? "font-medium" : "text-muted-foreground"
              )}
            >
              {index === null ? "—" : `${index < 10 ? index.toFixed(1) : Math.round(index)}×`}
            </span>
          </div>
        );
      })}
    </div>
  </div>
);

/** The absolute figure beside a bar, in the unit its axis is measured in. */
function format(id: ScaleDimensionId, value: number | null): string {
  if (value === null) return "—";
  switch (id) {
    case "cost_usd":
      return usd(value);
    case "egress_gb":
      return bytes(value * 1_000_000_000);
    case "storage_gib_month":
      return `${compact(value)} GiB`;
    case "function_compute_gbs":
      return `${compact(value)} GB·s`;
    default:
      return compact(value);
  }
}
