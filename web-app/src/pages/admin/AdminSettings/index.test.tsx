// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { MemoryRouter } from "react-router-dom";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PlatformCapability } from "@/types/auth";
import type { UsageReportEmailPreference } from "@/types/usageReport";

const usePreference = vi.fn();
const setMutate = vi.fn();
const sendMutate = vi.fn();
const pending = vi.hoisted(() => ({ set: false, send: false }));
// Who is looking at the page. The default holds the page's own capability and no more,
// which is someone the list of recipients is not shown to.
const user = vi.hoisted(() => ({
  value: { is_owner: false, platform_capabilities: ["operate_platform"] as PlatformCapability[] }
}));
const recipientsCalls = vi.hoisted(() => [] as Array<{ enabled?: boolean }>);

vi.mock("@/hooks/api/usageReport", () => ({
  useUsageReportEmailPreference: () => usePreference(),
  useSetUsageReportEmailPreference: () => ({ mutate: setMutate, isPending: pending.set }),
  useSendUsageReportToMe: () => ({ mutate: sendMutate, isPending: pending.send }),
  useUsageReportRecipients: (options: { enabled?: boolean } = {}) => {
    recipientsCalls.push(options);
    return { data: { recipients: [] }, isPending: false, isError: false, error: null };
  },
  useSetUsageReportRecipient: () => ({ mutate: vi.fn(), isPending: false })
}));
vi.mock("@/hooks/api/users/useCurrentUser", () => ({ default: () => ({ data: user.value }) }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import AdminSettings from "./index";
import { preference } from "./testFixtures";

const mount = (over: Partial<UsageReportEmailPreference> = {}) => {
  usePreference.mockReturnValue({
    data: preference(over),
    isPending: false,
    isError: false,
    error: null
  });
  render(
    <MemoryRouter initialEntries={["/admin/settings"]}>
      <AdminSettings />
    </MemoryRouter>
  );
};

/** The error axios rejects with when the server answers `status` with `body`. */
const answered = (status: number, body: unknown) =>
  new AxiosError(`Request failed with status code ${status}`, "ERR_BAD_REQUEST", undefined, null, {
    status,
    data: body
  } as AxiosResponse);

const theSwitch = () => screen.getByTestId("admin-settings-usage-report-switch");
const sendButton = () => screen.getByTestId("admin-settings-send-latest") as HTMLButtonElement;

type MutateOptions<T> = { onSuccess: (result: T) => void; onError: (err: unknown) => void };

beforeEach(() => {
  usePreference.mockReset();
  setMutate.mockReset();
  sendMutate.mockReset();
  pending.set = false;
  pending.send = false;
  user.value = { is_owner: false, platform_capabilities: ["operate_platform"] };
  recipientsCalls.length = 0;
  vi.mocked(toast.success).mockReset();
  vi.mocked(toast.error).mockReset();
});
afterEach(cleanup);

/**
 * The page mounts the section; the section decides whether it shows. These two only pin
 * that it is on the page and where — everything about the list itself is in
 * `components/UsageReportRecipients.test.tsx`.
 */
describe("AdminSettings — who gets the usage report", () => {
  const headings = () => screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);

  it("is not shown, and its list is not asked for, with only the page's own capability", () => {
    mount();
    expect(headings()).toEqual(["Email notifications"]);
    expect(screen.queryByTestId("admin-settings-recipients")).toBeNull();
    // The section did mount — that is how it came to decide — and asked for nothing.
    expect(recipientsCalls.length).toBeGreaterThan(0);
    expect(recipientsCalls.every((call) => call.enabled === false)).toBe(true);
  });

  it("sits below the email card for someone who may decide it", () => {
    user.value = {
      is_owner: false,
      platform_capabilities: ["operate_platform", "manage_platform_grants"]
    };
    mount();
    expect(headings()).toEqual(["Email notifications", "Who gets the usage report"]);
    expect(recipientsCalls.every((call) => call.enabled === true)).toBe(true);
  });
});

describe("AdminSettings — the page", () => {
  it("is named by the route map and says who the email goes to", () => {
    mount();
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Settings");
    expect(screen.getByRole("heading", { level: 3 }).textContent).toBe("Email notifications");
    expect(screen.getByTestId("admin-settings-usage-report-help").textContent).toBe(
      "A weekly summary of how organizations used their custom apps, sent on Mondays to luong@oxy.tech."
    );
  });

  it("links to the latest report", () => {
    mount();
    expect(screen.getByTestId("admin-settings-open-report").getAttribute("href")).toBe(
      "/admin/usage-report"
    );
  });

  it("shows the error state, and no switch, when the preference cannot be read", () => {
    // A switch drawn from a default would let someone save the opposite of a setting
    // the page never saw.
    usePreference.mockReturnValue({
      data: undefined,
      isPending: false,
      isError: true,
      error: new Error("boom")
    });
    render(
      <MemoryRouter initialEntries={["/admin/settings"]}>
        <AdminSettings />
      </MemoryRouter>
    );
    expect(screen.getByTestId("admin-async-error")).toBeTruthy();
    expect(screen.queryByTestId("admin-settings-usage-report-switch")).toBeNull();
  });
});

describe("AdminSettings — the usage report switch", () => {
  it("is on when the preference is enabled, and turning it off saves false", async () => {
    mount({ enabled: true });
    expect(theSwitch().getAttribute("aria-checked")).toBe("true");

    await userEvent.click(theSwitch());
    expect(setMutate).toHaveBeenCalledTimes(1);
    expect(setMutate.mock.calls[0][0]).toEqual({ enabled: false });
  });

  it("is off when the preference is disabled, and turning it on saves true", async () => {
    mount({ enabled: false });
    expect(theSwitch().getAttribute("aria-checked")).toBe("false");

    await userEvent.click(theSwitch());
    expect(setMutate).toHaveBeenCalledTimes(1);
    expect(setMutate.mock.calls[0][0]).toEqual({ enabled: true });
  });

  it("is named by its label, so a screen reader hears what it switches", () => {
    mount();
    expect(screen.getByRole("switch", { name: "Custom app usage report" })).toBe(theSwitch());
  });

  it("cannot be pressed again while a save is in flight", () => {
    pending.set = true;
    mount();
    expect((theSwitch() as HTMLButtonElement).disabled).toBe(true);
  });

  it("confirms in words what was saved — from the server's answer, not the click", async () => {
    mount({ enabled: true });
    await userEvent.click(theSwitch());
    const options = setMutate.mock.calls[0][1] as MutateOptions<UsageReportEmailPreference>;

    options.onSuccess(preference({ enabled: false }));
    expect(vi.mocked(toast.success).mock.calls).toEqual([["Usage report emails turned off."]]);

    options.onSuccess(preference({ enabled: true }));
    expect(vi.mocked(toast.success).mock.lastCall).toEqual(["Usage report emails turned on."]);
  });

  it("toasts the server's message when the save is refused", async () => {
    mount();
    await userEvent.click(theSwitch());
    const options = setMutate.mock.calls[0][1] as MutateOptions<UsageReportEmailPreference>;

    options.onError(answered(500, { code: "internal", message: "Preference store is read-only." }));
    expect(vi.mocked(toast.error).mock.calls).toEqual([["Preference store is read-only."]]);
    expect(toast.success).not.toHaveBeenCalled();
  });
});

describe("AdminSettings — how this deployment delivers", () => {
  it("says nothing when the report is really emailed", () => {
    mount({ delivery: "email" });
    expect(screen.queryByTestId("admin-settings-delivery-note")).toBeNull();
  });

  it("warns when no sender is configured", () => {
    mount({ delivery: "off" });
    const note = screen.getByTestId("admin-settings-delivery-note");
    expect(note.textContent).toBe(
      "This deployment has no email sender configured, so the report is not emailed. You can still read it under Usage report."
    );
    expect(note.getAttribute("data-delivery")).toBe("off");
  });

  it("explains a deployment that only previews email", () => {
    mount({ delivery: "preview" });
    const note = screen.getByTestId("admin-settings-delivery-note");
    expect(note.textContent).toBe(
      "This deployment previews email in the browser instead of sending it, so the Monday report is not delivered here."
    );
    expect(note.getAttribute("data-delivery")).toBe("preview");
  });
});

describe("AdminSettings — send me the latest report", () => {
  it("is disabled when the deployment has no sender, and sends nothing", async () => {
    mount({ delivery: "off" });
    expect(sendButton().disabled).toBe(true);
    await userEvent.click(sendButton());
    expect(sendMutate).not.toHaveBeenCalled();
  });

  it("is enabled when the deployment emails or previews", () => {
    mount({ delivery: "email" });
    expect(sendButton().disabled).toBe(false);
    cleanup();
    mount({ delivery: "preview" });
    expect(sendButton().disabled).toBe(false);
  });

  it("is disabled while a send is in flight", () => {
    pending.send = true;
    mount({ delivery: "email" });
    expect(sendButton().disabled).toBe(true);
  });

  it("stays available with the Monday email switched off — it is a one-off", () => {
    mount({ enabled: false, delivery: "email" });
    expect(sendButton().disabled).toBe(false);
  });

  it("says where it went, or that a preview opened", async () => {
    mount({ delivery: "email" });
    await userEvent.click(sendButton());
    expect(sendMutate).toHaveBeenCalledTimes(1);
    const options = sendMutate.mock.calls[0][1] as MutateOptions<{
      outcome: "sent" | "previewed";
      to: string;
    }>;

    options.onSuccess({ outcome: "sent", to: "luong@oxy.tech" });
    expect(vi.mocked(toast.success).mock.lastCall).toEqual(["Sent to luong@oxy.tech."]);

    options.onSuccess({ outcome: "previewed", to: "luong@oxy.tech" });
    expect(vi.mocked(toast.success).mock.lastCall).toEqual(["Opened a preview in the browser."]);
  });

  it("toasts the server's message when the send is refused", async () => {
    mount({ delivery: "email" });
    await userEvent.click(sendButton());
    const options = sendMutate.mock.calls[0][1] as MutateOptions<unknown>;

    options.onError(
      answered(404, { code: "no_report", message: "No usage report has been written yet." })
    );
    expect(vi.mocked(toast.error).mock.calls).toEqual([["No usage report has been written yet."]]);
  });
});
