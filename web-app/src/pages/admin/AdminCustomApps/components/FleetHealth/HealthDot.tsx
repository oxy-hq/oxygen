import { cn } from "@/libs/shadcn/utils";
import type { AppHealth } from "@/types/apps";

/**
 * The verdict glyph, and the one deliberate design decision on this surface.
 *
 * A colour on a status scale carries an implied *ordering* — red is worse than
 * amber is worse than green. `not_measured` is not a point on that scale: it is
 * the absence of a reading, and giving it a fourth colour would file it as a
 * fourth severity, which is how "we are not watching this app" gets read as
 * "this app is a bit unwell" and then ignored.
 *
 * So severities are **filled** dots on the colour scale, and the two non-verdicts
 * are **hollow**: `quiet` a plain ring (known, idle), `not_measured` a *dashed*
 * ring (no signal at all). The difference is legible before the label is read,
 * and it survives greyscale and colour-blindness — which a fourth hue would not.
 *
 * Emerald is deliberately not used for `operational`: it is reserved for
 * workflow-node success, and `StatusDot` already leans on the brand `primary`
 * token for the same reason.
 */
const GLYPH: Record<AppHealth, string> = {
  down: "bg-destructive ring-2 ring-destructive/25",
  degraded: "bg-amber-500 ring-2 ring-amber-500/25",
  not_measured: "border border-muted-foreground/60 border-dashed",
  quiet: "border border-muted-foreground/40",
  operational: "bg-primary ring-2 ring-primary/20"
};

export const HEALTH_LABEL: Record<AppHealth, string> = {
  down: "Down",
  degraded: "Degraded",
  not_measured: "Not measured",
  quiet: "Quiet",
  operational: "Operational"
};

export const HealthDot = ({
  health,
  className,
  decorative
}: {
  health: AppHealth;
  className?: string;
  /** Skip the a11y label when the word sits right beside the dot, so a screen
   *  reader doesn't announce the verdict twice. */
  decorative?: boolean;
}) => (
  <span
    {...(decorative
      ? { "aria-hidden": true }
      : { role: "img", "aria-label": HEALTH_LABEL[health] })}
    className={cn("size-2 shrink-0 rounded-full", GLYPH[health], className)}
    data-testid={`admin-fleet-health-dot-${health}`}
  />
);

/** Dot plus word, for the table cell. */
export const HealthBadge = ({ health }: { health: AppHealth }) => (
  <span className='inline-flex items-center gap-1.5 whitespace-nowrap'>
    <HealthDot health={health} decorative />
    <span
      className={cn(
        "text-xs",
        health === "down" && "font-medium text-destructive",
        health === "degraded" && "font-medium text-amber-600 dark:text-amber-500",
        // The two non-verdicts read as muted text: they are not claims about
        // the app, so they should not compete with the ones that are.
        (health === "not_measured" || health === "quiet") && "text-muted-foreground"
      )}
    >
      {HEALTH_LABEL[health]}
    </span>
  </span>
);
