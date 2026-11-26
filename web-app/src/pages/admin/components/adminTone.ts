/**
 * The admin console's one status vocabulary, and the only place it is given a colour.
 *
 * Every "is this fine?" signal on the staff surface — a workspace's health, a worker's
 * liveness, a compile's state, a nav badge — maps onto one of these five tones. A new
 * resource status maps onto an existing tone; it does not get a sixth.
 *
 * Classes come from the reference design's `status-*` / `dot-*` roles
 * (`styles/reference/theme.css`), which already carry their own light and dark values. That is why nothing here has a `dark:`
 * variant, and why a call site never needs a raw palette colour with a `dark:` twin again:
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
    dot: "bg-dot-success",
    text: "text-status-success",
    bg: "bg-status-success/10",
    ring: "ring-status-success/25"
  },
  // Emphasis, not an alert: an `admin` role, a `Cloning` workspace, a `Parent` marker.
  // Deliberately NOT the reference's `status-info` blue, though one exists: on this
  // surface colour means "something is wrong", so a label that merely informs must not
  // borrow a signal colour. `--primary` is the foreground ink in the reference palette
  // too, so this rule survives the restyle unchanged.
  info: {
    dot: "bg-primary",
    text: "text-primary",
    bg: "bg-primary/5",
    ring: "ring-primary/15"
  },
  warn: {
    dot: "bg-dot-warning",
    text: "text-status-warning",
    bg: "bg-status-warning/10",
    ring: "ring-status-warning/30"
  },
  danger: {
    dot: "bg-dot-error",
    text: "text-status-error",
    bg: "bg-status-error/10",
    ring: "ring-status-error/25"
  },
  muted: {
    dot: "bg-disabled-foreground",
    text: "text-status-neutral",
    bg: "bg-status-neutral-bg",
    ring: "ring-border-subtle"
  }
};
