/**
 * The pieces `oxyc validate` ported from the Rust checks, tested on their own.
 * The end-to-end verdicts are `commands/validate-parity.test.ts`.
 */

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { schemaFor } from "../commands/validate.js";
import { templateSyntaxError } from "./jinja.js";
import { automationFindings, workspaceConfig } from "./references.js";
import { legacyGlobalsFinding, variableSyntaxErrors } from "./semantics.js";
import { applySerdeLeniency, patchedSchemas } from "./serde-leniency.js";

const SCHEMAS = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "json-schemas");

describe("serde leniency", () => {
  /**
   * THE DRIFT GUARD. A patch that no longer finds its target means the Rust
   * type changed under it; the patch may now be wrong, or no longer needed.
   * Either way it is a decision for the PR that changed the type.
   */
  it.each(patchedSchemas())("every patch for %s applies to the committed schema", (schema) => {
    const doc = JSON.parse(readFileSync(join(SCHEMAS, schema), "utf8"));
    expect(applySerdeLeniency(schema, doc)).toEqual([]);
  });

  /** …and the check is real: a second application finds its target already changed. */
  it("reports a patch whose target is not there", () => {
    const doc = JSON.parse(readFileSync(join(SCHEMAS, "view.json"), "utf8"));
    applySerdeLeniency("view.json", doc);
    expect(applySerdeLeniency("view.json", doc)).toHaveLength(1);
  });
});

/** Moved from `commands/validate.test.ts`, which spawns the binary; this needs none. */
describe("schemaFor", () => {
  it("maps each file kind to its own schema", () => {
    expect(schemaFor("orders.automation.yml")).toBe("workflow.json");
    expect(schemaFor("analyst.agentic.yml")).toBe("agentic.json");
    expect(schemaFor("config.yml")).toBe("config.json");
  });

  /**
   * `.procedure.yml` is the legacy spelling of `.automation.yml` the platform
   * still accepts, so a validator that skipped it would report a workspace as
   * clean without checking it. `.workflow.yml` is not a recognised kind at all
   * (the walker does not compile it), so it is not validated: a clean report
   * would suggest a file that never runs is fine.
   */
  it("accepts the legacy procedure spelling and not the retired workflow one", () => {
    expect(schemaFor("x.procedure.yml")).toBe("workflow.json");
    expect(schemaFor("x.workflow.yml")).toBeUndefined();
  });

  it("ignores YAML it has no schema for", () => {
    expect(schemaFor("docker-compose.yml")).toBeUndefined();
  });

  /** The manifest's schema is the SDK's, compiled in — named so the walk and `--file` treat it like the rest. */
  it("names the SDK's schema for oxy-app.json, in any directory", () => {
    expect(schemaFor("oxy-app.json")).toBe("oxy-app.schema.json");
    expect(schemaFor("apps/acme/sales/oxy-app.json")).toBe("oxy-app.schema.json");
    expect(schemaFor("not-oxy-app.json")).toBeUndefined();
  });

  /** `config.yml` is matched as a whole name, in any directory. */
  it("finds config.yml in a subdirectory", () => {
    expect(schemaFor("oxy/config.yml")).toBe("config.json");
  });

  /** …but not a file that merely ends in those letters. */
  it("does not match a lookalike name", () => {
    expect(schemaFor("myconfig.yml")).toBeUndefined();
    expect(schemaFor("notanapp.yml")).toBeUndefined();
  });
});

/**
 * Views and topics, against `view.json` / `topic.json` (`schema_for!` on
 * `oxy_semantic`'s types). `.yml` only: the compiler's walker recognises no
 * `.view.yaml`, so one is never served — the `.workflow.yml` rule.
 */
describe("the semantic file kinds", () => {
  it("maps views and topics, in .yml only", () => {
    expect(schemaFor("semantics/views/orders.view.yml")).toBe("view.json");
    expect(schemaFor("models/sales.topic.yml")).toBe("topic.json");
    expect(schemaFor("semantics/views/orders.view.yaml")).toBeUndefined();
    expect(schemaFor("semantics/topics/sales.topic.yaml")).toBeUndefined();
  });
});

