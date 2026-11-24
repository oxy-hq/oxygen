/**
 * The checks that need the workspace loaded, not just the file parsed.
 *
 * AUTOMATIONS: a port of the `garde` rules `Config::validate_workflow` runs on
 * an `Automation` (the `oxy` crate's config model and its validators) — what the
 * Rust `oxy validate` checked after the file deserialized. It follows garde's
 * `dive`s exactly, SKIPS INCLUDED, because the verdicts are pinned against the
 * Rust binary's: a `loop_sequential`'s tasks are checked, a `conditional`'s are
 * not (`conditions` carries `length(min = 1)` and no `dive`, and `else` is
 * `skip`), and `sql_file` is only checked for being non-empty — nothing wires
 * `validate_sql_file`.
 *
 * AGENTIC AGENTS: `databases:` entries and `llm.ref` must name a database and a
 * model in `config.yml`. The Rust command never checked this — its
 * `AgentConfig::from_file` is serde only — so this is STRICTER than Rust, on
 * purpose: at run time an unknown name is a warning and a `None`
 * (`workspace-runtime`'s `resolve_connector_impl` / `resolve_model_impl`), which
 * reaches the user as an agent that cannot query, not as an error.
 */

import type { Finding } from "./finding.js";
import { templateSyntaxError } from "./jinja.js";

/** The parts of a parsed `config.yml` references resolve against. */
export interface WorkspaceConfig {
  databases: Set<string>;
  models: Set<string>;
  /** Integration name → its `type`. */
  integrations: Map<string, string>;
}

type Doc = Record<string, unknown>;

function record(value: unknown): Doc | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Doc) : undefined;
}

function names(list: unknown): Set<string> {
  const out = new Set<string>();
  if (!Array.isArray(list)) return out;
  for (const item of list) {
    const name = record(item)?.name;
    if (typeof name === "string") out.add(name);
  }
  return out;
}

/** Read the reference targets out of a `config.yml` that passed its schema. */
export function workspaceConfig(parsed: unknown): WorkspaceConfig {
  const doc = record(parsed) ?? {};
  const integrations = new Map<string, string>();
  if (Array.isArray(doc.integrations)) {
    for (const item of doc.integrations) {
      const i = record(item);
      if (typeof i?.name === "string" && typeof i.type === "string")
        integrations.set(i.name, i.type);
    }
  }
  return { databases: names(doc.databases), models: names(doc.models), integrations };
}

/** Export formats each task kind allows (`validate_task`). */
const EXPORT_FORMATS: Record<string, { label: string; formats: string[] }> = {
  agent: { label: "agent", formats: ["json", "csv", "sql"] },
  execute_sql: { label: "ExecuteSQL", formats: ["json", "csv", "sql"] },
  formatter: { label: "Formatter", formats: ["txt", "docx"] },
  semantic_query: { label: "SemanticQuery", formats: ["json", "csv", "sql"] },
  omni_query: { label: "OmniQuery", formats: ["json", "csv", "sql"] },
  looker_query: { label: "LookerQuery", formats: ["json", "csv", "sql"] }
};

class Collector {
  readonly findings: Finding[] = [];
  constructor(
    private readonly file: string,
    private readonly config: WorkspaceConfig | undefined
  ) {}

  rule(path: string, message: string): void {
    this.findings.push({ file: this.file, path, kind: "rule", message });
  }

  ref(path: string, message: string): void {
    this.findings.push({ file: this.file, path, kind: "ref", message });
  }

  /** `garde(length(min = 1))` on a string, or on a list. */
  nonEmpty(doc: Doc, key: string, path: string): void {
    const value = doc[key];
    if ((typeof value === "string" || Array.isArray(value)) && value.length === 0) {
      this.rule(path, "length is lower than 1");
    }
  }

  /** `validate_consistency_prompt`. */
  template(doc: Doc, key: string, path: string): void {
    const value = doc[key];
    if (typeof value !== "string") return;
    const error = templateSyntaxError(value);
    if (error) this.rule(path, `Invalid consistency prompt template syntax: ${error}`);
  }

  /** `validate_database_exists`; skipped (not passed) without a loaded config. */
  database(name: unknown, path: string): void {
    if (!this.config || typeof name !== "string") return;
    if (!this.config.databases.has(name)) this.ref(path, `Database not found (${name})`);
  }

  /** `validate_omni_integration_exists` / `validate_looker_integration_exists`. */
  integration(name: unknown, type: "omni" | "looker", path: string): void {
    if (!this.config || typeof name !== "string") return;
    if (this.config.integrations.get(name) === type) return;
    this.ref(
      path,
      `${type === "looker" ? "Looker integration" : "Integration"} not found (${name})`
    );
  }

  model(name: unknown, path: string): void {
    if (!this.config || typeof name !== "string") return;
    if (!this.config.models.has(name)) this.ref(path, `Model not found in config.yml (${name})`);
  }
}

