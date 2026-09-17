import { useState } from "react";
import { cn } from "@/libs/shadcn/utils";

// Size variants cover every surface that shows an app glyph: the admin table/card
// (sm), the homepage launcher card (md), and the admin detail header (lg).
const SIZES = {
  sm: "size-5 text-xs",
  md: "size-6 text-xs",
  lg: "size-8 text-sm"
} as const;

/**
 * The app's glyph (its manifest `icon`, resolved server-side to `icon_url`) with
 * a monogram fallback — the ONE way any surface renders an app icon. There is no
 * favicon.ico probe or per-surface derivation; if the app declares no `icon`, it
 * shows the monogram (never nothing). See the `oxy-app-visual-identity` skill.
 */
export function AppMark({
  iconUrl,
  name,
  size = "md",
  className,
  testId
}: {
  iconUrl?: string | null;
  name: string;
  size?: keyof typeof SIZES;
  className?: string;
  testId?: string;
}) {
  const [failed, setFailed] = useState(false);
  const box = SIZES[size];

  if (!iconUrl || failed) {
    return (
      <span
        aria-hidden
        data-testid={testId}
        className={cn(
          "flex shrink-0 items-center justify-center rounded-md border bg-primary/10 font-semibold text-primary",
          box,
          className
        )}
      >
        {name.slice(0, 1).toUpperCase()}
      </span>
    );
  }
  return (
    <img
      src={iconUrl}
      alt=''
      // Admin lists render one per app, most of them off-screen.
      loading='lazy'
      decoding='async'
      data-testid={testId}
      onError={() => setFailed(true)}
      className={cn("shrink-0 rounded-md border object-contain", box, className)}
    />
  );
}
