import type { BuildCheckNote, BuildCheckRef } from "@/types/appBuildChecks";

/**
 * A note as the console draws it, read off whatever the server stored. Notes are
 * written by a rule or by a model and kept as JSON, so a note with no `refs`, no
 * `created_at`, or no text at all must not break the list it sits in: the first two
 * get their empty value, the last is left out.
 */

type Json = Record<string, unknown>;
const record = (v: unknown): v is Json => typeof v === "object" && v !== null && !Array.isArray(v);
const text = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);

function readRef(raw: unknown): BuildCheckRef | null {
  if (!record(raw)) return null;
  const label = text(raw.label);
  return label ? { kind: text(raw.kind) ?? "", label, href: text(raw.href) } : null;
}

/** One note, or `null` when it says nothing. */
export function readNote(raw: unknown): BuildCheckNote | null {
  if (!record(raw)) return null;
  const body = text(raw.text);
  if (!body) return null;
  const refs = Array.isArray(raw.refs) ? raw.refs.map(readRef) : [];
  return {
    author: text(raw.author) ?? "rule",
    model: text(raw.model),
    text: body,
    refs: refs.filter((r): r is BuildCheckRef => r !== null),
    created_at: text(raw.created_at) ?? ""
  };
}

/** Every readable note of a list; anything else (`null`, an object) reads as none. */
export function readNotes(raw: unknown): BuildCheckNote[] {
  return (Array.isArray(raw) ? raw : [])
    .map(readNote)
    .filter((n): n is BuildCheckNote => n !== null);
}
