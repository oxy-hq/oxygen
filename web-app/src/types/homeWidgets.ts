/** Home widgets: each app's live cards on the home (`internal-docs/home-widgets.md`). */

export type WidgetLevel = "ok" | "info" | "attention" | "critical";
type WidgetSize = "regular" | "wide" | "full";
/** The home's sections, in the order the org's layout draws them. */
export type HomeSectionKind = "widgets" | "apps" | "insights";
/** One section as the org's layout draws it: a widgets group lists its copies by key. */
export type HomeSection =
  | { kind: "widgets"; title: string | null; items: string[] }
  | { kind: "apps" }
  | { kind: "insights" };
type WidgetParams = Record<string, string | number | boolean>;
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

interface WidgetRow {
  title: string;
  detail: string | null;
  state: WidgetRowState | null;
  /** Absolute, on the app. */
  href: string | null;
  button: WidgetButton | null;
}

/** A KPI: one number that matters, and how it moved. */
export interface WidgetMetric {
  value: string;
  label: string | null;
  delta: string | null;
  trend: "up" | "down" | "flat" | null;
  /** Drawn as a sparkline. */
  spark: number[];
}

interface WidgetView {
  headline: string;
  detail: string | null;
  level: WidgetLevel;
  progress: { done: number; total: number } | null;
  metric: WidgetMetric | null;
  rows: WidgetRow[];
  buttons: WidgetButton[];
  /** Where "Open" goes: absolute, on the app. */
  href: string | null;
  badge: number | null;
  /** Read the widget again after this many seconds. */
  refresh_after: number | null;
}

interface WidgetApp {
  id: string;
  slug: string;
  name: string;
  org_slug: string;
  url: string;
}

/** One widget: its view, or why there is none. */
export interface WidgetCard {
  app: WidgetApp;
  /** The copy's name: `<app>/<widget>` or `<app>/<widget>#<id>`. */
  key: string;
  id: string;
  /** The copy's id, when the layout placed more than one. */
  instance: string | null;
  /** The layout's title for this copy, else the widget's. */
  title: string;
  params: WidgetParams;
  /** The size it is drawn at: the org layout's, else the widget's default. */
  size: WidgetSize;
  sizes: WidgetSize[];
  view: WidgetView | null;
  error: string | null;
}

/** Whether Today shows on the person's own home. */
export type TodayPreference = "shown" | "hidden";

/** A copy the person hid: enough to name it in a restore list. */
export interface HiddenWidget {
  key: string;
  title: string;
  app: WidgetApp;
}

/** A person's own choices about the home, per org. No row on the server means the defaults. */
export interface HomePreferences {
  /** The keys of the widget copies they hid. */
  hidden: string[];
  today: TodayPreference;
  /**
   * The kinds of the org layout's sections, in the order the home draws them
   * (`["apps", "insights"]`), answered with the preferences so the home knows
   * whether the org kept Insights while Today is hidden and the widgets are
   * not read. Read-only: a save never sends it. Absent from an older server.
   */
  sections?: HomeSectionKind[];
}

export interface HomeWidgetsResponse {
  widgets: WidgetCard[];
  sections: HomeSection[];
  /** The copies the person hid that they could otherwise see. Absent from an older server. */
  hidden?: HiddenWidget[];
  /** The person's Today preference, echoed. Absent from an older server. */
  today?: TodayPreference;
}

export interface WidgetPress {
  appId: string;
  widget: string;
  /** The copy pressed, so its card alone takes the answer's view. */
  key: string;
  button: WidgetButton;
}

export interface WidgetPressResponse {
  ok: true;
  message: string | null;
  view: WidgetView | null;
}
