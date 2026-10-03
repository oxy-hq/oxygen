import { describe, expect, it } from "vitest";
import { formDataToYaml, yamlToFormData } from "./formYaml";

/** The file as the form loads it, edited by nothing, written back. */
const roundTrip = (file: Record<string, unknown>) => formDataToYaml(yamlToFormData(file), file);

describe("automation form ⇄ YAML", () => {
  it("keeps a semantic query's filters, which are a list rather than a Looker map", () => {
    const filters = [{ field: "orders.status", op: "eq", value: "open" }];
    const file = {
      name: "a",
      tasks: [{ name: "q", type: "semantic_query", topic: "orders", filters }]
    };
    expect(roundTrip(file).tasks).toEqual(file.tasks);
  });

  it("still turns a Looker query's filter map into rows and back", () => {
    const file = {
      tasks: [{ name: "l", type: "looker_query", filters: { "orders.status": "open" } }]
    };
    const form = yamlToFormData(file);
    expect(form.tasks?.[0].filters).toEqual([{ key: "orders.status", value: "open" }]);
    expect(formDataToYaml(form, file).tasks).toEqual(file.tasks);
  });

  it("writes variables back as a map, not as the JSON text the editor shows", () => {
    const variables = { region: { type: "string", default: "east" } };
    expect(roundTrip({ tasks: [], variables }).variables).toEqual(variables);
  });

  it("keeps the file's variables while the editor holds JSON that does not parse", () => {
    const variables = { region: { type: "string" } };
    const form = { ...yamlToFormData({ tasks: [], variables }), variables: '{ "region": ' };
    expect(formDataToYaml(form, { tasks: [], variables }).variables).toEqual(variables);
  });

  it("keeps top-level keys the form has no fields for", () => {
    const file = { name: "a", tasks: [], consistency_prompt: "Judge it", consistency_model: "m" };
    expect(roundTrip(file)).toEqual(file);
  });

  it("removes a key the form manages when the form no longer has it", () => {
    const file = { name: "a", description: "old", tasks: [] };
    expect(formDataToYaml({ name: "a", tasks: [] }, file)).toEqual({ name: "a", tasks: [] });
  });
});
