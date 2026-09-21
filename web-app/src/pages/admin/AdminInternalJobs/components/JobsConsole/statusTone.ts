import { ADMIN_TONE, type AdminTone } from "@/pages/admin/components/adminTone";

/**
 * Cockpit status palette. Status is carried by a small left-accent + dot rather
 * than a big badge, so this returns the Tailwind classes the row/debug panel
 * apply. The five console tones replace the bespoke emerald / amber /
 * destructive triples this file used to spell out in raw palette colours.
 */
export interface StatusTone {
  /** Dot / left-accent background. */
  accent: string;
  /** Text color for the status label. */
  text: string;
}

/** Which of the console's five tones a queue status reads as. */
export function queueStatusTone(status: string): AdminTone | null {
  switch (status) {
    case "dead":
      return "danger";
    case "failed":
      return "warn";
    case "claimed":
      return "info";
    case "completed":
      return "ok";
    case "cancelled":
      return "muted";
    default:
      // No tone: an unrecognised status stays in the plain foreground rather
      // than borrowing a colour that would read as a verdict.
      return null;
  }
}

export function statusTone(status: string): StatusTone {
  const tone = queueStatusTone(status);
  if (!tone) return { accent: "bg-foreground/60", text: "text-foreground" };
  const v = ADMIN_TONE[tone];
  return { accent: v.dot, text: v.text };
}
