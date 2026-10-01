// @vitest-environment jsdom

/**
 * The two signals a preview puts on the wire, read where every request passes:
 *
 *  - `409 {"code":"preview_read_only","message":…}` — anything that runs or
 *    changes something while pinned. Most call sites toast a fixed "Failed to
 *    …", so the plain reason has to be said centrally or nobody hears it.
 *  - `x-oxy-preview: <branch>@<revision>` — the server's confirmation that it
 *    served the request from the preview, which the bar waits for.
 *
 * Driven through the REAL `apiClient` (a per-request adapter stands in for the
 * network) so the interceptors themselves are what is under test.
 */

import { AxiosError, AxiosHeaders, type InternalAxiosRequestConfig } from "axios";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  toastError: vi.fn(),
  sseResponse: null as unknown as Response
}));

vi.mock("sonner", () => ({ toast: { error: mocks.toastError, success: vi.fn() } }));
vi.mock("@microsoft/fetch-event-source", () => ({
  // The library's contract: a throw from `onopen` is handed to `onerror`, and a
  // throw from `onerror` ends the stream and rejects.
  fetchEventSource: async (
    _url: string,
    opts: { onopen: (r: Response) => Promise<void>; onerror: (e: unknown) => void }
  ) => {
    try {
      await opts.onopen(mocks.sseResponse);
    } catch (err) {
      opts.onerror(err);
    }
  }
}));

import { errMessage } from "@/hooks/api/errMessage";
import {
  isPreviewReadOnlyError,
  isPreviewRevisionServed,
  isRevisionToken,
  resetPreviewServed
} from "@/libs/utils/preview";
import { apiClient } from "./axios";
import fetchSSE from "./fetchSSE";

const READ_ONLY = {
  code: "preview_read_only",
  message: "This is a preview of feat/x. Runs are turned off here — exit the preview to run it."
};

const reject409 =
  (data: unknown, headers: Record<string, string> = {}) =>
  (config: InternalAxiosRequestConfig) =>
    Promise.reject(
      new AxiosError("Request failed with status code 409", "ERR_BAD_REQUEST", config, null, {
        status: 409,
        statusText: "Conflict",
        headers: new AxiosHeaders(headers),
        config,
        data
      })
    );

describe("409 preview_read_only", () => {
  beforeEach(() => {
    mocks.toastError.mockReset();
    resetPreviewServed();
  });

  it("is said plainly, once, in the server's own words", async () => {
    const err = await apiClient
      .post("/ws-1/automations/run", {}, { adapter: reject409(READ_ONLY) })
      .catch((e: unknown) => e);

    expect(mocks.toastError).toHaveBeenCalledTimes(1);
    expect(mocks.toastError).toHaveBeenCalledWith(READ_ONLY.message, { id: "preview-read-only" });
    expect(isPreviewReadOnlyError(err)).toBe(true);
    // A call site that shows the error's message says the same thing, rather
    // than "Request failed with status code 409".
    expect((err as Error).message).toBe(READ_ONLY.message);
    expect(errMessage(err, "Failed to run.")).toBe(READ_ONLY.message);
  });

  it("leaves every other 409 to its caller", async () => {
    const err = await apiClient
      .post("/ws-1/files", {}, { adapter: reject409({ code: "conflict", message: "exists" }) })
      .catch((e: unknown) => e);

    expect(mocks.toastError).not.toHaveBeenCalled();
    expect(isPreviewReadOnlyError(err)).toBe(false);
    expect((err as Error).message).toBe("Request failed with status code 409");
  });

  it("is said plainly when a streamed run is refused, too", async () => {
    mocks.sseResponse = {
      status: 409,
      headers: new Headers({ "x-oxy-preview": "feat/x@rev-7" }),
      json: async () => READ_ONLY
    } as unknown as Response;
    const onError = vi.fn();

    await expect(
      fetchSSE("/api/ws-1/threads/t1/ask", { onMessage: vi.fn(), onError })
    ).rejects.toThrow(READ_ONLY.message);

    expect(onError).toHaveBeenCalledWith(new Error(READ_ONLY.message));
    expect(mocks.toastError).toHaveBeenCalledWith(READ_ONLY.message, { id: "preview-read-only" });
    expect(isPreviewRevisionServed("rev-7")).toBe(true);
  });
});

describe("x-oxy-preview", () => {
  beforeEach(() => resetPreviewServed());

  it("records which revision the server served a preview from", async () => {
    await apiClient.get("/ws-1/agents", {
      params: { branch: "feat/x@2" },
      adapter: (config) =>
        Promise.resolve({
          data: [],
          status: 200,
          statusText: "OK",
          headers: new AxiosHeaders({ "x-oxy-preview": "feat/x@2@0192-abc" }),
          config
        })
    });

    // Split on the last `@`: git allows one inside a branch name, so the
    // revision is `0192-abc` — not `2@0192-abc`.
    expect(isPreviewRevisionServed("0192-abc")).toBe(true);
    expect(isPreviewRevisionServed("2@0192-abc")).toBe(false);
  });

  it("records nothing for a live response", async () => {
    await apiClient.get("/ws-1/agents", {
      adapter: (config) =>
        Promise.resolve({
          data: [],
          status: 200,
          statusText: "OK",
          headers: new AxiosHeaders({}),
          config
        })
    });
    expect(isPreviewRevisionServed("rev-7")).toBe(false);
  });
});

describe("a ?preview= value", () => {
  // It is sent as a header on every request. A value the browser cannot put in
  // a header makes EVERY request throw — so it is simply not a pin.
  it("is a revision only when it is a plain token", () => {
    expect(isRevisionToken("01928c4e-7f2a-7c1d-9a55-3be0f1d2a911")).toBe(true);
    expect(isRevisionToken("rev-7")).toBe(true);
    expect(isRevisionToken("café")).toBe(false);
    expect(isRevisionToken("feat/x")).toBe(false);
    expect(isRevisionToken("a\r\nx-evil: 1")).toBe(false);
    expect(isRevisionToken("")).toBe(false);
  });
});