/** The Rust unit tests for `validate_consistency_prompt`, and a few more. */
describe("a consistency prompt's template syntax", () => {
  it.each([
    "Hello {{ name }}, welcome!",
    "Just plain text without variables",
    "{% for item in items %}{{ item.name }}{% endfor %}",
    "{% if a %}x{% elif b %}y{% else %}z{% endif %}",
    "{% set x = 1 %}{{ x }}",
    "{# a comment #}{% raw %}{{ not parsed {% %}{% endraw %}",
    "{%- for i in xs -%}{{- i -}}{%- endfor -%}",
    "a { brace } is text"
  ])("accepts %j", (source) => {
    expect(templateSyntaxError(source)).toBeUndefined();
  });

  it.each([
    ["Hello {{ name", /expected end of variable block/],
    ["{% for x in y %}no end", /expected end of block/],
    ["{% if a %}x{% endfor %}", /unexpected `endfor`/],
    ["{% endif %}", /unexpected `endif`/],
    ["{% else %}", /unexpected `else`/],
    ["{{ }}", /end of variable block/],
    ["{# never closed", /end of comment/],
    ["{% raw %}never closed", /raw block/]
  ])("rejects %j", (source, message) => {
    expect(templateSyntaxError(source as string)).toMatch(message as RegExp);
  });
});

/** `validate_variable_syntax`'s cases (the `oxy-semantic` crate's own tests). */
describe("variable syntax in a semantic expression", () => {
  it("accepts a well-formed variable and text with none", () => {
    expect(variableSyntaxErrors("{{variables.schema}}.orders", "Table")).toEqual([]);
    expect(variableSyntaxErrors("{{variables.a_b.c1}}", "Table")).toEqual([]);
    expect(variableSyntaxErrors("amount * 2", "Measure")).toEqual([]);
  });

  it("names a malformed one, and an incomplete one", () => {
    expect(variableSyntaxErrors("{{variables.1bad}}", "Dimension")).toEqual([
      "Dimension contains invalid variable syntax: '{{variables.1bad}}'. Variables must use format: {{variables.name}}"
    ]);
    expect(variableSyntaxErrors("{{variables}}", "Table reference")).toHaveLength(2);
  });

  it("finds the removed globals features on the raw text", () => {
    expect(legacyGlobalsFinding("v.view.yml", "  inherits_from: base\n")?.kind).toBe("parse");
    expect(legacyGlobalsFinding("v.view.yml", "table: {{ globals.s }}.t\n")).toBeDefined();
    expect(legacyGlobalsFinding("v.view.yml", "name: v\n")).toBeUndefined();
  });
});

describe("an automation's references and rules", () => {
  const config = workspaceConfig({
    databases: [{ name: "local" }],
    models: [{ name: "gpt" }],
    integrations: [
      { name: "lk", type: "looker" },
      { name: "om", type: "omni" }
    ]
  });

  it("resolves each reference against the config, by kind", () => {
    const findings = automationFindings(
      "a.automation.yml",
      {
        tasks: [
          { name: "a", type: "execute_sql", database: "local", sql_query: "x" },
          { name: "b", type: "execute_sql", database: "nope", sql_query: "x" },
          { name: "c", type: "looker_query", integration: "om", model: "m", explore: "e" },
          { name: "d", type: "omni_query", integration: "om", topic: "t" }
        ]
      },
      config
    );
    expect(findings.map((f) => `${f.path} ${f.kind}`)).toEqual([
      "tasks.1.database ref",
      "tasks.2.integration ref"
    ]);
  });

  /** Without a config, references are SKIPPED, not failed: the caller says why. */
  it("checks rules but not references when there is no config", () => {
    const findings = automationFindings(
      "a.automation.yml",
      { tasks: [{ name: "", type: "execute_sql", database: "nope", sql_query: "x" }] },
      undefined
    );
    expect(findings.map((f) => `${f.path} ${f.kind}`)).toEqual(["tasks.0.name rule"]);
  });

  it("ignores an export key on a task kind that has none, as serde does", () => {
    const findings = automationFindings(
      "a.automation.yml",
      { tasks: [{ name: "h", type: "http_request", url: "u", export: { path: "", format: "x" } }] },
      config
    );
    expect(findings).toEqual([]);
  });
});
