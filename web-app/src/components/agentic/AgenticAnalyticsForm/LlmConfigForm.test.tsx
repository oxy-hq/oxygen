// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { FormProvider, useForm } from "react-hook-form";
import { afterEach, describe, expect, it } from "vitest";
import type { AgenticFormData } from "./index";
import { LlmConfigForm } from "./LlmConfigForm";

afterEach(() => {
  cleanup();
});

const Harness = ({ llm }: { llm: AgenticFormData["llm"] }) => {
  const methods = useForm<AgenticFormData>({ defaultValues: { llm } });
  return (
    <FormProvider {...methods}>
      <LlmConfigForm />
    </FormProvider>
  );
};

const withExtendedThinking: AgenticFormData["llm"] = {
  ref: "claude",
  thinking: "disabled",
  extended_thinking: { model: "claude-opus-4-6", thinking: "adaptive" }
};

describe("LlmConfigForm — extended thinking", () => {
  // A <button> inside a <button> is invalid HTML: the trigger's own name and
  // click both swallowed the remove control.
  it("keeps the remove control outside the section's collapse trigger", () => {
    render(<Harness llm={withExtendedThinking} />);
    const trigger = screen.getByRole("button", { name: "Extended Thinking" });
    const remove = screen.getByRole("button", { name: "Remove extended thinking" });
    expect(trigger).not.toContainElement(remove);
  });

  it("collapses from its trigger and removes from its remove control", () => {
    render(<Harness llm={withExtendedThinking} />);
    fireEvent.click(screen.getByRole("button", { name: "Extended Thinking" }));
    expect(screen.queryByDisplayValue("claude-opus-4-6")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove extended thinking" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Remove extended thinking" }));
    expect(screen.queryByRole("button", { name: "Extended Thinking" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add Extended Thinking" })).toBeInTheDocument();
  });

  // Two selects both named "Thinking Mode" cannot be told apart in a list of
  // form controls; the second one sets the extended-thinking mode only.
  it("names the two thinking mode selects apart", () => {
    render(<Harness llm={withExtendedThinking} />);
    expect(screen.getByRole("combobox", { name: "Thinking Mode" })).toHaveTextContent("Disabled");
    expect(screen.getByRole("combobox", { name: "Extended thinking mode" })).toHaveTextContent(
      "Adaptive"
    );
    expect(screen.getByRole("button", { name: "Clear thinking mode" })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Clear extended thinking mode" })
    ).toBeInTheDocument();
  });

  // Same for the two model inputs: the second sets the extended-thinking model only.
  it("names the two model inputs apart", () => {
    render(<Harness llm={{ ...withExtendedThinking, model: "claude-haiku-4-5" }} />);
    expect(screen.getByRole("textbox", { name: "Model" })).toHaveValue("claude-haiku-4-5");
    expect(screen.getByRole("textbox", { name: "Extended thinking model" })).toHaveValue(
      "claude-opus-4-6"
    );
  });
});
