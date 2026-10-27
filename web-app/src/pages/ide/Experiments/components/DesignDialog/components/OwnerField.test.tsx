// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useForm } from "react-hook-form";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DesignFormValues } from "../designForm";
import OwnerField from "./OwnerField";

vi.mock("@/stores/useCurrentOrg", () => ({
  default: (pick: (s: { org: { id: string } }) => unknown) => pick({ org: { id: "org-1" } })
}));
vi.mock("@/hooks/api/organizations/useOrgMembers", () => ({
  useOrgMembers: () => ({
    data: [
      { id: "m1", user_id: "u1", email: "maya@acme.test", name: "Maya", role: "owner" },
      { id: "m2", user_id: "u2", email: "amara@acme.test", name: "Amara", role: "member" }
    ]
  })
}));
vi.mock("@/hooks/api/appAccess/useTeams", () => ({
  useTeams: () => ({
    data: [{ id: "t1", name: "ops", description: null, member_count: 3, created_at: "" }]
  })
}));

afterEach(cleanup);

function Harness({ owner = "" }: { owner?: string }) {
  const form = useForm<DesignFormValues>({ defaultValues: { owner } as DesignFormValues });
  return <OwnerField form={form} messageFor={() => undefined} />;
}

const input = () => screen.getByTestId("experiment-design-owner") as HTMLInputElement;
const status = () => screen.getByTestId("experiment-design-owner-match");

describe("OwnerField", () => {
  it("opens a styled list of teams and members on focus, and picking one fills the field", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(input());

    const list = await screen.findByRole("listbox", { name: "Owner suggestions" });
    const options = Array.from(list.querySelectorAll("[role=option]")).map((o) =>
      o.getAttribute("data-value")
    );
    expect(options).toEqual(["ops", "amara@acme.test", "maya@acme.test"]);
    expect(list).toHaveTextContent("Teams");
    expect(list).toHaveTextContent("Members");
    expect(list).toHaveTextContent("Maya");

    await user.click(screen.getByRole("option", { name: /maya@acme\.test/ }));
    expect(input().value).toBe("maya@acme.test");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("filters the list by what is typed", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.type(input(), "ama");
    const values = screen.getAllByRole("option").map((o) => o.getAttribute("data-value"));
    expect(values).toEqual(["amara@acme.test"]);
  });

  it("is not a field password managers should fill", () => {
    render(<Harness />);
    expect(input()).toHaveAttribute("autocomplete", "off");
    expect(input()).toHaveAttribute("data-bwignore", "true");
    expect(input()).toHaveAttribute("data-1p-ignore", "true");
    expect(input()).not.toHaveAttribute("list");
    expect(input().getAttribute("placeholder") ?? "").not.toMatch(/email/i);
  });

  it("says who a typed owner reaches", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.type(input(), "ops");
    expect(status()).toHaveTextContent("Team ops · 3 members");
    expect(status()).toHaveAttribute("data-tone", "success");

    await user.clear(input());
    await user.type(input(), "maya@acme.test");
    expect(status()).toHaveTextContent("Maya");
    expect(status()).toHaveAttribute("data-tone", "success");
  });

  it("warns when the owner matches nobody, since notices then go to the org's owners and admins", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.type(input(), "opps");
    expect(status()).toHaveAttribute("data-tone", "warning");
    expect(status()).toHaveTextContent("org's owners and admins");
  });

  it("matches exactly as the server does: a differently-cased email reaches nobody", () => {
    render(<Harness owner='Maya@Acme.test' />);
    expect(status()).toHaveAttribute("data-tone", "warning");
  });
});