/** The task kinds with an `export:` field; on the others serde ignores the key. */
const HAS_EXPORT = new Set([...Object.keys(EXPORT_FORMATS), "workflow"]);

function checkExport(c: Collector, task: Doc, type: string, path: string): void {
  const exp = record(task.export);
  if (!exp || !HAS_EXPORT.has(type)) return;
  c.nonEmpty(exp, "path", `${path}.export.path`);
  const allowed = EXPORT_FORMATS[type];
  if (allowed && typeof exp.format === "string" && !allowed.formats.includes(exp.format)) {
    c.rule(
      `${path}.export.format`,
      `Invalid export format: ${exp.format}, only supports ${allowed.formats.join(", ")} for ${allowed.label} task`
    );
  }
}

type Rules = (c: Collector, task: Doc, path: string) => void;

function checkHttp(c: Collector, task: Doc, path: string): void {
  c.nonEmpty(task, "url", `${path}.url`);
  const timeout = task.timeout_secs;
  if (typeof timeout === "number" && timeout < 1) c.rule(`${path}.timeout_secs`, "lower than 1");
  if (typeof timeout === "number" && timeout > 120)
    c.rule(`${path}.timeout_secs`, "greater than 120");
  const persist = record(task.persist_to_secret);
  if (!persist) return;
  c.nonEmpty(persist, "from", `${path}.persist_to_secret.from`);
  c.nonEmpty(persist, "name", `${path}.persist_to_secret.name`);
}

/**
 * Each task kind's rules. `workflow`, `semantic_query` and an unknown `type:`
 * (serde's `other`) carry none beyond the export one.
 */
const TASK_RULES: Record<string, Rules> = {
  agent: (c, task, path) => {
    c.nonEmpty(task, "prompt", `${path}.prompt`);
    c.template(task, "consistency_prompt", `${path}.consistency_prompt`);
  },
  execute_sql: (c, task, path) => {
    c.database(task.database, `${path}.database`);
    c.nonEmpty(task, "sql_file", `${path}.sql_file`);
    c.nonEmpty(task, "sql_query", `${path}.sql_query`);
    if (typeof task.dry_run_limit === "number" && task.dry_run_limit < 1) {
      c.rule(`${path}.dry_run_limit`, "lower than 1");
    }
  },
  omni_query: (c, task, path) => {
    c.integration(task.integration, "omni", `${path}.integration`);
    c.nonEmpty(task, "topic", `${path}.topic`);
  },
  looker_query: (c, task, path) => {
    c.integration(task.integration, "looker", `${path}.integration`);
    c.nonEmpty(task, "model", `${path}.model`);
    c.nonEmpty(task, "explore", `${path}.explore`);
  },
  loop_sequential: (c, task, path) => checkTasks(c, task.tasks, `${path}.tasks`),
  formatter: (c, task, path) => c.nonEmpty(task, "template", `${path}.template`),
  // `length(min = 1)` and NO `dive`: the condition's tasks are not checked.
  conditional: (c, task, path) => c.nonEmpty(task, "conditions", `${path}.conditions`),
  http_request: checkHttp,
  airway: (c, task, path) => c.nonEmpty(task, "pipeline", `${path}.pipeline`)
};

function checkTasks(c: Collector, tasks: unknown, path: string): void {
  if (!Array.isArray(tasks)) return;
  tasks.forEach((item, i) => {
    const task = record(item);
    if (!task) return;
    const at = `${path}.${i}`;
    c.nonEmpty(task, "name", `${at}.name`);
    const type = typeof task.type === "string" ? task.type : "";
    checkExport(c, task, type, at);
    TASK_RULES[type]?.(c, task, at);
    const cache = record(task.cache);
    if (cache) c.nonEmpty(cache, "path", `${at}.cache.path`);
  });
}

/**
 * The garde rules on an automation that passed its schema. Without `config`
 * the reference checks are skipped, not passed: the caller reports why.
 */
export function automationFindings(
  file: string,
  automation: unknown,
  config: WorkspaceConfig | undefined
): Finding[] {
  const doc = record(automation);
  if (!doc) return [];
  const c = new Collector(file, config);
  c.nonEmpty(doc, "tasks", "tasks");
  c.template(doc, "consistency_prompt", "consistency_prompt");
  checkTasks(c, doc.tasks, "tasks");
  return c.findings;
}

/** An agentic agent's `databases:` and `llm.ref`, against `config.yml`. */
export function agenticFindings(
  file: string,
  agent: unknown,
  config: WorkspaceConfig | undefined
): Finding[] {
  const doc = record(agent);
  if (!doc) return [];
  const c = new Collector(file, config);
  if (Array.isArray(doc.databases)) {
    doc.databases.forEach((name, i) => {
      c.database(name, `databases.${i}`);
    });
  }
  c.model(record(doc.llm)?.ref, "llm.ref");
  return c.findings;
}
