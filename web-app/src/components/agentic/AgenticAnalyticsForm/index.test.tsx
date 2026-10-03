// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, type Mock, vi } from "vitest";
import { AgenticAnalyticsForm, type AgenticFormData, type AgenticYamlData } from "./index";

afterEach(() => {
  cleanup();
});

const defaultData: AgenticFormData = {
  instructions: "",
  databases: [],
  llm: { ref: "claude", model: "claude-haiku-4-5", max_tokens: 8000, thinking: "disabled" },
  context: [],
  thinking: undefined,
  states: {}
};

// ─── Rendering ───────────────────────────────────────────────────────────────

describe("AgenticAnalyticsForm — rendering", () => {
  it("renders all top-level sections", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    expect(screen.getByText("Databases")).toBeInTheDocument();
    expect(screen.getByText("LLM Configuration")).toBeInTheDocument();
    expect(screen.getByText("Context")).toBeInTheDocument();
    expect(screen.getByText("State Overrides")).toBeInTheDocument();
  });

  it("renders the instructions textarea", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    expect(screen.getByPlaceholderText(/Global instructions injected/i)).toBeInTheDocument();
  });

  // The thinking mode that applies to every pipeline state lives under LLM
  // Configuration (`llm.thinking`); there is no separate top-level control.
  it("renders the thinking mode select with the configured mode", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    expect(screen.getByRole("combobox", { name: "Thinking Mode" })).toHaveTextContent("Disabled");
  });

  it("pre-fills LLM fields from data", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    expect(screen.getByDisplayValue("claude")).toBeInTheDocument();
    expect(screen.getByDisplayValue("claude-haiku-4-5")).toBeInTheDocument();
    expect(screen.getByDisplayValue("8000")).toBeInTheDocument();
  });

  it("renders all six state override rows", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    for (const state of [
      "clarifying",
      "specifying",
      "solving",
      "executing",
      "interpreting",
      "diagnosing"
    ]) {
      expect(screen.getByText(state)).toBeInTheDocument();
    }
  });

  it("does not show extended thinking section by default when not in data", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    expect(screen.queryByText("Extended Thinking")).not.toBeInTheDocument();
    expect(screen.getByText("Add Extended Thinking")).toBeInTheDocument();
  });

  it("shows extended thinking section when data contains it", () => {
    const data: AgenticFormData = {
      ...defaultData,
      llm: {
        ...defaultData.llm,
        extended_thinking: { model: "claude-opus-4-6", thinking: "adaptive" }
      }
    };
    render(<AgenticAnalyticsForm data={data} />);
    expect(screen.getByText("Extended Thinking")).toBeInTheDocument();
    expect(screen.getByDisplayValue("claude-opus-4-6")).toBeInTheDocument();
  });
});

// ─── LLM Config ──────────────────────────────────────────────────────────────

describe("AgenticAnalyticsForm — LLM config", () => {
  it("renders api_key and base_url fields", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    expect(screen.getByLabelText(/API Key/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/Base URL/i)).toBeInTheDocument();
  });

  // A provider ref inherits its vendor from `config.yml`, so the vendor picker
  // is only offered while no ref is set.
  it("hides the vendor picker while a provider ref is set", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    expect(screen.queryByRole("combobox", { name: "Vendor" })).not.toBeInTheDocument();
  });

  it("shows the vendor picker without a provider ref, and hides it once one is typed", () => {
    const data: AgenticFormData = { ...defaultData, llm: { ...defaultData.llm, ref: undefined } };
    render(<AgenticAnalyticsForm data={data} />);
    expect(screen.getByRole("combobox", { name: "Vendor" })).toHaveTextContent(
      "Select vendor (default: anthropic)"
    );

    fireEvent.change(screen.getByLabelText("Provider Ref"), { target: { value: "claude" } });
    expect(screen.queryByRole("combobox", { name: "Vendor" })).not.toBeInTheDocument();
  });
});

// ─── State Overrides ─────────────────────────────────────────────────────────

describe("AgenticAnalyticsForm — state overrides", () => {
  it("expands a state row and shows instructions + model fields", async () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    fireEvent.click(screen.getByText("specifying"));
    await waitFor(() =>
      expect(
        screen.getByPlaceholderText(/Additional instructions for this state only/i)
      ).toBeInTheDocument()
    );
    expect(screen.getByPlaceholderText(/inherits global if blank/i)).toBeInTheDocument();
  });
});

// ─── Databases ───────────────────────────────────────────────────────────────

