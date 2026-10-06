// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/shadcn/tooltip";
import { CopyableId } from "./CopyableId";

const FINGERPRINT = "9f2c41d07be35a18";

const mount = (ui: React.ReactNode) => render(<TooltipProvider>{ui}</TooltipProvider>);

afterEach(cleanup);

describe("CopyableId", () => {
  // Grouping is for the eye comparing a fingerprint against a Slack page. It
  // must not change the value: a search for it, or a paste of it, still has to
  // find sixteen unbroken characters.
  it("sets a value in groups by spacing alone, and copies it whole", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    mount(<CopyableId value={FINGERPRINT} full group={4} />);
    const button = screen.getByRole("button");

    expect(button.textContent).toBe(FINGERPRINT);
    const groups = [...button.querySelectorAll("span > span")].map((s) => s.textContent);
    expect(groups).toEqual(["9f2c", "41d0", "7be3", "5a18"]);

    fireEvent.click(button);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(FINGERPRINT));
  });

  it("leaves a value in one run when no grouping is asked for", () => {
    mount(<CopyableId value={FINGERPRINT} full />);
    expect(screen.getByRole("button").querySelectorAll("span > span")).toHaveLength(0);
    expect(screen.getByRole("button").textContent).toBe(FINGERPRINT);
  });
});
