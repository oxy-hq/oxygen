// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/shadcn/tooltip";
import { TodayButton } from "./TodayButton";

vi.mock("./TodayPanel", () => ({ TodayPanel: () => <div data-testid='today-panel' /> }));

afterEach(cleanup);

describe("the top bar's Today button", () => {
  it("opens Today's panel from any page", async () => {
    render(
      <TooltipProvider>
        <TodayButton />
      </TooltipProvider>
    );
    const button = screen.getByTestId("topbar-today-button");
    expect(button).toHaveAccessibleName("Today");
    expect(screen.queryByTestId("today-panel")).toBeNull();
    await userEvent.click(button);
    expect(screen.getByTestId("today-panel")).toBeInTheDocument();
  });
});
