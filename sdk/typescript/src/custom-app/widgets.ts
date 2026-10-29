// Home widgets an app declares in `oxy-app.json` (the app-level `widgets`
// block): the manifest types, the view a widget function returns, and the
// checks publish makes on the block.
//
// A widget is a live card on the Oxygen home: "19 stores ready to post", with
// the button that does it. The home never runs the app; it calls the widget's
// function, validates the JSON view that comes back and draws it with its own
// components. A button press calls one of the widget's declared actions, each
// another function of the app, once per `key` (an `Idempotency-Key`).
//
// ONE SOURCE, THREE PACKAGES. Like `notification-catalog.ts`, this is the only
// copy: `oxyc validate`, `oxyc publish` and `@oxy-hq/vite-plugin` import it
// through `@oxy-hq/sdk/manifest`, and `validateOxyAppManifest` runs
// `checkWidgets` right after `checkNotifications`.
//
// THE SERVER IS THE AUTHORITY. Every rule repeats the publish check in
// `crates/app-core/src/widgets/`; the one difference is on purpose: the server
// stops at the first problem, this lists every one. Each finding also carries
// the server's check id (`widget_too_many`, …) in `check`.
//
// Only the manifest-side types are read by the JSON Schema generator; the view
// types below are for a function author's return type and are not part of the
// manifest.
//
// Nothing past ES2020, so it compiles under all three packages' settings.

import { CATALOG_SEGMENT, type CatalogFinding } from "./notification-catalog";

/** How wide a widget's card is on the home: one column, or two. */
export type OxyAppWidgetSize = "regular" | "wide";

/** A button on a widget: a declared function the home runs when it is pressed. */
export interface OxyAppWidgetAction {
  /**
   * A function this app declares. Pressing the button calls it as the signed-in
   * person with `{ widget, action, input }` and an `Idempotency-Key` header.
   */
  function: string;
  /** The home asks before running it (the button's own `confirm` text, if any). */
  confirm?: boolean;
  /** Drawn as a destructive button. */
  destructive?: boolean;
}

/** One widget, keyed by its id in {@link OxyAppManifest.widgets}. */
export interface OxyAppWidget {
  /**
   * The card's heading, 1–40 characters.
   *
   * @maxLength 40
   * @minLength 1
   */
  title: string;
  /**
   * A function this app declares. The home calls it with `{ widget: "<id>" }`
   * and expects a {@link WidgetView} back.
   */
  function: string;
  /** `regular` (default) or `wide` (two columns). */
  size?: OxyAppWidgetSize;
  /**
   * The buttons this widget may run, keyed by action id (1–32 lowercase
   * letters, digits, `_` or `-`). At most 3. A button in a view naming an
   * action missing here is dropped.
   */
  actions?: Record<string, OxyAppWidgetAction>;
}

// ── The view a widget function returns ──────────────────────────────────────

/** How loudly a widget speaks. Default `info`. */
export type WidgetLevel = "ok" | "info" | "attention" | "critical";

/** What a row's state dot says. */
export type WidgetRowState = "done" | "ready" | "blocked" | "failed" | "pending";

/** A button in a view: runs one of the widget's declared actions. */
export interface WidgetButton {
  /** An action id this widget declares. */
  action: string;
  /** The button's text, ≤ 32 characters. */
  label: string;
  /** Passed to the action as `input`; at most 4 KB of JSON. */
  input?: unknown;
  /**
   * What makes two presses the same press (`"2026-09"`). Becomes part of the
   * idempotency key; absent, a hash of `input` stands in.
   */
  key?: string;
  /** The question the home asks before running it, ≤ 160 characters. */
  confirm?: string;
}

/** One line on a widget: a store, an invoice, a task. */
export interface WidgetRow {
  /** ≤ 60 characters. */
  title: string;
  /** ≤ 120 characters. */
  detail?: string;
  state?: WidgetRowState;
  /** App-relative: starts with `/` or `?`. Anything else is dropped. */
  link?: string;
  /** A per-row action, such as posting one store. */
  button?: WidgetButton;
}

/**
 * What a widget function returns. The server trims text to its limit, cuts
 * lists, drops links that are not app-relative and buttons naming an
 * undeclared action; no `headline` shows "This widget returned nothing to show".
 */
