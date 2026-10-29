import { isAxiosError } from "axios";
import type { WidgetLevel, WidgetRowState } from "@/types/homeWidgets";

/** What a failed press says: the server's message (the app's own, when it gave one). */
export function pressError(error: unknown): string {
  if (isAxiosError(error)) {
    const said = (error.response?.data as { message?: unknown } | undefined)?.message;
    if (typeof said === "string" && said.trim()) return said;
  }
  return "That didn't work. Try again.";
}

/** The dot beside a headline: nothing for plain information. */
export const LEVEL_DOT: Record<WidgetLevel, string | null> = {
  ok: "bg-success",
  info: null,
  attention: "bg-warning",
  critical: "bg-destructive"
};

export const ROW_STATE: Record<WidgetRowState, { label: string; className: string }> = {
  done: { label: "Done", className: "text-success" },
  ready: { label: "Ready", className: "text-primary" },
  blocked: { label: "Blocked", className: "text-warning" },
  failed: { label: "Failed", className: "text-destructive" },
  pending: { label: "Pending", className: "text-muted-foreground" }
};
