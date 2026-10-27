/**
 * The one expression shape `lever.read_back` and trigger guardrails take
 * (crates/experiment/src/expr.rs): `{{view.measure}} <op> <number>`.
 */
export type Operator = ">" | ">=" | "<" | "<=" | "==" | "!=";

export const OPERATORS: { value: Operator; label: string }[] = [
  { value: ">", label: "> is above" },
  { value: ">=", label: ">= is at least" },
  { value: "<", label: "< is below" },
  { value: "<=", label: "<= is at most" },
  { value: "==", label: "== equals" },
  { value: "!=", label: "!= differs from" }
];

export interface Condition {
  measure: string;
  /** A trigger guardrail may compare `measure / denominator`; empty or absent = one measure. */
  denominator?: string;
  op: Operator;
  value: string;
}

const NUMBER = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/;
// Two-character operators first, or `>=` would read as `>` followed by `=1`.
const SHAPE = /^\{\{\s*([\p{L}\p{N}_]+\.[\p{L}\p{N}_]+)\s*\}\}\s*(>=|<=|==|!=|>|<)\s*(\S+)\s*$/u;
const RATIO =
  /^\{\{\s*([\p{L}\p{N}_]+\.[\p{L}\p{N}_]+)\s*\}\}\s*\/\s*\{\{\s*([\p{L}\p{N}_]+\.[\p{L}\p{N}_]+)\s*\}\}\s*(>=|<=|==|!=|>|<)\s*(\S+)\s*$/u;

const isNumber = (s: string) => NUMBER.test(s) && Number.isFinite(Number(s));

/**
 * The text as a condition, or null when the server would not read it as one.
 * `ratio` also reads `{{a}} / {{b}} <op> <number>`, which only a trigger guardrail takes.
 */
export function parseCondition(text: string, opts: { ratio?: boolean } = {}): Condition | null {
  const t = text.trim();
  const m = SHAPE.exec(t);
  if (m) return isNumber(m[3]) ? { measure: m[1], op: m[2] as Operator, value: m[3] } : null;
  const r = opts.ratio ? RATIO.exec(t) : null;
  if (!r || !isNumber(r[4])) return null;
  return { measure: r[1], denominator: r[2], op: r[3] as Operator, value: r[4] };
}

export const composeCondition = (c: Condition) =>
  c.denominator?.trim()
    ? `{{${c.measure}}} / {{${c.denominator.trim()}}} ${c.op} ${c.value}`
    : `{{${c.measure}}} ${c.op} ${c.value}`;
