/**
 * One problem `oxyc validate` found, and its KIND.
 *
 * The kind is the class of error the Rust `oxy validate` reported for the same
 * file, kept so a caller can tell "this file does not load" from "this file
 * names a database the workspace does not have" without parsing prose:
 *
 * - `parse`   the file does not load: YAML syntax, or a shape the runtime's
 *             deserializer rejects (Rust: "Couldn't parse", "Parsing error").
 * - `ref`     it names something the workspace's `config.yml` does not define:
 *             a database, an integration, a model (Rust: "Database not found").
 * - `rule`    it loads, and breaks a rule the runtime checks after loading: an
 *             empty `tasks:`, a timeout out of range, a bad variable expression
 *             (Rust: "Invalid automation", "Validation error").
 * - `missing` the workspace has no `config.yml` to read references from (Rust:
 *             "Could not find config.yml").
 *
 * Pinned by the parity corpus (`validate-parity.test.ts`).
 */
export type FindingKind = "parse" | "ref" | "rule" | "missing";

export interface Finding {
  file: string;
  path: string;
  kind: FindingKind;
  message: string;
}

/** A `parse` finding: the file does not load. */
export function parseFinding(file: string, path: string, message: string): Finding {
  return { file, path, kind: "parse", message };
}

/** A `rule` finding: it loads, and breaks a rule. */
export function ruleFinding(file: string, path: string, message: string): Finding {
  return { file, path, kind: "rule", message };
}
