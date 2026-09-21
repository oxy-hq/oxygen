/**
 * The admin console's one status vocabulary, and the only place it is given a colour.
 *
 * Every "is this fine?" signal on the staff surface — a workspace's health, a worker's
 * liveness, a compile's state, a nav badge — maps onto one of these five tones. A new
 * resource status maps onto an existing tone; it does not get a sixth.
 *
 * Classes come from the design system's `status-*` theme tokens (`styles/shadcn`), which
 * already carry their own light and dark values. That is why nothing here has a `dark:`
 * variant, and why a call site never needs `text-amber-700 dark:text-amber-400` again:
 * before this file the console spelled its tones out by hand in ~100 places, in raw
 * palette colours the repo's own style rule forbids.
 */
export type AdminTone = "ok" | "info" | "warn" | "danger" | "muted";

export type AdminToneClasses = {
  /** Solid fill for a status dot or a meter segment. */
  dot: string;
  /** Foreground for a label or an icon. */
  text: string;
  /** Tinted surface for a pill, a badge, or a callout. */
  bg: string;
  /** Hairline for a pill or a callout, paired with `ring-1 ring-inset`. */
  ring: string;
};

export const ADMIN_TONE: Record<AdminTone, AdminToneClasses> = {
  ok: {
    dot: "bg-success",
    text: "text-status-success-text",
    bg: "bg-status-success-bg",
    ring: "ring-success/20"
  },
  // Neutral emphasis, not an alert: an `admin` role, a `Cloning` workspace, a `Parent`
  // marker. It stays in the foreground colour on purpose — on this surface colour means
  // "something is wrong", so a label that merely informs must not borrow the blue.
  info: {
    dot: "bg-primary",
    text: "text-primary",
    bg: "bg-primary/5",
    ring: "ring-primary/15"
  },
  warn: {
    dot: "bg-warning",
    text: "text-status-warning-text",
    bg: "bg-status-warning-bg",
    ring: "ring-warning/25"
  },
  danger: {
    dot: "bg-destructive",
    text: "text-status-error-text",
    bg: "bg-status-error-bg",
    ring: "ring-destructive/20"
  },
  muted: {
    dot: "bg-muted-foreground/60",
    text: "text-status-neutral-text",
    bg: "bg-status-neutral-bg",
    ring: "ring-border"
  }
};

/** Severity order, worst first — the order a triage list sorts by. */
export const ADMIN_TONE_SEVERITY: readonly AdminTone[] = ["danger", "warn", "info", "muted", "ok"];

/** The worst tone in a set, or `ok` for an empty one. */
export function worstTone(tones: readonly AdminTone[]): AdminTone {
  return ADMIN_TONE_SEVERITY.find((t) => tones.includes(t)) ?? "ok";
}
