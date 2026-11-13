/** Languages the views highlight; anything else renders as plain text. */
const KNOWN = new Set([
  "sql",
  "yaml",
  "json",
  "markdown",
  "typescript",
  "javascript",
  "tsx",
  "jsx",
  "python",
  "bash",
  "css",
  "html",
  "toml"
]);

const ALIASES: Record<string, string> = {
  yml: "yaml",
  md: "markdown",
  ts: "typescript",
  js: "javascript",
  sh: "bash",
  py: "python"
};

/** The Prism language for a language name, or `text`. */
export function prismLanguage(language?: string): string {
  const name = (language ?? "").toLowerCase();
  const resolved = ALIASES[name] ?? name;
  return KNOWN.has(resolved) ? resolved : "text";
}
