// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { describe, expect, it } from "vitest";
import UnknownWorkspacePath from "./UnknownWorkspacePath";

const Where = () => <div data-testid='where'>{useLocation().pathname}</div>;

// The shape App.tsx mounts: a workspace splat route holding its own <Routes>.
function renderAt(path: string) {
  render(
    <MemoryRouter initialEntries={[path]}>
      <Where />
      <Routes>
        <Route
          path='acme/workspaces/:wsId/*'
          element={
            <Routes>
              <Route index element={<div>home</div>} />
              <Route path='threads' element={<div>threads</div>} />
              <Route path='*' element={<UnknownWorkspacePath />} />
            </Routes>
          }
        />
      </Routes>
    </MemoryRouter>
  );
}

describe("UnknownWorkspacePath", () => {
  it("sends the removed Data App addresses to the workspace home, not a blank page", () => {
    for (const path of ["/acme/workspaces/w1/apps", "/acme/workspaces/w1/apps/Zm9v"]) {
      renderAt(path);
      expect(screen.getAllByTestId("where")[0].textContent).toBe("/acme/workspaces/w1");
      expect(screen.getByText("home")).toBeTruthy();
      document.body.innerHTML = "";
    }
  });

  it("does the same for any other address nothing matches, and leaves real ones alone", () => {
    renderAt("/acme/workspaces/w1/nonsense");
    expect(screen.getByText("home")).toBeTruthy();
    document.body.innerHTML = "";
    renderAt("/acme/workspaces/w1/threads");
    expect(screen.getByText("threads")).toBeTruthy();
  });
});
