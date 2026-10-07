// @vitest-environment jsdom

/**
 * The page with its real hooks, against a stand-in for the server.
 *
 * Every other settings test mocks the hooks, which is right for what a component renders
 * and useless for the one thing only the hooks and the page together can get wrong: the
 * switch at the top and the caller's own row in the list are the same setting, and must
 * not disagree on screen — while a save is in flight, after it lands, or after it fails.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { UsageReportService } from "@/services/api/usageReport";
import type { PlatformCapability } from "@/types/auth";
import type { UsageReportEmailPreference, UsageReportRecipient } from "@/types/usageReport";
import { preference, recipient } from "./testFixtures";

const user = vi.hoisted(() => ({
  value: { is_owner: false, platform_capabilities: [] as PlatformCapability[] }
}));

vi.mock("@/hooks/api/users/useCurrentUser", () => ({ default: () => ({ data: user.value }) }));
vi.mock("@/services/api/usageReport", () => ({
  UsageReportService: {
    getEmailPreference: vi.fn(),
    setEmailPreference: vi.fn(),
    recipients: vi.fn(),
    setRecipient: vi.fn(),
    sendToMe: vi.fn()
  }
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import AdminSettings from "./index";

const service = vi.mocked(UsageReportService);

const ME = "luong@oxy.tech";
const ADA = "ada@oxy.tech";

/** What the stand-in server holds: who is emailed. */
let emailed: Record<string, boolean>;
/** When set, the next save waits on this instead of answering at once. */
let hold: { promise: Promise<void>; release: () => void; refuse: (err: Error) => void } | null;

const ownAnswer = (): UsageReportEmailPreference => preference({ email: ME, enabled: emailed[ME] });
const rowAnswer = (email: string): UsageReportRecipient =>
  recipient({ email, is_self: email === ME, enabled: emailed[email] });

/** Makes the next save wait until the test releases or refuses it. */
const holdNextSave = () => {
  let release!: () => void;
  let refuse!: (err: Error) => void;
  const promise = new Promise<void>((res, rej) => {
    release = res;
    refuse = rej;
  });
  hold = { promise, release, refuse };
  return hold;
};

/** A save: waits if the test is holding it, then records the change and answers. */
const save = async <T,>(apply: () => void, answer: () => T): Promise<T> => {
  const held = hold;
  hold = null;
  if (held) await held.promise;
  apply();
  return answer();
};

const mount = (capabilities: PlatformCapability[]) => {
  user.value = { is_owner: false, platform_capabilities: capabilities };
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } }
  });
  render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={["/admin/settings"]}>
        <AdminSettings />
      </MemoryRouter>
    </QueryClientProvider>
  );
};

const BOTH: PlatformCapability[] = ["operate_platform", "manage_platform_grants"];

const topSwitch = () => screen.getByTestId("admin-settings-usage-report-switch");
const rowSwitch = (email: string) => screen.getByTestId(`admin-settings-recipient-${email}-switch`);
const isOn = (el: HTMLElement) => el.getAttribute("aria-checked") === "true";

/** Both halves of the page have loaded. */
const loaded = async () => {
  await screen.findByTestId("admin-settings-usage-report-switch");
  await screen.findByTestId(`admin-settings-recipient-${ME}-switch`);
};

