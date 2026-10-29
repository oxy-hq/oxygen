/** Home widgets: each app's live cards on the home (`internal-docs/home-widgets.md`). */

export type WidgetLevel = "ok" | "info" | "attention" | "critical";
export type WidgetRowState = "done" | "ready" | "blocked" | "failed" | "pending";

/** A button, as the server checked it. Its `input` and `key` go back with the press. */
export interface WidgetButton {
  action: string;
  label: string;
  input: unknown;
  key: string | null;
  /** Set whenever the home must ask before running it. */
  confirm: string | null;
  destructive: boolean;
}

export interface WidgetRow {
  title: string;
  detail: string | null;
  state: WidgetRowState | null;
  /** Absolute, on the app. */
  href: string | null;
  button: WidgetButton | null;
}

export interface WidgetView {
  headline: string;
  detail: string | null;
  level: WidgetLevel;
  progress: { done: number; total: number } | null;
  rows: WidgetRow[];
  buttons: WidgetButton[];
  /** Where "Open" goes: absolute, on the app. */
  href: string | null;
  badge: number | null;
}

export interface WidgetApp {
  id: string;
  slug: string;
  name: string;
  org_slug: string;
  url: string;
}

/** One widget: its view, or why there is none. */
export interface WidgetCard {
  app: WidgetApp;
  id: string;
  title: string;
  size: "regular" | "wide";
  view: WidgetView | null;
  error: string | null;
}

export interface HomeWidgetsResponse {
  widgets: WidgetCard[];
}

export interface WidgetPress {
  appId: string;
  widget: string;
  button: WidgetButton;
}

export interface WidgetPressResponse {
  ok: true;
  message: string | null;
  view: WidgetView | null;
}
