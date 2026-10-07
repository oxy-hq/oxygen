// @vitest-environment jsdom
import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { UsageReportService } from "@/services/api/usageReport";
import type {
  UsageReportEmailPreference,
  UsageReportRecipient,
  UsageReportRecipientsResponse
} from "@/types/usageReport";
import {
  deferred,
  everyone,
  LIST_KEY,
  OWN_KEY,
  ownPreference,
  person,
  seededClient
} from "./testSupport";

vi.mock("@/services/api/usageReport", () => ({
  UsageReportService: {
    setRecipient: vi.fn(),
    recipients: vi.fn(),
    getEmailPreference: vi.fn()
  }
}));

import { useSetUsageReportRecipient } from "./useSetUsageReportRecipient";

const setRecipient = vi.mocked(UsageReportService.setRecipient);

const mountWith = (seed: {
  own?: UsageReportEmailPreference;
  list?: UsageReportRecipientsResponse;
}) => {
  const client = seededClient(seed);
  const { result } = renderHook(() => useSetUsageReportRecipient(), { wrapper: client.wrapper });
  return { result, ...client };
};

/** Starts a save the test settles by hand, and waits until the request has gone out. */
const startSaving = async (
  mounted: ReturnType<typeof mountWith>,
  email: string,
  enabled: boolean
) => {
  const inFlight = deferred<UsageReportRecipient>();
  setRecipient.mockReturnValue(inFlight.promise);
  act(() => {
    mounted.result.current.mutate({ email, enabled });
  });
  await waitFor(() => expect(setRecipient).toHaveBeenCalled());
  return inFlight;
};

const settle = async (
  inFlight: ReturnType<typeof deferred<UsageReportRecipient>>,
  outcome: UsageReportRecipient | Error
) => {
  await act(async () => {
    if (outcome instanceof Error) inFlight.reject(outcome);
    else inFlight.resolve(outcome);
    await inFlight.promise.catch(() => {});
  });
};

afterEach(() => {
  vi.clearAllMocks();
});

describe("useSetUsageReportRecipient", () => {
  it("sends the person's address and the chosen value", async () => {
    setRecipient.mockResolvedValue(person({ email: "ada@oxy.tech", enabled: false }));
    const { result } = mountWith({ own: ownPreference, list: everyone() });
    await act(async () => {
      await result.current.mutateAsync({ email: "ada@oxy.tech", enabled: false });
    });
    expect(setRecipient.mock.calls).toEqual([["ada@oxy.tech", false]]);
  });

  it("moves that person's switch before the server answers, and nobody else's", async () => {
    const mounted = mountWith({ own: ownPreference, list: everyone() });
    const inFlight = await startSaving(mounted, "ada@oxy.tech", false);
    await waitFor(() => expect(mounted.row("ada@oxy.tech")?.enabled).toBe(false));
    expect(mounted.row("mel@oxy.tech")).toEqual(person({ email: "mel@oxy.tech" }));
    expect(mounted.row("luong@oxy.tech")?.enabled).toBe(true);
    await settle(inFlight, person({ email: "ada@oxy.tech", enabled: false }));
  });

  it("does not credit the last person to change it while the save is in flight", async () => {
    // Mel turned Ada's email off and on again last month. Turning it off now must not
    // show "Turned off by mel" for the moment before the server says who really did.
    const stale = { updated_by: "mel@oxy.tech", updated_at: "2026-09-01T00:00:00Z" };
    const mounted = mountWith({
      list: { recipients: [person({ email: "ada@oxy.tech", enabled: true, ...stale })] }
    });
    const inFlight = await startSaving(mounted, "ada@oxy.tech", false);
    await waitFor(() => expect(mounted.row("ada@oxy.tech")?.enabled).toBe(false));
    expect(mounted.row("ada@oxy.tech")).toMatchObject({ updated_by: null, updated_at: null });
    await settle(inFlight, person({ email: "ada@oxy.tech", enabled: false }));
  });

  it("puts the switch back, with who changed it last, when the server refuses", async () => {
    const before = person({
      email: "ada@oxy.tech",
      enabled: true,
      updated_by: "mel@oxy.tech",
      updated_at: "2026-09-01T00:00:00Z"
    });
    const mounted = mountWith({ list: { recipients: [before] } });
    const inFlight = await startSaving(mounted, "ada@oxy.tech", false);
    await waitFor(() => expect(mounted.row("ada@oxy.tech")?.enabled).toBe(false));

    await settle(inFlight, new Error("not a recipient"));
    await waitFor(() => expect(mounted.result.current.isError).toBe(true));
    expect(mounted.row("ada@oxy.tech")).toEqual(before);
  });

  it("does not invent a list when none has been loaded", async () => {
    const mounted = mountWith({ own: ownPreference });
    const inFlight = await startSaving(mounted, "ada@oxy.tech", false);
    expect(mounted.list()).toBeUndefined();
    await settle(inFlight, person({ email: "ada@oxy.tech", enabled: false }));
  });

  it("marks both caches stale once the save settles", async () => {
    const mounted = mountWith({ own: ownPreference, list: everyone() });
    const inFlight = await startSaving(mounted, "ada@oxy.tech", false);
    await settle(inFlight, person({ email: "ada@oxy.tech", enabled: false }));
    await waitFor(() =>
      expect(mounted.queryClient.getQueryState(LIST_KEY)?.isInvalidated).toBe(true)
    );
    expect(mounted.queryClient.getQueryState(OWN_KEY)?.isInvalidated).toBe(true);
  });
});

