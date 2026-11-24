/**
 * The human rendering of an agentic run's events — the Rust
 * `oxy agentic run`'s `render_event_pretty`, event for event. Each renderer
 * returns exactly what to write to stdout, newline included (a `text_delta`
 * token is written as it comes, with none).
 */

import { out } from "../ui/tty.js";
import { printable, printableLines } from "../util/printable.js";

type Payload = Record<string, unknown>;
type Renderer = (p: Payload, runId: string) => string;

const str = (v: unknown, fallback = "?") => (typeof v === "string" ? printable(v) : fallback);

/** `s` cut to 60 characters, as `…[..57]` + `...` was in Rust. */
export function truncate(s: string, max = 60): string {
  const chars = Array.from(s);
  return chars.length > max ? `${chars.slice(0, max - 3).join("")}...` : s;
}

function stepStart(p: Payload): string {
  const label = out.cyan(str(p.label));
  const summary = str(p.summary, "");
  return summary ? `⠋ ${label} — ${summary}\n` : `⠋ ${label}\n`;
}

function stepEnd(p: Payload): string {
  const label = str(p.label);
  return p.outcome === "advanced"
    ? `${out.green(`✓ ${label}`)}\n`
    : `${out.yellow(`✗ ${label}`)}\n`;
}

function awaitingInput(p: Payload, runId: string): string {
  const questions = Array.isArray(p.questions) ? (p.questions as Payload[]) : [];
  const lines = questions.map((q) => out.yellow(`? ${str(q?.prompt)}`));
  const hint = `  (Use \`oxyc agentic answer ${runId} --answer "..."\` to respond)`;
  return `\n${[...lines, hint].join("\n")}\n`;
}

const RENDERERS: Record<string, Renderer> = {
  step_start: stepStart,
  step_end: stepEnd,
  tool_call: (p) => `  → ${out.gray(str(p.name))}(${truncate(str(p.input, ""))})\n`,
  tool_result: (p) => `  ← ${out.gray(str(p.name))} → ${truncate(str(p.output, ""))}\n`,
  // The answer streams token by token; its line breaks are its own, every
  // other control character is dropped.
  text_delta: (p) => (typeof p.token === "string" ? printableLines(p.token) : ""),
  thinking_token: () => "",
  done: () => `\n${out.green("✓ Done")}\n`,
  error: (p) => `\n${out.red(`✗ Error: ${str(p.message, "unknown error")}`)}\n`,
  awaiting_input: awaitingInput,
  tool_used: (p) => `  🔧 ${out.gray(str(p.tool_name))} — ${str(p.summary, "")}\n`,
  file_change_pending: (p) => `  📝 ${out.cyan(str(p.file_path))} — ${str(p.description, "")}\n`
};

/** What to write for one event; any event without a renderer shows its type. */
export function renderEvent(eventType: string, payload: Payload, runId: string): string {
  const render = RENDERERS[eventType];
  return render ? render(payload, runId) : `  [${printable(eventType)}]\n`;
}
