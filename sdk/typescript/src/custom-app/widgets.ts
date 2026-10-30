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

/** How wide a widget's card is on the home: one column, two, or three (the row). */
export type OxyAppWidgetSize = "regular" | "wide" | "full";

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

/** The type of one widget setting. */
export type WidgetParamType = "string" | "integer" | "number" | "boolean";

/** A setting's value, as a layout gives it and a widget function receives it. */
export type WidgetParamValue = string | number | boolean;

/** One setting a layout gives a copy of a widget: a flat JSON Schema property. */
export interface WidgetParamSchema {
  type: WidgetParamType;
  /** What the setting is called, for a person or an agent filling it in. */
  title?: string;
  /** What to put in it (`"A store slug"`). */
  description?: string;
  /** Used when a layout leaves the setting out; matches `type` (and `enum`). */
  default?: WidgetParamValue;
  /** The values it may take: non-empty, each of `type`. */
  enum?: WidgetParamValue[];
  /** Numbers only. */
  minimum?: number;
  /** Numbers only. */
  maximum?: number;
  /** Strings only. */
  minLength?: number;
  /** Strings only. */
  maxLength?: number;
}

/**
 * A widget's settings: a small JSON Schema, an object of flat properties (at
 * most 8). No nesting and no `$ref`.
 */
export interface WidgetParamsSchema {
  type: "object";
  properties: Record<string, WidgetParamSchema>;
  /** Properties a layout must set; each names one declared in `properties`. */
  required?: string[];
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
   * A function this app declares. The home calls it with a
   * {@link WidgetInput} (`{ widget: "<id>", size, params }`) and expects a
   * {@link WidgetView} back.
   */
  function: string;
  /** The default size: `regular` (one column, default), `wide` (two) or `full` (three). */
  size?: OxyAppWidgetSize;
  /**
   * The sizes a home layout may choose for this widget. Non-empty, distinct,
   * and includes `size`. Default: just `size`.
   */
  sizes?: OxyAppWidgetSize[];
  /**
   * The settings a layout gives each copy of this widget (which store, which
   * vendor): a small JSON Schema of at most 8 flat properties. The function
   * receives them as `params`, with defaults filled in.
   */
  params?: WidgetParamsSchema;
  /**
   * The buttons this widget may run, keyed by action id (1–32 lowercase
   * letters, digits, `_` or `-`). At most 3. A button in a view naming an
   * action missing here is dropped.
   */
  actions?: Record<string, OxyAppWidgetAction>;
}

// ── The view a widget function returns ──────────────────────────────────────

/**
 * What the home calls a widget function with: the widget, the size it is
 * drawn at, and this copy's settings.
 */
export interface WidgetInput {
  widget: string;
  /** The size the card is drawn at, so a view can say more when it has the room. */
  size: OxyAppWidgetSize;
  /**
   * This copy's settings: the layout's `params` with the schema's defaults
   * filled in; `{}` when the widget declares none.
   */
  params: Record<string, WidgetParamValue>;
}

/** Which way a {@link WidgetMetric} is moving. */
export type WidgetTrend = "up" | "down" | "flat";

