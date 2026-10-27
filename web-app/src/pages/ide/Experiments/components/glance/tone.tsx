import { AlertTriangle, CheckCircle2, CircleMinus, Info, OctagonAlert } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/libs/shadcn/utils";

/** How a fact reads at a glance. Colour only: every word beside it is the server's or a plain label. */
export type Tone = "success" | "warning" | "danger" | "info" | "neutral";

/** Tinted surfaces with foreground text, so the colour never carries the contrast. */
export const TONE_SURFACE: Record<Tone, string> = {
  success: "border-success/40 bg-success/10",
  warning: "border-warning/50 bg-warning/10",
  danger: "border-destructive/40 bg-destructive/10",
  info: "border-info/40 bg-info/10",
  neutral: "border-border bg-muted/50"
};

const TONE_ICON_CLASS: Record<Tone, string> = {
  success: "text-success",
  warning: "text-warning",
  danger: "text-destructive",
  info: "text-info",
  neutral: "text-muted-foreground"
};

const TONE_ICON = {
  success: CheckCircle2,
  warning: AlertTriangle,
  danger: OctagonAlert,
  info: Info,
  neutral: CircleMinus
} satisfies Record<Tone, unknown>;

export function ToneIcon({ tone, className }: { tone: Tone; className?: string }) {
  const Icon = TONE_ICON[tone];
  return <Icon aria-hidden className={cn("size-3.5 shrink-0", TONE_ICON_CLASS[tone], className)} />;
}

/** A coloured status pill: an icon in the tone's colour and the label in foreground text. */
export function ToneBadge({
  tone,
  testId,
  title,
  children
}: {
  tone: Tone;
  testId?: string;
  title?: string;
  children: ReactNode;
}) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-md border px-2 py-0.5 font-medium text-foreground text-xs",
        TONE_SURFACE[tone]
      )}
      data-testid={testId}
      data-tone={tone}
      title={title}
    >
      <ToneIcon tone={tone} />
      <span>{children}</span>
    </span>
  );
}