/**
 * The caller's own row and the switch at the top of Settings are one setting shown
 * twice. Changing it through the row has to move the other, or the page disagrees with
 * itself about whether the person looking at it gets the email.
 */
describe("useSetUsageReportRecipient — the caller's own row", () => {
  it("moves the caller's own preference with it, before the server answers", async () => {
    const mounted = mountWith({ own: ownPreference, list: everyone() });
    const inFlight = await startSaving(mounted, "luong@oxy.tech", false);
    await waitFor(() => expect(mounted.row("luong@oxy.tech")?.enabled).toBe(false));
    expect(mounted.own()).toEqual({ ...ownPreference, enabled: false });
    await settle(inFlight, person({ email: "luong@oxy.tech", is_self: true, enabled: false }));
  });

  it("leaves the caller's own preference alone when the row is someone else's", async () => {
    const mounted = mountWith({ own: ownPreference, list: everyone() });
    const inFlight = await startSaving(mounted, "ada@oxy.tech", false);
    await waitFor(() => expect(mounted.row("ada@oxy.tech")?.enabled).toBe(false));
    // Turning Ada's email off must not turn off the email of whoever did it.
    expect(mounted.own()).toEqual(ownPreference);
    await settle(inFlight, person({ email: "ada@oxy.tech", enabled: false }));
  });

  it("puts both back when the server refuses", async () => {
    const mounted = mountWith({ own: ownPreference, list: everyone() });
    const inFlight = await startSaving(mounted, "luong@oxy.tech", false);
    await waitFor(() => expect(mounted.own()?.enabled).toBe(false));

    await settle(inFlight, new Error("refused"));
    await waitFor(() => expect(mounted.result.current.isError).toBe(true));
    expect(mounted.own()).toEqual(ownPreference);
    expect(mounted.list()).toEqual(everyone());
  });

  it("goes by the list's own mark for the caller, not by comparing addresses", async () => {
    // The preference and the list spell the caller's address differently. The server has
    // already said which row is theirs, and that is what decides.
    const mounted = mountWith({
      own: { ...ownPreference, email: "Luong@Oxy.Tech" },
      list: everyone()
    });
    const inFlight = await startSaving(mounted, "luong@oxy.tech", false);
    await waitFor(() => expect(mounted.own()?.enabled).toBe(false));
    await settle(inFlight, person({ email: "luong@oxy.tech", is_self: true, enabled: false }));
  });
});