/** A KPI: the one number that matters, with its context. */
export interface WidgetMetric {
  /** The number as it should read, `"$48,210"`; ≤ 24 characters. */
  value: string;
  /** What it counts, ≤ 40 characters. */
  label?: string;
  /** The change, `"+12% vs August"`; ≤ 24 characters. */
  delta?: string;
  trend?: WidgetTrend;
  /** Up to 30 points, drawn as a sparkline. */
  spark?: number[];
}

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
  /** A KPI card: the one number that matters. */
  metric?: WidgetMetric;
  /** At most 8. */
  rows?: WidgetRow[];
  /** At most 2. */
  buttons?: WidgetButton[];
  /** Where "Open" goes: app-relative (`/` or `?`). */
  link?: string;
  /**
   * Read the widget again after this many seconds: an integer from
   * {@link MIN_WIDGET_REFRESH_AFTER_SECONDS} to
   * {@link MAX_WIDGET_REFRESH_AFTER_SECONDS}.
   */
  refresh_after?: number;
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
export const MAX_WIDGETS = 6;
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
export const MAX_WIDGET_ROWS = 8;
/** A metric's `value`, in characters. */
export const MAX_WIDGET_METRIC_VALUE_CHARS = 24;
/** A metric's `label`, in characters. */
export const MAX_WIDGET_METRIC_LABEL_CHARS = 40;
/** A metric's `delta`, in characters. */
export const MAX_WIDGET_METRIC_DELTA_CHARS = 24;
/** Points in a metric's `spark`, at most. */
export const MAX_WIDGET_METRIC_SPARK_POINTS = 30;
/** Buttons in a view, at most. */
export const MAX_WIDGET_BUTTONS = 2;
/** A button's `label`, in characters. */
export const MAX_WIDGET_BUTTON_LABEL_CHARS = 32;
/** A button's `confirm` text, in characters. */
export const MAX_WIDGET_BUTTON_CONFIRM_CHARS = 160;
/** A button's `input`, serialized, in bytes. */
export const MAX_WIDGET_BUTTON_INPUT_BYTES = 4096;
/** Settings on one widget, at most. */
export const MAX_WIDGET_PARAMS = 8;
/** A view's `refresh_after`, at least, in seconds. */
export const MIN_WIDGET_REFRESH_AFTER_SECONDS = 60;
/** A view's `refresh_after`, at most, in seconds (a day). */
export const MAX_WIDGET_REFRESH_AFTER_SECONDS = 86400;
/** Widget sizes, in the order the home lists them. */
export const WIDGET_SIZES: readonly OxyAppWidgetSize[] = ["regular", "wide", "full"];

// ── The publish checks ──────────────────────────────────────────────────────

/** The server's check ids for a bad `widgets` block. */
export type WidgetCheckId =
  | "widget_invalid"
  | "widget_size_invalid"
  | "widget_too_many"
  | "widget_function_undeclared"
  | "widget_actions_too_many"
  | "widget_action_invalid"
  | "widget_action_function_undeclared"
  | "widget_params_invalid";

/** A {@link CatalogFinding} that also says which publish check it is. */
export interface WidgetFinding extends CatalogFinding {
  check: WidgetCheckId;
}