export interface WidgetView {
  /** ≤ 80 characters. */
  headline: string;
  /** ≤ 120 characters. */
  detail?: string;
  /** Default `info`. */
  level?: WidgetLevel;
  progress?: { done: number; total: number };
  /** At most 6. */
  rows?: WidgetRow[];
  /** At most 2. */
  buttons?: WidgetButton[];
  /** Where "Open" goes: app-relative (`/` or `?`). */
  link?: string;
  /** A count for the home to show. */
  badge?: number;
}

/** What an action function may return: a new view, a line to show, or nothing. */
export interface WidgetActionResult {
  view?: WidgetView;
  message?: string;
}

// ── Limits (the server's, in `crates/app-core/src/widgets/`) ────────────────

/** Widgets in one app, at most. */
export const MAX_WIDGETS = 3;
/** Actions on one widget, at most. */
export const MAX_WIDGET_ACTIONS = 3;
/** A widget's `title`, in characters. */
export const MAX_WIDGET_TITLE_CHARS = 40;
/** A view's `headline`, in characters. */
export const MAX_WIDGET_HEADLINE_CHARS = 80;
/** A view's or a row's `detail`, in characters. */
export const MAX_WIDGET_DETAIL_CHARS = 120;
/** A row's `title`, in characters. */
export const MAX_WIDGET_ROW_TITLE_CHARS = 60;
/** Rows in a view, at most. */
export const MAX_WIDGET_ROWS = 6;
/** Buttons in a view, at most. */
export const MAX_WIDGET_BUTTONS = 2;
/** A button's `label`, in characters. */
export const MAX_WIDGET_BUTTON_LABEL_CHARS = 32;
/** A button's `confirm` text, in characters. */
export const MAX_WIDGET_BUTTON_CONFIRM_CHARS = 160;
/** A button's `input`, serialized, in bytes. */
export const MAX_WIDGET_BUTTON_INPUT_BYTES = 4096;
/** Widget sizes, in the order the home lists them. */
export const WIDGET_SIZES: readonly OxyAppWidgetSize[] = ["regular", "wide"];

// ── The publish checks ──────────────────────────────────────────────────────

/** The server's check ids for a bad `widgets` block. */
export type WidgetCheckId =
  | "widget_invalid"
  | "widget_too_many"
  | "widget_function_undeclared"
  | "widget_actions_too_many"
  | "widget_action_invalid"
  | "widget_action_function_undeclared";

/** A {@link CatalogFinding} that also says which publish check it is. */
export interface WidgetFinding extends CatalogFinding {
  check: WidgetCheckId;
}

/** The keys each object of the block may have; any other is refused. */
const FIELDS = {
  widget: ["title", "function", "size", "actions"],
  action: ["function", "confirm", "destructive"]
};

type Json = Record<string, unknown>;

