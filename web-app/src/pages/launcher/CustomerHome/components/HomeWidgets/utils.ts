import { isAxiosError } from "axios";
import type { WidgetCard, WidgetLevel, WidgetRowState } from "@/types/homeWidgets";

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

/** A card's test-id root; a copy's id keeps two copies of one widget apart. */
export function widgetTestId(card: WidgetCard): string {
  return `home-widget-${card.app.slug}-${card.id}${card.instance ? `-${card.instance}` : ""}`;
}

/** A widget whose function did not answer: no view, only the server's reason. */
export function isUnavailable(card: WidgetCard): boolean {
  return !card.view;
}

/**
 * A widget with nothing to do: it says all is well (`ok`) or only informs
 * (`info`) and offers no button. The home folds it to one line; a card with a
 * job, a button, or no view at all (it failed) keeps its full size.
 */
export function isCalm(card: WidgetCard): boolean {
  const view = card.view;
  if (!view) return false;
  if (view.level !== "ok" && view.level !== "info") return false;
  return view.buttons.length === 0 && view.rows.every((row) => !row.button);
}