/** The keys each object of the block may have; any other is refused. */
const FIELDS = {
  widget: ["title", "function", "size", "sizes", "params", "actions"],
  params: ["type", "properties", "required"],
  param: [
    "type",
    "title",
    "description",
    "default",
    "enum",
    "minimum",
    "maximum",
    "minLength",
    "maxLength"
  ],
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

function isSize(value: unknown): value is OxyAppWidgetSize {
  return WIDGET_SIZES.indexOf(value as OxyAppWidgetSize) >= 0;
}

/** `size` and `sizes`: each a known size, `sizes` non-empty, distinct and holding `size`. */
function checkSizes(at: string, name: string, raw: Json, out: Findings): void {
  const allowed = WIDGET_SIZES.join(", ");
  const size = raw.size;
  const sizeOk = size === undefined || isSize(size);
  if (!sizeOk) {
    out.error(
      "widget_size_invalid",
      `${at}.size`,
      `${name} has size ${JSON.stringify(size)}; expected one of ${allowed}.`
    );
  }
  const sizes = raw.sizes;
  if (sizes === undefined) return;
  if (!Array.isArray(sizes) || sizes.length === 0) {
    out.error(
      "widget_size_invalid",
      `${at}.sizes`,
      `${name} needs \`sizes\` to be a non-empty list of: ${allowed}.`
    );
    return;
  }
  let clean = true;
  sizes.forEach((entry, i) => {
    if (!isSize(entry)) {
      clean = false;
      out.error(
        "widget_size_invalid",
        `${at}.sizes[${i}]`,
        `${name} lists size ${JSON.stringify(entry)} in \`sizes\`; expected one of ${allowed}.`
      );
    } else if (sizes.indexOf(entry) !== i) {
      clean = false;
      out.error(
        "widget_size_invalid",
        `${at}.sizes[${i}]`,
        `${name} lists size ${JSON.stringify(entry)} in \`sizes\` more than once.`
      );
    }
  });
  const dflt = size === undefined ? "regular" : size;
  if (clean && sizeOk && sizes.indexOf(dflt) < 0) {
    out.error(
      "widget_size_invalid",
      `${at}.sizes`,
      `${name} has \`sizes\` that leave out its size ${JSON.stringify(dflt)}; add it or change \`size\`.`
    );
  }
}

const PARAM_TYPES: readonly WidgetParamType[] = ["string", "integer", "number", "boolean"];

function isParamType(value: unknown): value is WidgetParamType {
  return PARAM_TYPES.indexOf(value as WidgetParamType) >= 0;
}

/** Whether `value` is a legal value of a setting of `type`. */
function fitsType(type: WidgetParamType, value: unknown): boolean {
  switch (type) {
    case "string":
      return typeof value === "string";
    case "boolean":
      return typeof value === "boolean";
    case "integer":
      return typeof value === "number" && Number.isInteger(value);
    default:
      return typeof value === "number" && Number.isFinite(value);
  }
}

/** `minimum`/`maximum` on numbers, `minLength`/`maxLength` on strings: right type, right place. */
function checkBounds(at: string, type: WidgetParamType, raw: Json, out: Findings): void {
  const numeric = type === "integer" || type === "number";
  const bounds: Array<[string, boolean]> = [
    ["minimum", numeric],
    ["maximum", numeric],
    ["minLength", type === "string"],
    ["maxLength", type === "string"]
  ];
  for (const [key, applies] of bounds) {
    const value = raw[key];
    if (value === undefined) continue;
    const lengths = key.endsWith("Length");
    if (!applies) {
      out.error(
        "widget_params_invalid",
        `${at}.${key}`,
        `\`${at}.${key}\` only applies to ${lengths ? "string" : "integer and number"} settings, and this one is ${type}.`
      );
    } else if (
      typeof value !== "number" ||
      !Number.isFinite(value) ||
      (lengths && (!Number.isInteger(value) || value < 0))
    ) {
      out.error(
        "widget_params_invalid",
        `${at}.${key}`,
        `\`${at}.${key}\` must be ${lengths ? "a whole number, 0 or more" : "a number"}.`
      );
    }
  }
}

/** `enum` and `default`: each matches the setting's type, and a default is one of the enum. */
function checkChoices(at: string, type: WidgetParamType, raw: Json, out: Findings): void {
  const list = raw.enum;
  let choices: unknown[] | undefined;
  if (list !== undefined) {
    if (!Array.isArray(list) || list.length === 0) {
      out.error("widget_params_invalid", `${at}.enum`, `\`${at}.enum\` must be a non-empty list.`);
    } else {
      choices = list;
      list.forEach((entry, i) => {
        if (fitsType(type, entry)) return;
        choices = undefined;
        out.error(
          "widget_params_invalid",
          `${at}.enum[${i}]`,
          `\`${at}.enum[${i}]\` is ${JSON.stringify(entry)}, which is not a ${type}.`
        );
      });
    }
  }
  const dflt = raw.default;
  if (dflt === undefined) return;
  if (!fitsType(type, dflt)) {
    out.error(
      "widget_params_invalid",
      `${at}.default`,
      `\`${at}.default\` is ${JSON.stringify(dflt)}, which is not a ${type}.`
    );
  } else if (choices && choices.indexOf(dflt) < 0) {
    out.error(
      "widget_params_invalid",
      `${at}.default`,
      `\`${at}.default\` ${JSON.stringify(dflt)} is not one of its \`enum\` values.`
    );
  }
}

function checkParam(widget: string, name: string, raw: unknown, out: Findings): void {
  const at = `widgets.${widget}.params.properties.${name}`;
  if (!CATALOG_SEGMENT.test(name)) {
    out.error(
      "widget_params_invalid",
      at,
      `Setting ${JSON.stringify(name)} of widget ${JSON.stringify(widget)} needs a name of 1–32 lowercase letters, digits, "_" or "-", starting with a letter or digit.`
    );
    return;
  }
  if (!isObject(raw)) {
    out.error("widget_params_invalid", at, `\`${at}\` must be an object { type, ... }.`);
    return;
  }
  out.onlyKeys("widget_params_invalid", raw, at, FIELDS.param);
  for (const key of ["title", "description"]) {
    if (raw[key] !== undefined && typeof raw[key] !== "string") {
      out.error("widget_params_invalid", `${at}.${key}`, `\`${at}.${key}\` must be text.`);
    }
  }
  const type = raw.type;
  if (!isParamType(type)) {
    out.error(
      "widget_params_invalid",
      `${at}.type`,
      `\`${at}.type\` is ${JSON.stringify(type)}; expected one of ${PARAM_TYPES.join(", ")}.`
    );
    return;
  }
  checkBounds(at, type, raw, out);
  checkChoices(at, type, raw, out);
}

/** `required`: a list of declared property names. */
function checkRequired(at: string, raw: Json, props: Json | undefined, out: Findings): void {
  const required = raw.required;
  if (required === undefined) return;
  if (!Array.isArray(required)) {
    out.error(
      "widget_params_invalid",
      `${at}.required`,
      `\`${at}.required\` must be a list of setting names.`
    );
    return;
  }
  required.forEach((entry, i) => {
    if (typeof entry !== "string") {
      out.error(
        "widget_params_invalid",
        `${at}.required[${i}]`,
        `\`${at}.required[${i}]\` must be a setting name.`
      );
    } else if (props && own(props, entry) === undefined) {
      out.error(
        "widget_params_invalid",
        `${at}.required[${i}]`,
        `\`${at}.required[${i}]\` names ${JSON.stringify(entry)}, which is not declared under \`properties\`.`
      );
    }
  });
}

/** A widget's `params`: a flat object schema of at most {@link MAX_WIDGET_PARAMS} settings. */
function checkParams(widget: string, raw: unknown, out: Findings): void {
  if (raw === undefined) return;
  const at = `widgets.${widget}.params`;
  if (!isObject(raw)) {
    out.error(
      "widget_params_invalid",
      at,
      `\`${at}\` must be an object { type: "object", properties, required? }.`
    );
    return;
  }
  out.onlyKeys("widget_params_invalid", raw, at, FIELDS.params);
  if (raw.type !== "object") {
    out.error("widget_params_invalid", `${at}.type`, `\`${at}.type\` must be "object".`);
  }
  const props = raw.properties;
  if (!isObject(props)) {
    out.error(
      "widget_params_invalid",
      `${at}.properties`,
      `\`${at}.properties\` must be an object keyed by setting name.`
    );
    checkRequired(at, raw, undefined, out);
    return;
  }
  const names = Object.keys(props);
  if (names.length > MAX_WIDGET_PARAMS) {
    out.error(
      "widget_params_invalid",
      `${at}.properties`,
      `Widget ${JSON.stringify(widget)} declares ${names.length} settings; at most ${MAX_WIDGET_PARAMS} are allowed.`
    );
  }
  for (const name of names) checkParam(widget, name, props[name], out);
  checkRequired(at, raw, props, out);
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
      `\`${at}\` must be an object { title, function, size?, sizes?, params?, actions? }.`
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
  checkSizes(at, name, raw, out);
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
  checkParams(id, raw.params, out);
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
