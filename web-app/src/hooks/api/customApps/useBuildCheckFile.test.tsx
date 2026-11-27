// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import { AxiosError, type AxiosResponse } from "axios";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppBuildChecksService } from "@/services/api/appBuildChecks";
import { useBuildCheckFile } from "./useBuildCheckFile";

vi.mock("@/services/api/appBuildChecks", () => ({ AppBuildChecksService: { file: vi.fn() } }));

const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={new QueryClient()}>{children}</QueryClientProvider>
);
const AT = { attempt: 2, environment: "staging" };
const file = vi.mocked(AppBuildChecksService.file);

beforeEach(() => {
  URL.createObjectURL = vi.fn(() => "blob:picture");
  URL.revokeObjectURL = vi.fn();
});
afterEach(() => file.mockReset());

describe("useBuildCheckFile", () => {
  it("turns the bytes into an object URL, and gives it back when the picture leaves", async () => {
    const bytes = new Blob(["png"]);
    file.mockResolvedValue(bytes);
    const { result, unmount } = renderHook(
      () => useBuildCheckFile("app-1", "row-s", "shot.png", AT),
      { wrapper }
    );
    expect(result.current).toEqual({ url: null, state: "loading" });
    await waitFor(() => expect(result.current).toEqual({ url: "blob:picture", state: "ready" }));
    expect(file).toHaveBeenCalledWith("app-1", "row-s", "shot.png", AT);
    expect(URL.createObjectURL).toHaveBeenCalledWith(bytes);
    unmount();
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:picture");
  });

  it("asks nothing until it is turned on, and nothing at all for a file never named", () => {
    const idle = renderHook(() => useBuildCheckFile("app-1", "row-s", "shot.png", AT, false), {
      wrapper
    });
    expect(idle.result.current).toEqual({ url: null, state: "idle" });
    const unnamed = renderHook(() => useBuildCheckFile("app-1", "row-s", null, AT), { wrapper });
    expect(unnamed.result.current).toEqual({ url: null, state: "missing" });
    expect(file).not.toHaveBeenCalled();
  });

  it("reads a 404 as missing and any other failure as an error, asking once", async () => {
    const status = (code: number) =>
      new AxiosError("no", "ERR", undefined, undefined, { status: code } as AxiosResponse);
    file.mockRejectedValueOnce(status(404));
    const gone = renderHook(() => useBuildCheckFile("app-1", "row-s", "gone.png", AT), { wrapper });
    await waitFor(() => expect(gone.result.current.state).toBe("missing"));

    file.mockRejectedValueOnce(status(502));
    const down = renderHook(() => useBuildCheckFile("app-1", "row-s", "down.png", AT), { wrapper });
    await waitFor(() => expect(down.result.current.state).toBe("error"));
    expect(file).toHaveBeenCalledTimes(2);
  });
});
