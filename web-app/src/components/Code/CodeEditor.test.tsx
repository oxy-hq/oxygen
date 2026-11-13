// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import CodeEditor from "./CodeEditor";

afterEach(cleanup);

describe("CodeEditor", () => {
  it("reports edits and runs on Cmd/Ctrl+Enter only", () => {
    const onChange = vi.fn();
    const onExecute = vi.fn();
    render(
      <CodeEditor value='SELECT 1' onChange={onChange} onExecute={onExecute} language='sql' />
    );
    const textarea = screen.getByRole("textbox");

    fireEvent.change(textarea, { target: { value: "SELECT 2" } });
    expect(onChange).toHaveBeenCalledWith("SELECT 2");

    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(onExecute).not.toHaveBeenCalled();
    fireEvent.keyDown(textarea, { key: "Enter", metaKey: true });
    fireEvent.keyDown(textarea, { key: "Enter", ctrlKey: true });
    expect(onExecute).toHaveBeenCalledTimes(2);
  });

  it("highlights the text it shows", () => {
    const { container } = render(
      <CodeEditor value='SELECT 1' onChange={() => {}} language='sql' />
    );
    expect(container.querySelector("pre")?.textContent).toContain("SELECT 1");
  });
});
