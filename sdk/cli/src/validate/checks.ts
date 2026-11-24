/**
 * Everything `oxyc validate` checks in a file once it has read it, past the
 * schema itself: the semantic-file checks, the empty-document rule, and the
 * references and rules that need the workspace's `config.yml`.
 */

import type { Finding } from "./finding.js";
import { agenticFindings, automationFindings, workspaceConfig } from "./references.js";
import { legacyGlobalsFinding, topicVariableFindings, viewVariableFindings } from "./semantics.js";
import { isConfig, type References } from "./workspace.js";

const SEMANTIC = new Set(["view.json", "topic.json"]);

/**
 * The check that runs on the raw text, BEFORE the YAML parse, as
 * `reject_legacy_globals` does: the removed features parse fine and silently
 * mean nothing.
 */
export function beforeParse(schema: string, rel: string, raw: string): Finding[] | undefined {
  if (!SEMANTIC.has(schema)) return undefined;
  const legacy = legacyGlobalsFinding(rel, raw);
  return legacy ? [legacy] : undefined;
}

export interface Parsed {
  rel: string;
  schema: string;
  /** The YAML document; `null` / `undefined` for an empty file. */
  doc: unknown;
  /** The schema's findings for a document, `[]` when it conforms. */
  shape: (doc: unknown) => Finding[];
  refs: References;
  notes: Set<string>;
}

/** Every finding for a parsed document. */
export function afterParse(p: Parsed): Finding[] {
  // AN EMPTY DOCUMENT IS AN EMPTY MAP to serde, so it is valid wherever every
  // field is optional: Rust loads an empty `config.yml`. Where `{}` is not
  // enough, say what the file is rather than which field `{}` lacks.
  const empty = p.doc === null || p.doc === undefined;
  const doc = empty ? {} : p.doc;
  const shape = p.shape(doc);
  if (empty && shape.length > 0) {
    return [{ file: p.rel, path: "(root)", kind: "parse", message: "the file is empty" }];
  }
  if (isConfig(p.rel))
    p.refs.loaded.set(p.rel, shape.length > 0 ? undefined : workspaceConfig(doc));
  const semantic = semanticFindings(p.schema, p.rel, doc);
  return shape.length > 0 ? [...semantic, ...shape] : [...semantic, ...referenceFindings(p, doc)];
}

function semanticFindings(schema: string, rel: string, doc: unknown): Finding[] {
  if (schema === "view.json") return viewVariableFindings(rel, doc);
  if (schema === "topic.json") return topicVariableFindings(rel, doc);
  return [];
}

/** The checks that need the workspace: against the config that owns the file. */
function referenceFindings(p: Parsed, doc: unknown): Finding[] {
  if (p.schema !== "workflow.json" && p.schema !== "agentic.json") return [];
  const owner = p.refs.ownerOf(p.rel);
  const config = owner === undefined ? undefined : p.refs.loaded.get(owner);
  if (owner !== undefined && config === undefined) {
    p.notes.add(`${p.rel}: references not checked — ${owner} does not load (see its findings)`);
  }
  return p.schema === "workflow.json"
    ? automationFindings(p.rel, doc, config)
    : agenticFindings(p.rel, doc, config);
}
