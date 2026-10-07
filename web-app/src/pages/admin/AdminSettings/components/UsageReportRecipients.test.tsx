// @vitest-environment jsdom

import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PlatformCapability } from "@/types/auth";
import type { UsageReportRecipient } from "@/types/usageReport";
import { recipient } from "../testFixtures";

type CurrentUser = { is_owner: boolean; platform_capabilities: PlatformCapability[] };

const user = vi.hoisted(() => ({ value: undefined as CurrentUser | undefined }));
const listCalls = vi.hoisted(() => [] as Array<{ enabled?: boolean }>);
const list = vi.hoisted(() => ({ value: {} as Record<string, unknown> }));
const saving = vi.hoisted(() => ({ value: false }));
const mutate = vi.fn();

vi.mock("@/hooks/api/users/useCurrentUser", () => ({ default: () => ({ data: user.value }) }));
vi.mock("@/hooks/api/usageReport", () => ({
  useUsageReportRecipients: (options: { enabled?: boolean } = {}) => {
    listCalls.push(options);
    return list.value;
  },
  useSetUsageReportRecipient: () => ({ mutate, isPending: saving.value })
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { UsageReportRecipients } from "./UsageReportRecipients";

const GRANTS: CurrentUser = { is_owner: false, platform_capabilities: ["manage_platform_grants"] };

/** Renders the section for `as`, with the server having answered `recipients`. */
const mount = (recipients: UsageReportRecipient[] = [recipient()], as: CurrentUser = GRANTS) => {
  user.value = as;
  list.value = { data: { recipients }, isPending: false, isError: false, error: null };
  render(<UsageReportRecipients />);
};

/** The error axios rejects with when the server answers `status` with `body`. */
const answered = (status: number, body: unknown) =>
  new AxiosError(`Request failed with status code ${status}`, "ERR_BAD_REQUEST", undefined, null, {
    status,
    data: body
  } as AxiosResponse);

const row = (email: string) => screen.getByTestId(`admin-settings-recipient-${email}`);
const switchFor = (email: string) =>
  screen.getByTestId(`admin-settings-recipient-${email}-switch`) as HTMLButtonElement;

type MutateOptions = {
  onSuccess: (saved: UsageReportRecipient) => void;
  onError: (err: unknown) => void;
};

beforeEach(() => {
  user.value = undefined;
  listCalls.length = 0;
  list.value = { data: undefined, isPending: true, isError: false };
  saving.value = false;
  mutate.mockReset();
  vi.mocked(toast.success).mockReset();
  vi.mocked(toast.error).mockReset();
});
afterEach(cleanup);

/**
 * The list decides what another person is sent, so its endpoint is narrower than the
 * page it sits on: `manage_platform_grants`, where the page itself needs only
 * `operate_platform`. Shown to the wrong person it is a table that cannot load; fetched
 * for them it is a 403 toast on every visit to Settings.
 */
describe("UsageReportRecipients — who it is for", () => {
  const hiddenAndNotFetched = (as: CurrentUser | undefined) => {
    user.value = as;
    list.value = { data: { recipients: [recipient()] }, isPending: false, isError: false };
    render(<UsageReportRecipients />);
    expect(screen.queryByTestId("admin-settings-recipients")).toBeNull();
    // Nothing at all rendered — not a heading over an empty space either.
    expect(document.body.textContent).toBe("");
    // The hook ran, as hooks must, and was told not to fetch every time it did.
    expect(listCalls.length).toBeGreaterThan(0);
    expect(listCalls.every((call) => call.enabled === false)).toBe(true);
  };

  it("is hidden, and fetches nothing, for someone holding only the page's own capability", () => {
    hiddenAndNotFetched({ is_owner: false, platform_capabilities: ["operate_platform"] });
  });

  it("is hidden, and fetches nothing, for an App Operator", () => {
    hiddenAndNotFetched({
      is_owner: false,
      platform_capabilities: ["manage_apps", "develop_apps"]
    });
  });

  it("is hidden, and fetches nothing, before the caller's standing has loaded", () => {
    // Unknown standing is not standing: guessing "yes" here is the 403.
    hiddenAndNotFetched(undefined);
  });

  it("is shown, and fetched, for someone holding manage_platform_grants", () => {
    mount();
    expect(screen.getByTestId("admin-settings-recipients")).toBeTruthy();
    expect(listCalls.every((call) => call.enabled === true)).toBe(true);
  });

  it("is shown, and fetched, for the owner, whatever the capability list says", () => {
    mount([recipient()], { is_owner: true, platform_capabilities: [] });
    expect(screen.getByTestId("admin-settings-recipients")).toBeTruthy();
    expect(listCalls.every((call) => call.enabled === true)).toBe(true);
  });
});

describe("UsageReportRecipients — the list", () => {
  it("says what it is and who is on it", () => {
    mount();
    const section = screen.getByTestId("admin-settings-recipients");
    expect(within(section).getByRole("heading", { level: 3 }).textContent).toBe(
      "Who gets the usage report"
    );
    expect(section.textContent).toContain(
      "Global owners and global admins. Turn it off for someone who does not want it."
    );
  });

  it("is one table, with a row per person in the order the server sent", () => {
    mount([
      recipient({ email: "zed@oxy.tech" }),
      recipient({ email: "ada@oxy.tech" }),
      recipient({ email: "mel@oxy.tech" })
    ]);
    expect(screen.getAllByRole("table")).toHaveLength(1);
    expect(screen.getAllByRole("columnheader").map((th) => th.textContent)).toEqual([
      "Person",
      "Role",
      "Reach",
      "Emailed"
    ]);
    // Header row first (no testid), then the people — not sorted by this page.
    expect(screen.getAllByRole("row").map((r) => r.getAttribute("data-testid"))).toEqual([
      null,
      "admin-settings-recipient-zed@oxy.tech",
      "admin-settings-recipient-ada@oxy.tech",
      "admin-settings-recipient-mel@oxy.tech"
    ]);
  });

  it("marks the caller's own row, and only that one", () => {
    mount([
      recipient({ email: "luong@oxy.tech", is_self: true }),
      recipient({ email: "ada@oxy.tech", is_self: false })
    ]);
    expect(screen.getByTestId("admin-settings-recipient-luong@oxy.tech-self").textContent).toBe(
      "you"
    );
    expect(screen.queryByTestId("admin-settings-recipient-ada@oxy.tech-self")).toBeNull();
    expect(screen.getAllByText("you")).toHaveLength(1);
  });

  it("names each person's role, and shows one it does not know as written", () => {
    mount([
      recipient({ email: "own@oxy.tech", role: "global_owner" }),
      recipient({ email: "adm@oxy.tech", role: "global_admin" }),
      recipient({ email: "new@oxy.tech", role: "report_reader" })
    ]);
    const role = (email: string) =>
      screen.getByTestId(`admin-settings-recipient-${email}-role`).textContent;
    expect(role("own@oxy.tech")).toBe("Global owner");
    expect(role("adm@oxy.tech")).toBe("Global admin");
    expect(role("new@oxy.tech")).toBe("report_reader");
  });

  it("says how far each person's grant reaches", () => {
    mount([
      recipient({ email: "all@oxy.tech", scope_all: true, org_count: null }),
      recipient({ email: "few@oxy.tech", scope_all: false, org_count: 3 }),
      recipient({ email: "one@oxy.tech", scope_all: false, org_count: 1 })
    ]);
    const reach = (email: string) =>
      screen.getByTestId(`admin-settings-recipient-${email}-reach`).textContent;
    expect(reach("all@oxy.tech")).toBe("All organizations");
    expect(reach("few@oxy.tech")).toBe("3 organizations");
    expect(reach("one@oxy.tech")).toBe("1 organization");
  });

  it("says so when nobody is set to get it", () => {
    mount([]);
    expect(screen.getByTestId("admin-settings-recipients-empty").textContent).toBe(
      "Nobody is set to get the report."
    );
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("shows the error state for a failed fetch, never 'nobody'", () => {
    // A list that did not load is not an empty list: one says nobody gets the report,
    // the other says we do not know who does.
    user.value = GRANTS;
    list.value = {
      data: undefined,
      isPending: false,
      isError: true,
      error: new Error("grant table unavailable")
    };
    render(<UsageReportRecipients />);
    expect(screen.getByTestId("admin-async-error").textContent).toContain(
      "Couldn’t load the list of people who get the report."
    );
    expect(screen.queryByTestId("admin-settings-recipients-empty")).toBeNull();
    expect(screen.queryByText("Nobody is set to get the report.")).toBeNull();
  });
});

describe("UsageReportRecipients — who turned it off", () => {
  const note = (email: string) =>
    screen.queryByTestId(`admin-settings-recipient-${email}-turned-off-by`);

  const BY_BOSS = { updated_by: "boss@oxy.tech", updated_at: "2026-10-01T12:00:00Z" };

  it("says who, under a person someone else switched off", () => {
    mount([recipient({ email: "ada@oxy.tech", enabled: false, ...BY_BOSS })]);
    // The day is in the reader's own zone, which this test does not choose; the wording
    // and the year are the same in all of them. `recipientText.test.ts` pins the day.
    expect(note("ada@oxy.tech")?.textContent).toMatch(
      /^Turned off by boss@oxy\.tech on \w+ \d+, 2026$/
    );
    expect(within(row("ada@oxy.tech")).getByText(/^Turned off by/)).toBeTruthy();
  });

  it("says nothing under a person who switched it off themselves", () => {
    mount([
      recipient({
        email: "ada@oxy.tech",
        enabled: false,
        updated_by: "ada@oxy.tech",
        updated_at: "2026-10-01T12:00:00Z"
      })
    ]);
    expect(note("ada@oxy.tech")).toBeNull();
    expect(screen.queryByText(/Turned off by/)).toBeNull();
  });

  it("says nothing under a person whose email is on", () => {
    // Someone else changed it last — by turning it back on.
    mount([recipient({ email: "ada@oxy.tech", enabled: true, ...BY_BOSS })]);
    expect(note("ada@oxy.tech")).toBeNull();
  });

  it("says nothing under a person nobody is recorded as having changed", () => {
    mount([recipient({ email: "ada@oxy.tech", enabled: false, updated_by: null })]);
    expect(note("ada@oxy.tech")).toBeNull();
  });

  it("puts the line under the right person in a list of several", () => {
    mount([
      recipient({ email: "ada@oxy.tech", enabled: false, ...BY_BOSS }),
      recipient({ email: "mel@oxy.tech", enabled: true }),
      recipient({ email: "boss@oxy.tech", enabled: false, ...BY_BOSS })
    ]);
    expect(note("ada@oxy.tech")).not.toBeNull();
    expect(note("mel@oxy.tech")).toBeNull();
    // The boss switched their own off: no line, though the same name is in `updated_by`.
    expect(note("boss@oxy.tech")).toBeNull();
  });
});

describe("UsageReportRecipients — the switch", () => {
  const people = [
    recipient({ email: "on@oxy.tech", enabled: true }),
    recipient({ email: "off@oxy.tech", enabled: false })
  ];

  it("shows whether each person is emailed", () => {
    mount(people);
    expect(switchFor("on@oxy.tech").getAttribute("aria-checked")).toBe("true");
    expect(switchFor("off@oxy.tech").getAttribute("aria-checked")).toBe("false");
  });

  it("is named for the person it is about", () => {
    mount(people);
    expect(screen.getByRole("switch", { name: "Email the usage report to on@oxy.tech" })).toBe(
      switchFor("on@oxy.tech")
    );
  });

  it("turns the email off for the person whose switch was on", async () => {
    mount(people);
    await userEvent.click(switchFor("on@oxy.tech"));
    expect(mutate).toHaveBeenCalledTimes(1);
    expect(mutate.mock.calls[0][0]).toEqual({ email: "on@oxy.tech", enabled: false });
  });

  it("turns it on for the person whose switch was off", async () => {
    mount(people);
    await userEvent.click(switchFor("off@oxy.tech"));
    expect(mutate).toHaveBeenCalledTimes(1);
    expect(mutate.mock.calls[0][0]).toEqual({ email: "off@oxy.tech", enabled: true });
  });

  it("asks for no confirmation first", async () => {
    mount(people);
    await userEvent.click(switchFor("on@oxy.tech"));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(mutate).toHaveBeenCalledTimes(1);
  });

  it("holds every switch while a save is in flight", () => {
    saving.value = true;
    mount(people);
    expect(switchFor("on@oxy.tech").disabled).toBe(true);
    expect(switchFor("off@oxy.tech").disabled).toBe(true);
  });

  it("confirms who it changed, from the server's answer", async () => {
    mount(people);
    await userEvent.click(switchFor("on@oxy.tech"));
    const options = mutate.mock.calls[0][1] as MutateOptions;

    options.onSuccess(recipient({ email: "on@oxy.tech", enabled: false }));
    expect(vi.mocked(toast.success).mock.lastCall).toEqual([
      "Usage report emails turned off for on@oxy.tech."
    ]);

    options.onSuccess(recipient({ email: "off@oxy.tech", enabled: true }));
    expect(vi.mocked(toast.success).mock.lastCall).toEqual([
      "Usage report emails turned on for off@oxy.tech."
    ]);
  });

  it("toasts the server's message when the change is refused", async () => {
    mount(people);
    await userEvent.click(switchFor("on@oxy.tech"));
    const options = mutate.mock.calls[0][1] as MutateOptions;

    options.onError(
      answered(404, { code: "not_a_recipient", message: "on@oxy.tech does not get the report." })
    );
    expect(vi.mocked(toast.error).mock.calls).toEqual([["on@oxy.tech does not get the report."]]);
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("names the person when the server gave no reason", async () => {
    mount(people);
    await userEvent.click(switchFor("on@oxy.tech"));
    const options = mutate.mock.calls[0][1] as MutateOptions;

    options.onError(answered(500, undefined));
    expect(vi.mocked(toast.error).mock.calls).toEqual([
      ["Couldn't change the usage report email for on@oxy.tech."]
    ]);
  });
});
