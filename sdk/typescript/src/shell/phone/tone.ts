/** Status colour of a row, card or pill. `warn` has no shell token of its own;
 *  shell.css mixes it from the destructive and success tokens. */
export type PhoneTone = "good" | "warn" | "bad" | "neutral";

/** The class that sets `--oxy-phone-tone` for an element and its children. */
export function toneClass(tone: PhoneTone | undefined): string | undefined {
  return tone ? `oxy-phone-tone--${tone}` : undefined;
}

/** Whether an optional slot has something to show. `0` does (a count of
 *  zero is content); `undefined`, `null`, `false` and `""` do not. */
export function present(node: unknown): boolean {
  return node !== undefined && node !== null && node !== false && node !== "";
}
