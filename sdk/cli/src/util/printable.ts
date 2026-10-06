/**
 * Text a deployment supplied, made safe to print.
 *
 * A terminal acts on control characters: an escape sequence in a token's name
 * or an error body can clear the screen, move the cursor or rewrite a line
 * already printed. Whatever a deployment sent goes through one of these before
 * it reaches stderr or a tool result.
 */

/** `text` on one line: every control character removed, line breaks included. */
export function printable(text: string): string {
  // biome-ignore lint/suspicious/noControlCharactersInRegex: removing them is the point
  return text.replace(/[\u0000-\u001f\u007f-\u009f]/g, "");
}

/** `text` with its line breaks kept and every other control character removed. */
export function printableLines(text: string): string {
  return text.split(/\r?\n/).map(printable).join("\n");
}
