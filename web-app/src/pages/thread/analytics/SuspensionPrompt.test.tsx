// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { HumanInputQuestion } from "@/services/api/analytics";
import SuspensionPrompt from "./SuspensionPrompt";

// lottie-web paints a canvas at import time and jsdom has no canvas. Nothing here
// renders the chart loader; it is only reachable through the prompt's import graph.
vi.mock("@lottiefiles/react-lottie-player", () => ({ Player: "div" }));

// The prompt reads the workspace file tree to offer @-mentions. The real hook needs
// a selected project and a QueryClient, neither of which this component owns.
vi.mock("@/hooks/api/files/useFileTree", () => ({
  default: () => ({
    data: {
      primary: [
        {
          name: "models",
          path: "models",
          is_dir: true,
          children: [
            {
              name: "orders.view.yml",
              path: "models/orders.view.yml",
              is_dir: false,
              children: []
            }
          ]
        }
      ],
      repositories: []
    }
  })
}));

afterEach(() => {
  cleanup();
});

const q = (prompt: string, suggestions: string[] = []): HumanInputQuestion => ({
  prompt,
  suggestions
});

const textbox = () => screen.getByRole<HTMLTextAreaElement>("textbox");

/** The round button under the textarea. It carries no label, only its arrow icon. */
const actionButton = (icon: "arrow-right" | "arrow-up") => {
  const button = document.querySelector(`.lucide-${icon}`)?.closest("button");
  if (!button) throw new Error(`no action button showing ${icon}`);
  return button;
};