function isObject(value: unknown): value is Json {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** An own key — `Object.keys`, so `__proto__` and `constructor` are not entries. */
function own(map: Json, key: string): unknown {
  return Object.keys(map).indexOf(key) >= 0 ? map[key] : undefined;
}

/** Characters as the server counts them: code points, not UTF-16 units. */
function chars(s: string): number {
  return Array.from(s).length;
}

class Findings {
  readonly list: WidgetFinding[] = [];
  error(check: WidgetCheckId, path: string, message: string): void {
    this.list.push({ level: "error", check, path, message });
  }
  /** Every key of `obj` not in `allowed`, each refused with `check`. */
  onlyKeys(check: WidgetCheckId, obj: Json, at: string, allowed: string[]): void {
    for (const key of Object.keys(obj)) {
      if (allowed.indexOf(key) >= 0) continue;
      this.error(
        check,
        `${at}.${key}`,
        `\`${at}.${key}\` is not a field; ${at} takes ${allowed.join(", ")}.`
      );
    }
  }
}

/** A declared function's name, or why it is not one. */
function functionProblem(value: unknown, functions: Json): "missing" | "undeclared" | null {
  if (typeof value !== "string" || !value.trim()) return "missing";
  return own(functions, value) === undefined ? "undeclared" : null;
}

function checkAction(
  widget: string,
  id: string,
  raw: unknown,
  functions: Json,
  out: Findings
): void {
  const at = `widgets.${widget}.actions.${id}`;
  const name = `Action ${JSON.stringify(id)} of widget ${JSON.stringify(widget)}`;
  if (!CATALOG_SEGMENT.test(id)) {
    out.error(
      "widget_action_invalid",
      at,
      `${name} needs an id of 1–32 lowercase letters, digits, "_" or "-", starting with a letter or digit.`
    );
    return;
  }
  if (!isObject(raw)) {
    out.error(
      "widget_action_invalid",
      at,
      `${name} must be an object { function, confirm?, destructive? }.`
    );
    return;
  }
  out.onlyKeys("widget_action_invalid", raw, at, FIELDS.action);
  for (const flag of ["confirm", "destructive"] as const) {
    if (raw[flag] !== undefined && typeof raw[flag] !== "boolean") {
      out.error(
        "widget_action_invalid",
        `${at}.${flag}`,
        `${name} has a \`${flag}\` that is not true or false.`
      );
    }
  }
  const problem = functionProblem(raw.function, functions);
  if (problem === "missing") {
    out.error("widget_action_invalid", `${at}.function`, `${name} needs a \`function\`.`);
  } else if (problem === "undeclared") {
    out.error(
      "widget_action_function_undeclared",
      `${at}.function`,
      `${name} calls the function ${JSON.stringify(raw.function)}, which this manifest does not declare under functions.`
    );
  }
}

function checkActions(widget: string, raw: unknown, functions: Json, out: Findings): void {
  if (raw === undefined) return;
  const at = `widgets.${widget}.actions`;
  if (!isObject(raw)) {
    out.error("widget_invalid", at, `\`${at}\` must be an object keyed by action id.`);
    return;
  }
  const ids = Object.keys(raw);
  if (ids.length > MAX_WIDGET_ACTIONS) {
    out.error(
      "widget_actions_too_many",
      at,
      `Widget ${JSON.stringify(widget)} declares ${ids.length} actions; at most ${MAX_WIDGET_ACTIONS} are allowed.`
    );
  }
  for (const id of ids) checkAction(widget, id, raw[id], functions, out);
}

function checkWidget(id: string, raw: unknown, functions: Json, out: Findings): void {
  const at = `widgets.${id}`;
  const name = `Widget ${JSON.stringify(id)}`;
  if (!CATALOG_SEGMENT.test(id)) {
    out.error(
      "widget_invalid",
      at,
      `${name} needs an id of 1–32 lowercase letters, digits, "_" or "-", starting with a letter or digit.`
    );
    return;
  }
  if (!isObject(raw)) {
    out.error(
      "widget_invalid",
      at,
      `\`${at}\` must be an object { title, function, size?, actions? }.`
    );
    return;
  }
  out.onlyKeys("widget_invalid", raw, at, FIELDS.widget);
  const title = raw.title;
  if (typeof title !== "string" || !title.trim() || chars(title) > MAX_WIDGET_TITLE_CHARS) {
    out.error(
      "widget_invalid",
      `${at}.title`,
      `${name} needs a \`title\` of 1–${MAX_WIDGET_TITLE_CHARS} characters.`
    );
  }
  const size = raw.size;
  if (size !== undefined && WIDGET_SIZES.indexOf(size as OxyAppWidgetSize) < 0) {
    out.error(
      "widget_invalid",
      `${at}.size`,
      `${name} has size ${JSON.stringify(size)}; expected one of ${WIDGET_SIZES.join(", ")}.`
    );
  }
  const problem = functionProblem(raw.function, functions);
  if (problem === "missing") {
    out.error(
      "widget_invalid",
      `${at}.function`,
      `${name} needs a \`function\` that returns its view.`
    );
  } else if (problem === "undeclared") {
    out.error(
      "widget_function_undeclared",
      `${at}.function`,
      `${name} calls the function ${JSON.stringify(raw.function)}, which this manifest does not declare under functions.`
    );
  }
  checkActions(id, raw.actions, functions, out);
}

/**
 * Everything wrong with a manifest's `widgets` block, in manifest order. An
 * empty list: nothing to say (an absent block included). Never throws,
 * whatever `manifest` is.
 */
export function checkWidgets(manifest: unknown): WidgetFinding[] {
  const out = new Findings();
  const m = isObject(manifest) ? manifest : {};
  const functions = isObject(m.functions) ? m.functions : {};
  const block = m.widgets;
  if (block === undefined) return out.list;
  if (!isObject(block)) {
    out.error("widget_invalid", "widgets", "`widgets` must be an object keyed by widget id.");
    return out.list;
  }
  const ids = Object.keys(block);
  if (ids.length > MAX_WIDGETS) {
    out.error(
      "widget_too_many",
      "widgets",
      `This app declares ${ids.length} widgets; at most ${MAX_WIDGETS} are allowed.`
    );
  }
  for (const id of ids) checkWidget(id, block[id], functions, out);
  return out.list;
}
