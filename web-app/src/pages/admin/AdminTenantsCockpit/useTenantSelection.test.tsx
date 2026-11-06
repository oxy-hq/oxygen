// @vitest-environment jsdom
//
// The phone's way back from a dossier must land on a list that exists. Workspaces
// have no list of their own (they live inside their org), so going back from one
// goes to the organizations, not to an empty rail.

import { act, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";
import { useTenantSelection } from "./useTenantSelection";

const at = (url: string) => ({
  wrapper: ({ children }: { children: ReactNode }) => (
    <MemoryRouter initialEntries={[url]}>{children}</MemoryRouter>
  )
});

describe("useTenantSelection.backToList", () => {
  it("clears the id and keeps the type where a list exists", () => {
    const { result } = renderHook(() => useTenantSelection(), at("/?type=users&id=u1"));
    act(() => result.current.backToList());
    expect(result.current.type).toBe("users");
    expect(result.current.id).toBeNull();
  });

  it("goes from a workspace to the organizations", () => {
    const { result } = renderHook(() => useTenantSelection(), at("/?type=workspaces&id=w1"));
    act(() => result.current.backToList());
    expect(result.current.type).toBe("orgs");
    expect(result.current.id).toBeNull();
  });
});