describe("SuspensionPrompt — input display logic", () => {
  it("renders the question prompt", () => {
    render(
      <SuspensionPrompt
        questions={[q("What date range?")]}
        onAnswer={vi.fn()}
        isAnswering={false}
      />
    );
    expect(screen.getByText("What date range?")).toBeTruthy();
  });

  it("renders a single textarea for one question", () => {
    render(
      <SuspensionPrompt questions={[q("Pick a metric")]} onAnswer={vi.fn()} isAnswering={false} />
    );
    expect(screen.getAllByRole("textbox")).toHaveLength(1);
    // A lone question has nothing to page through.
    expect(screen.queryByLabelText("Next question")).toBeNull();
  });

  it("shows one question at a time for multiple questions", () => {
    render(
      <SuspensionPrompt
        questions={[q("First?"), q("Second?"), q("Third?")]}
        onAnswer={vi.fn()}
        isAnswering={false}
      />
    );
    expect(screen.getByText("3 questions")).toBeTruthy();
    expect(screen.getByText("1 / 3")).toBeTruthy();
    expect(screen.getAllByRole("textbox")).toHaveLength(1);
    expect(screen.getByText("First?")).toBeTruthy();
    expect(screen.queryByText("Second?")).toBeNull();
    expect(screen.queryByText("Third?")).toBeNull();
  });

  it("pages between questions and keeps each answer", () => {
    render(
      <SuspensionPrompt
        questions={[q("First?"), q("Second?")]}
        onAnswer={vi.fn()}
        isAnswering={false}
      />
    );
    expect(screen.getByLabelText<HTMLButtonElement>("Previous question").disabled).toBe(true);
    fireEvent.change(textbox(), { target: { value: "one" } });

    fireEvent.click(screen.getByLabelText("Next question"));
    expect(screen.getByText("Second?")).toBeTruthy();
    expect(screen.queryByText("First?")).toBeNull();
    expect(screen.getByText("2 / 2")).toBeTruthy();
    expect(textbox().value).toBe("");
    expect(screen.getByLabelText<HTMLButtonElement>("Next question").disabled).toBe(true);

    fireEvent.click(screen.getByLabelText("Previous question"));
    expect(screen.getByText("First?")).toBeTruthy();
    expect(textbox().value).toBe("one");
  });

  it("renders suggestion chips", () => {
    render(
      <SuspensionPrompt
        questions={[q("Pick one", ["Option A", "Option B"])]}
        onAnswer={vi.fn()}
        isAnswering={false}
      />
    );
    expect(screen.getByText("Option A")).toBeTruthy();
    expect(screen.getByText("Option B")).toBeTruthy();
  });

  it("calls onAnswer immediately when a suggestion chip is clicked for a single question", () => {
    const onAnswer = vi.fn();
    render(
      <SuspensionPrompt
        questions={[q("Pick one", ["Option A"])]}
        onAnswer={onAnswer}
        isAnswering={false}
      />
    );
    fireEvent.click(screen.getByText("Option A"));
    expect(onAnswer).toHaveBeenCalledWith("Option A");
  });

  it("fills textarea when suggestion chip is clicked for multiple questions", () => {
    const onAnswer = vi.fn();
    render(
      <SuspensionPrompt
        questions={[q("Q1", ["Chip1"]), q("Q2")]}
        onAnswer={onAnswer}
        isAnswering={false}
      />
    );
    fireEvent.click(screen.getByText("Chip1"));
    // chip click on multi-question fills the field, does not submit
    expect(onAnswer).not.toHaveBeenCalled();
    expect(textbox().value).toBe("Chip1");
  });

  it("advances to the next unanswered question rather than sending a partial set", () => {
    const onAnswer = vi.fn();
    render(
      <SuspensionPrompt questions={[q("Q1"), q("Q2")]} onAnswer={onAnswer} isAnswering={false} />
    );
    // Nothing typed yet: there is nothing to move on from.
    expect(actionButton("arrow-right").disabled).toBe(true);

    fireEvent.change(textbox(), { target: { value: "Answer 1" } });
    fireEvent.click(actionButton("arrow-right"));

    expect(onAnswer).not.toHaveBeenCalled();
    expect(screen.getByText("Q2")).toBeTruthy();
    expect(screen.queryByText("Q1")).toBeNull();
  });

  it("submits combined answers for multiple questions", () => {
    const onAnswer = vi.fn();
    render(
      <SuspensionPrompt questions={[q("Q1"), q("Q2")]} onAnswer={onAnswer} isAnswering={false} />
    );
    fireEvent.change(textbox(), { target: { value: "Answer 1" } });
    fireEvent.keyDown(textbox(), { key: "Enter", shiftKey: false });
    expect(onAnswer).not.toHaveBeenCalled();

    fireEvent.change(textbox(), { target: { value: "Answer 2" } });
    // Every question is answered, so the button turns from "next" into "send".
    fireEvent.click(actionButton("arrow-up"));
    expect(onAnswer).toHaveBeenCalledTimes(1);
    expect(onAnswer).toHaveBeenCalledWith("Q: Q1\nA: Answer 1\n\nQ: Q2\nA: Answer 2");
  });

  it("disables inputs and buttons while isAnswering", () => {
    render(
      <SuspensionPrompt questions={[q("Q1", ["chip"])]} onAnswer={vi.fn()} isAnswering={true} />
    );
    expect(textbox().disabled).toBe(true);
    expect(screen.getByText<HTMLButtonElement>("chip").disabled).toBe(true);
    expect(actionButton("arrow-up").disabled).toBe(true);
  });

  it("submits on Enter for a single question", () => {
    const onAnswer = vi.fn();
    render(<SuspensionPrompt questions={[q("Q1")]} onAnswer={onAnswer} isAnswering={false} />);
    fireEvent.change(textbox(), { target: { value: "my answer" } });
    fireEvent.keyDown(textbox(), { key: "Enter", shiftKey: true });
    expect(onAnswer).not.toHaveBeenCalled();
    fireEvent.keyDown(textbox(), { key: "Enter", shiftKey: false });
    expect(onAnswer).toHaveBeenCalledWith("my answer");
  });

  it("does not submit an empty answer", () => {
    const onAnswer = vi.fn();
    render(<SuspensionPrompt questions={[q("Q1")]} onAnswer={onAnswer} isAnswering={false} />);
    fireEvent.change(textbox(), { target: { value: "   " } });
    fireEvent.keyDown(textbox(), { key: "Enter", shiftKey: false });
    expect(onAnswer).not.toHaveBeenCalled();
  });

  it("sends an @-mention as a file reference, not as the text the user sees", () => {
    const onAnswer = vi.fn();
    render(
      <SuspensionPrompt questions={[q("Which view?")]} onAnswer={onAnswer} isAnswering={false} />
    );
    fireEvent.change(textbox(), { target: { value: "use @ord" } });
    expect(screen.getByText("models/orders.view.yml")).toBeTruthy();

    // Enter picks the highlighted file while the popup is open; it does not submit.
    fireEvent.keyDown(textbox(), { key: "Enter", shiftKey: false });
    expect(onAnswer).not.toHaveBeenCalled();
    expect(textbox().value).toBe("use @orders ");

    fireEvent.keyDown(textbox(), { key: "Enter", shiftKey: false });
    expect(onAnswer).toHaveBeenCalledWith("use <@models/orders.view.yml|orders>");
  });
});