describe("AgenticAnalyticsForm — databases", () => {
  it("shows empty state message when no databases", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    expect(screen.getByText(/No databases configured/)).toBeInTheDocument();
  });

  it("renders database inputs when data has entries", () => {
    const data: AgenticFormData = {
      ...defaultData,
      databases: [{ value: "training" }, { value: "production" }]
    };
    render(<AgenticAnalyticsForm data={data} />);
    expect(screen.getByDisplayValue("training")).toBeInTheDocument();
    expect(screen.getByDisplayValue("production")).toBeInTheDocument();
  });

  it("adds a new database input when Add button is clicked", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    fireEvent.click(screen.getByRole("button", { name: /Add Database/i }));
    expect(screen.getAllByPlaceholderText(/Database name/i)).toHaveLength(1);
  });
});

// ─── Context globs ───────────────────────────────────────────────────────────

describe("AgenticAnalyticsForm — context globs", () => {
  it("shows empty state message when no context", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    expect(screen.getByText(/No context patterns defined/)).toBeInTheDocument();
  });

  it("renders glob inputs when data has entries", () => {
    const data: AgenticFormData = {
      ...defaultData,
      context: [{ value: "./semantics/**/*" }, { value: "./example_sql/*.sql" }]
    };
    render(<AgenticAnalyticsForm data={data} />);
    expect(screen.getByDisplayValue("./semantics/**/*")).toBeInTheDocument();
    expect(screen.getByDisplayValue("./example_sql/*.sql")).toBeInTheDocument();
  });

  it("adds a new glob input when Add Glob Pattern button is clicked", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    fireEvent.click(screen.getByRole("button", { name: /Add Glob Pattern/i }));
    expect(screen.getAllByPlaceholderText(/semantics/i)).toHaveLength(1);
  });
});

// ─── onChange serialization ───────────────────────────────────────────────────

describe("AgenticAnalyticsForm — onChange serialization", () => {
  it("calls onChange with correct yaml shape after LLM ref change", async () => {
    const onChange = vi.fn<(data: AgenticYamlData) => void>();
    render(<AgenticAnalyticsForm data={defaultData} onChange={onChange} />);
    const refInput = screen.getByDisplayValue("claude");
    fireEvent.change(refInput, { target: { value: "openai" } });
    fireEvent.blur(refInput);
    await waitFor(() => expect(onChange).toHaveBeenCalled(), { timeout: 1000 });
    const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1][0];
    expect(lastCall.llm?.ref).toBe("openai");
  });

  it("omits empty databases from onChange payload", async () => {
    const onChange = vi.fn<(data: AgenticYamlData) => void>();
    render(<AgenticAnalyticsForm data={defaultData} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /Add Database/i }));
    const refInput = screen.getByDisplayValue("claude");
    fireEvent.change(refInput, { target: { value: "claude" } });
    fireEvent.blur(refInput);
    await waitFor(() => expect(onChange).toHaveBeenCalled(), { timeout: 1000 });
    const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1][0];
    expect(lastCall.databases).toBeUndefined();
  });

  it("serializes context globs as a string array (not object array)", async () => {
    const onChange = vi.fn<(data: AgenticYamlData) => void>();
    const data: AgenticFormData = {
      ...defaultData,
      context: [{ value: "./semantics/**/*" }]
    };
    render(<AgenticAnalyticsForm data={data} onChange={onChange} />);
    const refInput = screen.getByDisplayValue("claude");
    fireEvent.change(refInput, { target: { value: "claude2" } });
    fireEvent.blur(refInput);
    await waitFor(() => expect(onChange).toHaveBeenCalled(), { timeout: 1000 });
    const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1][0];
    expect(Array.isArray(lastCall.context)).toBe(true);
    expect(typeof lastCall.context?.[0]).toBe("string");
  });

  // The editor replaces the whole file with what `onChange` emits. An edit to
  // any other field must therefore carry these keys through untouched — the
  // top-level `thinking` the form has no control for, and the validation and
  // semantic-engine sections it renders — or saving from the form silently
  // rewrites them in the user's YAML.
  it("carries keys it has no controls for through an edit", async () => {
    const onChange = vi.fn<(data: AgenticYamlData) => void>();
    const data: AgenticFormData = {
      ...defaultData,
      thinking: "adaptive",
      validation: {
        rules: {
          solved: [{ name: "outlier_detection", enabled: false, threshold_sigma: 3, min_rows: 6 }]
        }
      },
      semantic_engine: { vendor: "cube", base_url: "https://cube.example.com" }
    };
    render(<AgenticAnalyticsForm data={data} onChange={onChange} />);
    const refInput = screen.getByDisplayValue("claude");
    fireEvent.change(refInput, { target: { value: "openai" } });
    fireEvent.blur(refInput);
    await waitFor(() => expect(onChange).toHaveBeenCalled(), { timeout: 1000 });
    const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1][0];
    expect(lastCall.llm?.ref).toBe("openai");
    expect(lastCall.thinking).toBe("adaptive");
    expect(lastCall.validation).toEqual(data.validation);
    expect(lastCall.semantic_engine).toEqual(data.semantic_engine);
  });
});

