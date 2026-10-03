// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import MessageInputShell from "./MessageInputShell";

afterEach(() => {
  cleanup();
});

const renderShell = (props: { value?: string; isLoading?: boolean }) => {
  const onSend = vi.fn();
  const onStop = vi.fn();
  render(
    <MessageInputShell
      value={props.value ?? ""}
      onChange={vi.fn()}
      onKeyDown={vi.fn()}
      onSend={onSend}
      onStop={onStop}
      disabled={false}
      isLoading={props.isLoading}
    />
  );
  return { onSend, onStop };
};

// Both controls are a bare arrow / circle icon. The testids are what the agentic
// flows and e2e page objects drive, so they stay on the named buttons.
describe("MessageInputShell send and stop buttons", () => {
  it("names the send button and sends the message", () => {
    const { onSend } = renderShell({ value: "How many orders last week?" });
    const send = screen.getByRole("button", { name: "Send message" });
    expect(send).toHaveAttribute("data-testid", "message-input-send-button");

    fireEvent.click(send);
    expect(onSend).toHaveBeenCalledTimes(1);
  });

  it("disables send while there is nothing to send", () => {
    renderShell({ value: "   " });
    expect(screen.getByRole("button", { name: "Send message" })).toBeDisabled();
  });

  it("names the stop button while a run is in flight and stops it", () => {
    const { onStop } = renderShell({ value: "", isLoading: true });
    expect(screen.queryByRole("button", { name: "Send message" })).not.toBeInTheDocument();
    const stop = screen.getByRole("button", { name: "Stop run" });
    expect(stop).toHaveAttribute("data-testid", "message-input-stop-button");

    fireEvent.click(stop);
    expect(onStop).toHaveBeenCalledTimes(1);
  });
});
