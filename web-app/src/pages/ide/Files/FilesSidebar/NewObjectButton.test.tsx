// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import NewObjectButton from "./NewObjectButton";

// The Create button is disabled while a create is in flight; Enter in the name field has
// to be held to the same rule, or a double Enter creates the same file twice.

vi.setConfig({ testTimeout: 20000 });

const createFile = vi.fn();
const saveFile = vi.fn();

vi.mock("@/hooks/api/files/useCreateFile", () => ({
  default: () => ({ mutateAsync: createFile })
}));
vi.mock("@/hooks/api/files/useSaveFile", () => ({ default: () => ({ mutateAsync: saveFile }) }));
vi.mock("@/hooks/api/files/useFileTree", () => ({
  default: () => ({ data: { primary: [] }, refetch: vi.fn() })
}));
vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "p1" }, branchName: "main" })
}));
vi.mock("@/stores/useCurrentOrg", () => ({ default: () => "acme" }));
vi.mock("react-router-dom", () => ({ useNavigate: () => vi.fn() }));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));
vi.mock("@/components/auth/Can", () => ({
  CanWorkspaceEditor: ({ children }: { children: ReactNode }) => children
}));
vi.mock("@/pages/ide/pipelines/components/NewPipelineDialog", () => ({ default: () => null }));
// The menu is only the way in to the name dialog; a flat list of buttons gets there
// without Radix's pointer handling, which jsdom does not implement.
vi.mock("@/components/ui/shadcn/dropdown-menu", () => {
  const Pass = ({ children }: { children?: ReactNode }) => children;
  return {
    DropdownMenu: Pass,
    DropdownMenuContent: Pass,
    DropdownMenuSub: Pass,
    DropdownMenuSubContent: Pass,
    DropdownMenuSubTrigger: Pass,
    DropdownMenuTrigger: Pass,
    DropdownMenuItem: ({ children, onClick }: { children?: ReactNode; onClick?: () => void }) => (
      <button type='button' onClick={onClick}>
        {children}
      </button>
    )
  };
});

afterEach(() => cleanup());
beforeEach(() => {
  createFile.mockReset();
  saveFile.mockReset();
});

describe("NewObjectButton", () => {
  it("creates once when Enter is pressed again while the create is in flight", async () => {
    // Never settles, so the first create is still in flight for the second Enter.
    createFile.mockReturnValue(new Promise(() => {}));
    render(<NewObjectButton />);

    await userEvent.click(screen.getByText("Automation"));
    await userEvent.type(screen.getByLabelText("Name"), "nightly{Enter}{Enter}");

    expect(createFile).toHaveBeenCalledTimes(1);
  });
});