beforeEach(() => {
  emailed = { [ME]: true, [ADA]: true };
  hold = null;
  service.getEmailPreference.mockImplementation(async () => ownAnswer());
  service.recipients.mockImplementation(async () => ({
    recipients: [rowAnswer(ME), rowAnswer(ADA)]
  }));
  service.setEmailPreference.mockImplementation((enabled) =>
    save(
      () => {
        emailed[ME] = enabled;
      },
      () => ownAnswer()
    )
  );
  service.setRecipient.mockImplementation((email, enabled) =>
    save(
      () => {
        emailed[email] = enabled;
      },
      () => rowAnswer(email)
    )
  );
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("AdminSettings — the top switch and the caller's own row are one setting", () => {
  it("shows both on to begin with", async () => {
    mount(BOTH);
    await loaded();
    expect(isOn(topSwitch())).toBe(true);
    expect(isOn(rowSwitch(ME))).toBe(true);
  });

  it("turns the top switch off when the caller's own row is turned off", async () => {
    mount(BOTH);
    await loaded();
    const held = holdNextSave();

    await userEvent.click(rowSwitch(ME));
    // While the save is still in flight — not once a re-read catches up. Nothing has
    // been fetched a second time yet, so this can only be the two caches moving together.
    await waitFor(() => expect(isOn(topSwitch())).toBe(false));
    expect(isOn(rowSwitch(ME))).toBe(false);
    expect(service.getEmailPreference).toHaveBeenCalledTimes(1);
    expect(service.setRecipient.mock.calls).toEqual([[ME, false]]);
    expect(service.setEmailPreference).not.toHaveBeenCalled();

    await act(async () => {
      held.release();
    });
    // And after the server has been asked again, both still say what it holds.
    await waitFor(() => expect(service.getEmailPreference).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(service.recipients).toHaveBeenCalledTimes(2));
    expect(isOn(topSwitch())).toBe(false);
    expect(isOn(rowSwitch(ME))).toBe(false);
  });

  it("turns the caller's own row off when the top switch is turned off", async () => {
    mount(BOTH);
    await loaded();
    const held = holdNextSave();

    await userEvent.click(topSwitch());
    await waitFor(() => expect(isOn(rowSwitch(ME))).toBe(false));
    expect(isOn(topSwitch())).toBe(false);
    expect(service.recipients).toHaveBeenCalledTimes(1);
    expect(service.setEmailPreference.mock.calls).toEqual([[false]]);
    expect(service.setRecipient).not.toHaveBeenCalled();

    await act(async () => {
      held.release();
    });
    await waitFor(() => expect(service.recipients).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(service.getEmailPreference).toHaveBeenCalledTimes(2));
    expect(isOn(topSwitch())).toBe(false);
    expect(isOn(rowSwitch(ME))).toBe(false);
  });

  it("leaves the top switch alone when the row turned off is someone else's", async () => {
    mount(BOTH);
    await loaded();
    const held = holdNextSave();

    await userEvent.click(rowSwitch(ADA));
    await waitFor(() => expect(isOn(rowSwitch(ADA))).toBe(false));
    expect(isOn(topSwitch())).toBe(true);
    expect(isOn(rowSwitch(ME))).toBe(true);

    await act(async () => {
      held.release();
    });
    await waitFor(() => expect(service.recipients).toHaveBeenCalledTimes(2));
    expect(isOn(rowSwitch(ADA))).toBe(false);
    expect(isOn(topSwitch())).toBe(true);
    expect(isOn(rowSwitch(ME))).toBe(true);
  });

  it("puts both back when the change to the caller's own row is refused", async () => {
    mount(BOTH);
    await loaded();
    const held = holdNextSave();

    await userEvent.click(rowSwitch(ME));
    await waitFor(() => expect(isOn(topSwitch())).toBe(false));

    await act(async () => {
      held.refuse(new Error("refused"));
    });
    await waitFor(() => expect(isOn(topSwitch())).toBe(true));
    expect(isOn(rowSwitch(ME))).toBe(true);
    expect(emailed[ME]).toBe(true);
  });

  it("puts both back when the change through the top switch is refused", async () => {
    mount(BOTH);
    await loaded();
    const held = holdNextSave();

    await userEvent.click(topSwitch());
    await waitFor(() => expect(isOn(rowSwitch(ME))).toBe(false));

    await act(async () => {
      held.refuse(new Error("refused"));
    });
    await waitFor(() => expect(isOn(rowSwitch(ME))).toBe(true));
    expect(isOn(topSwitch())).toBe(true);
  });
});

/**
 * The list's endpoint needs `manage_platform_grants`; the page needs only
 * `operate_platform`. For someone holding just the latter, a single request for the
 * list is a 403 and a "you don't have permission" toast on a page they are allowed on.
 */
describe("AdminSettings — someone who may not decide who gets the report", () => {
  it("is never the reason the list is asked for — on arrival, or after using their own switch", async () => {
    mount(["operate_platform"]);
    await screen.findByTestId("admin-settings-usage-report-switch");
    expect(screen.queryByTestId("admin-settings-recipients")).toBeNull();

    // Their own switch refreshes "who is emailed" everywhere it is shown. For them that
    // is one place, and the list must stay unasked-for.
    await userEvent.click(topSwitch());
    await waitFor(() => expect(service.getEmailPreference).toHaveBeenCalledTimes(2));
    expect(isOn(topSwitch())).toBe(false);
    expect(service.recipients).not.toHaveBeenCalled();
  });
});
