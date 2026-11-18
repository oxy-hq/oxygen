// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useReducer } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RoleCeiling, TokenOptionOrg } from "@/types/apiToken";
import {
  type AccessDraft,
  accessReducer,
  emptyDraft,
  type GrantTarget,
  grantsFromDraft
} from "../../../accessDraft";
import OrgGrantCard from "./OrgGrantCard";

// Radix's select cannot open in jsdom; a native one drives the same `onChange`.
vi.mock("./CeilingSelect", () => ({
  default: ({
    value,
    onChange,
    testId
  }: {
    value: RoleCeiling;
    onChange: (next: RoleCeiling) => void;
    testId: string;
  }) => (
    <select
      data-testid={testId}
      value={value}
      onChange={(e) => onChange(e.target.value as RoleCeiling)}
    >
      {["viewer", "member", "admin", "owner"].map((c) => (
        <option key={c} value={c}>
          {c}
        </option>
      ))}
    </select>
  )
}));

const org = (workspaces: TokenOptionOrg["workspaces"]): TokenOptionOrg => ({
  org_id: "o1",
  org_name: "Acme",
  org_slug: "acme",
  role: "owner",
  via: "member",
  policy: { max_lifetime_days: null, allow_all_access_tokens: true },
  workspaces
});

const ONE = org([{ workspace_id: "w1", name: "Analytics", role: "owner" }]);
const TWO = org([
  { workspace_id: "w1", name: "Analytics", role: "owner" },
  { workspace_id: "w2", name: "Staging", role: "member" }
]);

/** A token narrowed to w1, as the edit dialog loads it. */
const narrowedTo = (workspaceId: string, ceiling: RoleCeiling, revoked: GrantTarget[] = []) => {
  const draft: AccessDraft = { ...emptyDraft(), mode: "selected", revoked };
  draft.orgs.o1 = { wide: false, wideCeiling: "owner", workspaces: { [workspaceId]: ceiling } };
  return draft;
};

let latest: AccessDraft = emptyDraft();

const Harness = ({ start, option }: { start: AccessDraft; option: TokenOptionOrg }) => {
  const [draft, dispatch] = useReducer(accessReducer, start);
  latest = draft;
  return (
    <OrgGrantCard org={option} draft={draft.orgs.o1} revoked={draft.revoked} dispatch={dispatch} />
  );
};

afterEach(cleanup);

describe("OrgGrantCard", () => {
  it("keeps a narrowed grant narrowed when its access changes, in an org with several", () => {
    render(<Harness start={narrowedTo("w1", "viewer")} option={TWO} />);
    expect(screen.getByTestId("account-token-org-partial")).toHaveTextContent("only part of it");

    fireEvent.change(screen.getByTestId("account-token-org-ceiling"), {
      target: { value: "member" }
    });
    // Still one grant on w1, now at Write: never widened to every workspace in the org.
    expect(grantsFromDraft(latest)).toEqual([
      { kind: "workspace", org_id: "o1", workspace_id: "w1", role_ceiling: "member" }
    ]);
  });

  it("turns a narrowed grant org-wide only where the org has one workspace", () => {
    render(<Harness start={narrowedTo("w1", "viewer")} option={ONE} />);
    expect(screen.queryByTestId("account-token-org-partial")).not.toBeInTheDocument();

    fireEvent.change(screen.getByTestId("account-token-org-ceiling"), {
      target: { value: "member" }
    });
    expect(grantsFromDraft(latest)).toEqual([
      { kind: "workspace", org_id: "o1", workspace_id: null, role_ceiling: "member" }
    ]);
  });

  it("caps an org-wide grant at the lowest role it covers", () => {
    render(<Harness start={{ ...emptyDraft(), mode: "selected" }} option={TWO} />);
    fireEvent.click(screen.getByTestId("account-token-org-wide-checkbox"));
    // Full by default, but the owner is only a member of Staging.
    expect(screen.getByText(/capped at Write, your role here/)).toBeInTheDocument();
  });

  it("lets a narrowed grant be widened on purpose, by ticking the org again", () => {
    render(<Harness start={narrowedTo("w1", "admin")} option={TWO} />);
    const box = screen.getByTestId("account-token-org-wide-checkbox");
    fireEvent.click(box);
    expect(grantsFromDraft(latest)).toEqual([]);
    fireEvent.click(box);
    expect(grantsFromDraft(latest)).toEqual([
      { kind: "workspace", org_id: "o1", workspace_id: null, role_ceiling: "owner" }
    ]);
  });

  it("closes the org only when what was taken away is what a tick would add back", () => {
    // Several workspaces: the org removed one narrowed grant, not the org-wide one.
    const partly = narrowedTo("w2", "viewer", [{ orgId: "o1", workspaceId: "w1" }]);
    delete partly.orgs.o1;
    render(<Harness start={partly} option={TWO} />);
    expect(screen.getByTestId("account-token-org-wide-checkbox")).not.toBeDisabled();
    cleanup();

    // One workspace: either removal reached the same data, so the org is closed.
    const closed = narrowedTo("w1", "viewer", [{ orgId: "o1", workspaceId: "w1" }]);
    delete closed.orgs.o1;
    render(<Harness start={closed} option={ONE} />);
    expect(screen.getByTestId("account-token-org-wide-checkbox")).toBeDisabled();
    expect(screen.getByTestId("account-token-grant-removed")).toHaveTextContent("removed by Acme");
  });
});