/** Edit an unrelated field so the form emits, then return the YAML it emitted. */
const yamlAfterAnEdit = async (
  onChange: Mock<(data: AgenticYamlData) => void>
): Promise<AgenticYamlData> => {
  onChange.mockClear();
  const refInput = screen.getByLabelText("Provider Ref");
  fireEvent.change(refInput, { target: { value: "openai" } });
  fireEvent.blur(refInput);
  await waitFor(() => expect(onChange).toHaveBeenCalled(), { timeout: 1000 });
  return onChange.mock.calls[onChange.mock.calls.length - 1][0];
};

// ─── Semantic Engine ─────────────────────────────────────────────────────────

describe("AgenticAnalyticsForm — semantic engine", () => {
  const section = () => screen.getByRole("region", { name: "Semantic Engine" });

  it("shows add button by default when no semantic engine data", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    expect(
      within(section()).getByRole("button", { name: /Add Semantic Engine/i })
    ).toBeInTheDocument();
    expect(within(section()).queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("shows engine fields after clicking Add Semantic Engine", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    fireEvent.click(screen.getByRole("button", { name: /Add Semantic Engine/i }));
    expect(within(section()).getByRole("combobox", { name: "Vendor" })).toBeInTheDocument();
    expect(within(section()).getByLabelText(/Base URL/)).toBeInTheDocument();
  });

  it("marks vendor and base_url as required", () => {
    const data: AgenticFormData = { ...defaultData, semantic_engine: { vendor: "cube" } };
    render(<AgenticAnalyticsForm data={data} />);
    expect(within(section()).getByRole("combobox", { name: "Vendor" })).toBeRequired();
    expect(within(section()).getByLabelText(/Base URL/)).toBeRequired();
  });

  it("shows api_token field for cube vendor", () => {
    const data: AgenticFormData = {
      ...defaultData,
      semantic_engine: { vendor: "cube", base_url: "https://cube.example.com" }
    };
    render(<AgenticAnalyticsForm data={data} />);
    expect(within(section()).getByLabelText(/API Token/i)).toBeInTheDocument();
    expect(within(section()).queryByLabelText(/Client ID/i)).not.toBeInTheDocument();
  });

  it("shows client id and secret fields for looker vendor", () => {
    const data: AgenticFormData = {
      ...defaultData,
      semantic_engine: { vendor: "looker", base_url: "https://myco.looker.com" }
    };
    render(<AgenticAnalyticsForm data={data} />);
    expect(within(section()).getByLabelText(/Client ID/i)).toBeInTheDocument();
    expect(within(section()).getByLabelText(/Client Secret/i)).toBeInTheDocument();
    expect(within(section()).queryByLabelText(/API Token/i)).not.toBeInTheDocument();
  });

  it("removes the semantic engine from the YAML", async () => {
    const onChange = vi.fn<(data: AgenticYamlData) => void>();
    const data: AgenticFormData = {
      ...defaultData,
      semantic_engine: { vendor: "cube", base_url: "https://cube.example.com" }
    };
    render(<AgenticAnalyticsForm data={data} onChange={onChange} />);
    fireEvent.click(within(section()).getByRole("button", { name: /Remove semantic engine/i }));
    expect(within(section()).getByRole("button", { name: /Add Semantic Engine/i })).toBeVisible();
    expect((await yamlAfterAnEdit(onChange)).semantic_engine).toBeUndefined();
  });
});

// ─── Validation ──────────────────────────────────────────────────────────────

// The backend reads `validation:` as the complete rule list. Absent, every
// built-in rule runs with its defaults; present, ONLY the rules it lists run,
// so `validation: { rules: { solved: [] } }` runs none at all.
describe("AgenticAnalyticsForm — validation", () => {
  const section = () => screen.getByRole("region", { name: "Validation" });

  it("says every built-in rule runs while the file has no validation section", () => {
    render(<AgenticAnalyticsForm data={defaultData} />);
    expect(within(section()).getByText(/Every built-in rule runs/)).toBeInTheDocument();
    expect(within(section()).queryByText("After Specify")).not.toBeInTheDocument();
  });

  it("customizing starts from every built-in rule, grouped by stage", async () => {
    const onChange = vi.fn<(data: AgenticYamlData) => void>();
    render(<AgenticAnalyticsForm data={defaultData} onChange={onChange} />);
    fireEvent.click(within(section()).getByRole("button", { name: /Customize Rules/i }));
    expect(within(section()).getByText("After Specify")).toBeInTheDocument();
    expect(within(section()).getByText("After Solve")).toBeInTheDocument();
    expect(within(section()).getByText("After Execute")).toBeInTheDocument();

    // Starting from an empty list would switch off every rule the user did not
    // re-add; starting from the defaults keeps what runs today.
    const rules = (await yamlAfterAnEdit(onChange)).validation?.rules;
    expect(rules?.specified?.map((r) => r.name)).toEqual([
      "metric_resolves",
      "join_key_exists",
      "filter_unambiguous"
    ]);
    expect(rules?.solvable?.map((r) => r.name)).toEqual([
      "sql_syntax",
      "tables_exist_in_catalog",
      "spec_tables_present",
      "column_refs_valid",
      "timeseries_order_by_check"
    ]);
    expect(rules?.solved?.map((r) => r.name)).toEqual([
      "non_empty",
      "truncation_warning",
      "no_nan_inf",
      "outlier_detection",
      "null_ratio_check",
      "duplicate_row_check",
      "freshness_check"
    ]);
  });

  it("adds a rule to a stage and shows its rule select", () => {
    const data: AgenticFormData = { ...defaultData, validation: { rules: {} } };
    render(<AgenticAnalyticsForm data={data} />);
    expect(within(section()).getAllByRole("button", { name: /Add Rule/i })).toHaveLength(3);
    expect(within(section()).queryByRole("combobox", { name: "Rule" })).not.toBeInTheDocument();
    fireEvent.click(within(section()).getAllByRole("button", { name: /Add Rule/i })[0]);
    expect(within(section()).getByRole("combobox", { name: "Rule" })).toBeInTheDocument();
  });

  it("shows a rule's tunable parameters and whether it is enabled", () => {
    const data: AgenticFormData = {
      ...defaultData,
      validation: {
        rules: {
          solved: [{ name: "outlier_detection", enabled: false, threshold_sigma: 3, min_rows: 6 }]
        }
      }
    };
    render(<AgenticAnalyticsForm data={data} />);
    expect(within(section()).getByRole("combobox", { name: "Rule" })).toHaveTextContent(
      "Outlier Detection"
    );
    expect(within(section()).getByRole("checkbox", { name: "Enabled" })).not.toBeChecked();
    expect(within(section()).getByLabelText(/Threshold/)).toHaveValue(3);
    expect(within(section()).getByLabelText(/Min Rows/)).toHaveValue(6);
  });

  it("keeps an empty validation section through an edit, since empty runs no rules", async () => {
    const onChange = vi.fn<(data: AgenticYamlData) => void>();
    const data: AgenticFormData = { ...defaultData, validation: { rules: { solved: [] } } };
    render(<AgenticAnalyticsForm data={data} onChange={onChange} />);
    expect((await yamlAfterAnEdit(onChange)).validation).toEqual({ rules: {} });
  });

  it("does not add a validation section the file did not have", async () => {
    const onChange = vi.fn<(data: AgenticYamlData) => void>();
    render(<AgenticAnalyticsForm data={defaultData} onChange={onChange} />);
    expect(await yamlAfterAnEdit(onChange)).not.toHaveProperty("validation");
  });

  it("'Use Defaults' removes the section so every built-in rule runs again", async () => {
    const onChange = vi.fn<(data: AgenticYamlData) => void>();
    const data: AgenticFormData = {
      ...defaultData,
      validation: { rules: { solved: [{ name: "non_empty" }] } }
    };
    render(<AgenticAnalyticsForm data={data} onChange={onChange} />);
    fireEvent.click(within(section()).getByRole("button", { name: /Use Defaults/i }));
    expect(within(section()).getByText(/Every built-in rule runs/)).toBeInTheDocument();
    expect(await yamlAfterAnEdit(onChange)).not.toHaveProperty("validation");
  });

  it("leaves a rule out of the YAML until it has a name", async () => {
    const onChange = vi.fn<(data: AgenticYamlData) => void>();
    const data: AgenticFormData = {
      ...defaultData,
      validation: { rules: { solved: [{ name: "non_empty" }] } }
    };
    render(<AgenticAnalyticsForm data={data} onChange={onChange} />);
    fireEvent.click(within(section()).getAllByRole("button", { name: /Add Rule/i })[2]);
    expect((await yamlAfterAnEdit(onChange)).validation).toEqual({
      rules: { solved: [{ name: "non_empty" }] }
    });
  });
});
