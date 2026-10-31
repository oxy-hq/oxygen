import { useState } from "react";
import { cn } from "@/libs/shadcn/utils";

// Size variants cover every surface that shows an app glyph: the admin table/card
// (sm), the homepage launcher card (md), the admin detail header (lg), and the
// home's app grid on a phone (xl), where the icon is the tap target.
const SIZES = {
  sm: "size-5 text-xs",
  md: "size-6 text-xs",
  lg: "size-8 text-sm",
  xl: "size-12 text-base"
} as const;

/**
 * A monogram's ground: one of the theme's chart hues, picked by the app's name so
 * an app keeps its colour everywhere and "Store Ops" and "Site Scout" do not look
 * alike. Whole class names, so Tailwind sees them.
 */
const TINTS = [
  "bg-chart-1/15",
  "bg-chart-2/15",
  "bg-chart-3/20",
  "bg-chart-4/15",
  "bg-chart-5/15"
] as const;

function tintOf(name: string): string {
  let hash = 0;
  for (const ch of name) hash = (hash * 31 + (ch.codePointAt(0) ?? 0)) >>> 0;
  return TINTS[hash % TINTS.length] ?? TINTS[0];
}

/** "Store Ops" -> "SO", "Warehouse" -> "W": two words' initials, else the first letter. */
export function monogram(name: string, letters: 1 | 2): string {
  const words = name.trim().split(/\s+/).filter(Boolean);
  const first = words[0]?.[0] ?? "?";
  const second = letters === 2 && words.length > 1 ? (words[1]?.[0] ?? "") : "";
  return (first + second).toUpperCase();
}

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
          // The letters stay in the foreground colour: a chart hue as text fails contrast.
          "flex shrink-0 items-center justify-center rounded-md border font-semibold text-foreground",
          tintOf(name),
          box,
          className
        )}
      >
        {/* The smallest mark has room for one letter. */}
        {monogram(name, size === "sm" ? 1 : 2)}
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
