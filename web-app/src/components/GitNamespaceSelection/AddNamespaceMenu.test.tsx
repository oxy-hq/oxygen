// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import AddNamespaceMenu from "./AddNamespaceMenu";

afterEach(() => cleanup());

const menu = (canUseApp: boolean) =>
  render(
    <AddNamespaceMenu
      open={true}
      canUseApp={canUseApp}
      onClose={() => {}}
      onSelectApp={vi.fn()}
      onSelectPAT={vi.fn()}
    />
  );

describe("AddNamespaceMenu", () => {
  it("offers the GitHub App to an org owner or admin", () => {
    menu(true);
    expect(screen.getByRole("button", { name: /via github app/i })).toBeEnabled();
  });

  it("keeps the GitHub App from a plain member, and says why", () => {
    menu(false);
    expect(screen.getByRole("button", { name: /via github app/i })).toBeDisabled();
    expect(screen.getByText(/only organization owners and admins/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /personal access token/i })).toBeEnabled();
  });
});
