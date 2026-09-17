import { cn } from "@/libs/shadcn/utils";
import { AdminStatusPill } from "@/pages/admin/components/AdminStatusPill";
import {
  type AppStatus,
  ATTENTION,
  STATUS_LABEL,
  type StatusCounts,
  type StatusFilter
} from "../useAppsTable";
import { ATTENTION_TONE } from "./AppStatus";

/** Chip order: the states that need someone first, worst first. */
const ORDER: AppStatus[] = ["down", "degraded", "not_measured", "quiet", "operational", "draft"];

/**
 * The fleet summary and the status filter, as one control — the same pattern as
 * Airhouse's `FleetFilterChips`, and for the same reason: a count an operator can
 * read but not act on sends them hunting down the list by eye.
 *
 * One deliberate departure. `FleetFilterChips` keeps an empty chip visible but
 * disabled, because with three states a missing chip reads as "no such state".
 * With seven, that rule fills the bar with greyed chips on a healthy day, which is
 * the clutter this page is meant to lose. So `All` and `Needs attention` always
 * show — `Needs attention 0` carries the stable "nothing right now" shape — and a
 * specific state appears only when something is in it. Colour on the bar then
 * means the same thing it means in the rows: look here.
 */
export const StatusFilterChips = ({
  counts,
  total,
  active,
  onChange
}: {
  counts: StatusCounts;
  total: number;
  active: StatusFilter;
  onChange: (next: StatusFilter) => void;
}) => {
  // `attention` selects all three attention states at once, so each of their
  // chips reads as selected too — the bar never contradicts the rows below it.
  const isActive = (s: AppStatus) => active === s || (active === "attention" && ATTENTION.has(s));
  const visible = ORDER.filter((s) => counts[s] > 0);

  return (
    <div className='flex flex-wrap items-center gap-1' data-testid='admin-apps-status-filters'>
      <Chip
        id='all'
        selected={active === "all"}
        onClick={() => onChange("all")}
        label={
          <>
            All <span className='tabular-nums'>{total}</span>
          </>
        }
      />
      <Chip
        id='attention'
        selected={active === "attention"}
        disabled={counts.attention === 0}
        onClick={() => onChange(active === "attention" ? "all" : "attention")}
        label={
          counts.attention > 0 ? (
            // The aggregate takes its worst member's tone: red only when something
            // is actually down. A fleet whose only problem is an unmeasured app
            // should not look like an outage.
            <AdminStatusPill
              tone={counts.down > 0 ? "danger" : "warn"}
              label={`Needs attention ${counts.attention}`}
            />
          ) : (
            <span className='text-muted-foreground'>
              Needs attention <span className='tabular-nums'>0</span>
            </span>
          )
        }
      />

      {visible.length > 0 && <span className='mx-1 h-4 w-px bg-border' aria-hidden />}

      {visible.map((s) => {
        const tone = ATTENTION_TONE[s];
        const text = `${STATUS_LABEL[s]} ${counts[s]}`;
        return (
          <Chip
            key={s}
            id={s}
            selected={isActive(s)}
            onClick={() => onChange(active === s ? "all" : s)}
            label={
              tone ? (
                <AdminStatusPill tone={tone} label={text} />
              ) : (
                <span className='text-muted-foreground'>{text}</span>
              )
            }
          />
        );
      })}
    </div>
  );
};

const Chip = ({
  id,
  label,
  selected,
  disabled,
  onClick
}: {
  id: string;
  label: React.ReactNode;
  selected: boolean;
  disabled?: boolean;
  onClick: () => void;
}) => (
  <button
    type='button'
    disabled={disabled}
    onClick={onClick}
    aria-pressed={selected}
    className={cn(
      "rounded-sm border px-1.5 py-0.5 text-xs transition-colors",
      "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
      selected
        ? "border-border bg-muted text-foreground"
        : "border-transparent hover:bg-muted/50 disabled:hover:bg-transparent",
      disabled && "opacity-50"
    )}
    data-testid={`admin-apps-filter-${id}`}
  >
    {label}
  </button>
);
