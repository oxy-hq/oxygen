import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE, type AdminTone } from "@/pages/admin/components/adminTone";

/** Slice a possibly-undefined string. Lets rows render without crashing
 *  on a malformed row from a misconfigured deployment. */
export function safeSlice(value: string | null | undefined, end: number): string {
  if (typeof value !== "string") return "—";
  return value.slice(0, end);
}

export function formatMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  const m = Math.floor(ms / 60_000);
  const s = Math.floor((ms % 60_000) / 1000);
  return `${m}m ${s}s`;
}

export function formatRelative(iso: string | null | undefined): string {
  if (!iso) return "—";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "—";
  const diff = Math.max(0, Math.floor((Date.now() - then) / 1000));
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

/** Which admin tone a compile status reads as. Was spelled in raw palette
 *  colours here (emerald=ready, destructive=failed, amber=compiling); the
 *  console now has one status vocabulary, so the mapping names a tone and
 *  `adminTone.ts` owns the colour. */
export function compileTone(status: string | null | undefined): AdminTone {
  switch (status) {
    case "ready":
      return "ok";
    case "failed":
      return "danger";
    case "compiling":
      return "warn";
    default:
      return "muted";
  }
}

/** `<Badge>` classes for a tone. `border-transparent` because the hairline is
 *  the tone's ring, the way `AdminStatusPill` draws one. */
export function toneBadgeClass(tone: AdminTone): string {
  const v = ADMIN_TONE[tone];
  return cn("border-transparent ring-1 ring-inset", v.bg, v.text, v.ring);
}

/** Badge classes for a compile status accent. */
export function statusAccent(status: string | null | undefined): string {
  return toneBadgeClass(compileTone(status));
}
