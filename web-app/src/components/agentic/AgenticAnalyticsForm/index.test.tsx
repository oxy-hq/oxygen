// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
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
    expect(screen.getByText("Thinking Mode")).toBeInTheDocument();
    expect(screen.getByRole("combobox")).toHaveTextContent("Disabled");
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
    expect(screen.queryByText("Vendor")).not.toBeInTheDocument();
    expect(screen.queryByText("Select vendor (default: anthropic)")).not.toBeInTheDocument();
  });

  it("shows the vendor picker without a provider ref, and hides it once one is typed", () => {
    const data: AgenticFormData = { ...defaultData, llm: { ...defaultData.llm, ref: undefined } };
    render(<AgenticAnalyticsForm data={data} />);
    expect(screen.getByText("Vendor")).toBeInTheDocument();
    expect(screen.getByText("Select vendor (default: anthropic)")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Provider Ref"), { target: { value: "claude" } });
    expect(screen.queryByText("Vendor")).not.toBeInTheDocument();
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

  // The form has no controls for `thinking`, `validation` or `semantic_engine`,
  // but the editor replaces the whole file with what `onChange` emits. An edit
  // to any other field must therefore carry those keys through untouched, or
  // saving from the form silently deletes them from the user's YAML.
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
