/**
 * Where a generated schema and the runtime's deserializer disagree, the
 * deserializer is right, and these patches say so.
 *
 * `json-schemas/*.json` are `schemars` output for the Rust types, and
 * `schemars` cannot see everything `serde` does: a hand-written `Deserialize`,
 * an `alias`, `deny_unknown_fields` beside a `flatten`, a `default` on a
 * `skip_serializing` field. Each gap made `oxyc validate` reject a file the
 * Rust `oxy validate` accepted, or accept one it rejected, and each is closed
 * here rather than in the committed schema, because the committed schema is
 * compared against `schema_for!()` by `json_schemas_are_current` and must stay
 * exactly what the types generate.
 *
 * EVERY PATCH CHECKS ITS TARGET. A patch whose target is not shaped the way it
 * expects is reported, not skipped in silence: the Rust type changed, and the
 * patch may now be wrong or unnecessary. `serde-leniency.test.ts` applies every
 * patch to the committed schemas and fails on any report, so a drift is a red
 * test in the PR that caused it.
 *
 * Each patch is pinned by a case in the parity corpus whose verdict was
 * recorded from the Rust binary (`fixtures/validate-parity/`).
 */

type Json = Record<string, unknown>;

interface Patch {
  /** The schema file it applies to. */
  schema: string;
  /** What `serde` does that the schema does not say. */
  why: string;
  /** Apply it; return a reason when the target is not where it should be. */
  apply: (doc: Json) => string | undefined;
}

function obj(value: unknown): Json | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Json) : undefined;
}

function definition(doc: Json, name: string): Json | undefined {
  return obj(obj(doc.definitions)?.[name]);
}

const PATCHES: Patch[] = [
  {
    schema: "workflow.json",
    why:
      "`Automation` is `deny_unknown_fields`, which `schemars` drops beside its " +
      "`#[serde(flatten)] variables`; and `name:` is accepted (then replaced by the " +
      "file name) though `#[schemars(skip)]` hides it. Rust: `unknown field `tests``.",
    apply: (doc) => {
      const props = obj(doc.properties);
      if (!props?.tasks || "additionalProperties" in doc) return "root is not the open Automation";
      props.name = {};
      doc.additionalProperties = false;
      return undefined;
    }
  },
  {
    schema: "workflow.json",
    why:
      "`variables:` values go through a hand-written visitor that takes a bare " +
      'default (`month: "2024-01"`, `n: 3`) as well as a schema object.',
    apply: (doc) => {
      const variables = obj(obj(doc.properties)?.variables);
      if (!variables || !obj(variables.additionalProperties)) {
        return "variables has no additionalProperties schema";
      }
      variables.additionalProperties = true;
      return undefined;
    }
  },
  {
    schema: "workflow.json",
    why:
      "`TaskType::Unknown` is `#[serde(other)]`: any `type:` the runtime does not " +
      "know loads as an unknown step. `schemars` renders it as the literal tag `Unknown`.",
    apply: (doc) => {
      const branches = definition(doc, "Task")?.oneOf;
      if (!Array.isArray(branches)) return "Task has no oneOf";
      const tagOf = (b: unknown) => {
        const values = obj(obj(obj(b)?.properties)?.type)?.enum;
        return Array.isArray(values) && values.length === 1 ? values[0] : undefined;
      };
      const other = branches.find((b) => tagOf(b) === "Unknown");
      const props = obj(obj(other)?.properties);
      if (!props) return "Task has no `Unknown` branch";
      const known = branches.map(tagOf).filter((t) => t !== undefined && t !== "Unknown");
      props.type = { type: "string", not: { enum: known } };
      return undefined;
    }
  },
  {
    schema: "config.json",
    why:
      "`health_check.smoke_test.apps` is `#[serde(default, skip_serializing)]`, which " +
      "`schemars` emits as required; serde treats it as optional.",
    apply: (doc) => {
      const smoke = definition(doc, "SmokeTestConfig");
      if (!smoke || !Array.isArray(smoke.required) || !smoke.required.includes("apps")) {
        return "SmokeTestConfig does not require apps";
      }
      const rest = smoke.required.filter((r) => r !== "apps");
      if (rest.length === 0) delete smoke.required;
      else smoke.required = rest;
      return undefined;
    }
  },
  {
    schema: "config.json",
    why: '`repositories:` is `#[serde(alias = "data_repos")]`, its old name; `config.yml` is closed.',
    apply: (doc) => {
      const props = obj(doc.properties);
      if (!props?.repositories || "data_repos" in props)
        return "config.json has no lone repositories";
      props.data_repos = props.repositories;
      return undefined;
    }
  },
  {
    schema: "agentic.json",
    why:
      "`LlmVendor` takes `openai` and `openai_compat` as aliases of `open_ai` / " +
      "`open_ai_compat`; `schemars` lists only the renamed spellings.",
    apply: (doc) => {
      const branches = definition(doc, "LlmVendor")?.oneOf;
      if (!Array.isArray(branches)) return "LlmVendor has no oneOf";
      const aliases: Record<string, string> = {
        open_ai: "openai",
        open_ai_compat: "openai_compat"
      };
      const targets = branches
        .map((b) => obj(b)?.enum)
        .filter((v): v is string[] => Array.isArray(v) && v.length === 1 && `${v[0]}` in aliases);
      for (const values of targets) values.push(aliases[values[0] ?? ""] ?? "");
      return targets.length === 2
        ? undefined
        : "LlmVendor's open_ai / open_ai_compat branches are not single-valued";
    }
  },
  {
    schema: "topic.json",
    why:
      "`TopicFilter` flattens an externally tagged enum beside `field:`; `schemars` " +
      "closes each variant (`additionalProperties: false`), which then rejects the " +
      "`field` every filter has.",
    apply: (doc) => {
      const branches = definition(doc, "TopicFilter")?.oneOf;
      if (!Array.isArray(branches) || branches.length === 0) return "TopicFilter has no oneOf";
      for (const branch of branches) {
        const b = obj(branch);
        if (b?.additionalProperties !== false) return "a TopicFilter variant is not closed";
        delete b.additionalProperties;
      }
      return undefined;
    }
  },
  {
    schema: "view.json",
    why: '`MeasureType::Average` is `#[serde(alias = "avg")]`; `schemars` reads `rename`, not `alias`.',
    apply: (doc) => {
      const values = definition(doc, "MeasureType")?.enum;
      if (!Array.isArray(values) || !values.includes("average") || values.includes("avg")) {
        return "MeasureType's enum is not the one with `average` and no `avg`";
      }
      values.push("avg");
      return undefined;
    }
  }
];

/**
 * Apply every patch for `schema` to `doc`, in place. Returns one line per patch
 * that did not apply, naming it and why.
 */
export function applySerdeLeniency(schema: string, doc: Json): string[] {
  const failed: string[] = [];
  for (const patch of PATCHES) {
    if (patch.schema !== schema) continue;
    const reason = patch.apply(doc);
    if (reason)
      failed.push(`${schema}: ${reason} — the patch for "${patch.why}" no longer applies`);
  }
  return failed;
}

/** The schema files that carry a patch, for the test that applies them all. */
export function patchedSchemas(): string[] {
  return [...new Set(PATCHES.map((p) => p.schema))];
}
