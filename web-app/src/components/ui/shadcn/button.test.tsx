// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { RefreshCw } from "lucide-react";
import { afterEach, describe, expect, it } from "vitest";
import { Button } from "./button";

afterEach(() => {
  cleanup();
});

// A Radix tooltip is a description shown on hover, not a name: until it opens,
// nothing on the trigger says what an icon-only button does.
describe("Button tooltip as accessible name", () => {
  it("names an icon-only button after its string tooltip", () => {
    render(
      <Button size='icon' tooltip='Refresh schema'>
        <RefreshCw />
      </Button>
    );
    expect(screen.getByRole("button", { name: "Refresh schema" })).toBeInTheDocument();
  });

  it("names an icon-only button after a tooltip config whose content is text", () => {
    render(
      <Button size='icon' tooltip={{ content: "Expand Sidebar", side: "right" }}>
        <RefreshCw />
      </Button>
    );
    expect(screen.getByRole("button", { name: "Expand Sidebar" })).toBeInTheDocument();
  });

  it("names an icon-only link rendered through asChild", () => {
    render(
      <Button asChild size='icon' tooltip='Open docs'>
        <a href='/docs'>
          <RefreshCw />
        </a>
      </Button>
    );
    expect(screen.getByRole("link", { name: "Open docs" })).toBeInTheDocument();
  });

  // ShellRail's tiles say "(opens in a new tab)" on the <a> itself, not in the tooltip.
  it("keeps the name an asChild element gives itself", () => {
    render(
      <Button asChild size='icon' tooltip='Docs'>
        <a href='/docs' aria-label='Docs (opens in a new tab)'>
          <RefreshCw />
        </a>
      </Button>
    );
    expect(screen.getByRole("link", { name: "Docs (opens in a new tab)" })).toBeInTheDocument();
  });

  it("keeps a name the caller gave", () => {
    render(
      <Button size='icon' tooltip='Refresh' aria-label='Refresh the schema of my-postgres'>
        <RefreshCw />
      </Button>
    );
    expect(
      screen.getByRole("button", { name: "Refresh the schema of my-postgres" })
    ).toBeInTheDocument();
  });

  it("does not override aria-labelledby", () => {
    render(
      <>
        <span id='label'>Reload</span>
        <Button size='icon' tooltip='Refresh' aria-labelledby='label'>
          <RefreshCw />
        </Button>
      </>
    );
    const button = screen.getByRole("button", { name: "Reload" });
    expect(button).not.toHaveAttribute("aria-label");
  });

  // Visible text is the name a speech-input user says; a tooltip that differs
  // from it must not replace it.
  it("leaves a button with visible text named by that text", () => {
    render(
      <Button tooltip='Stores the values'>
        <RefreshCw /> Save
      </Button>
    );
    const button = screen.getByRole("button", { name: "Save" });
    expect(button).not.toHaveAttribute("aria-label");
  });

  it("finds visible text nested inside an element", () => {
    render(
      <Button tooltip='Stores the values'>
        <span>
          <strong>Save</strong>
        </span>
      </Button>
    );
    expect(screen.getByRole("button", { name: "Save" })).not.toHaveAttribute("aria-label");
  });
});
