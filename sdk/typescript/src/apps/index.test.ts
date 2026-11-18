import { describe, expect, it } from "vitest";
import {
  APP_CALL_LABELS,
  AppCallError,
  type AppCallErrorKind,
  appCallErrorFromHostMessage,
  isAppCallError
} from "./index";

describe("appCallErrorFromHostMessage", () => {
  const cases: [string, AppCallErrorKind][] = [
    ["AppCallRefused", "refused"],
    ["AppCallNotFound", "not_found"],
    ["AppCallNotCallable", "not_callable"],
    ["AppCallNotDeployed", "not_deployed"],
    ["AppCallTimeout", "timeout"],
    ["AppCallFailed", "failed"],
    ["AppCallBusy", "busy"],
    ["AppCallDepthExceeded", "depth_exceeded"]
  ];

  it.each(cases)("maps %s to %s", (label, kind) => {
    const host = `${label}: something went wrong: really`;
    const err = appCallErrorFromHostMessage(host);
    expect(err).toBeInstanceOf(AppCallError);
    expect(err?.kind).toBe(kind);
    expect(err?.name).toBe("AppCallError");
    expect(err?.message).toBe("something went wrong: really");
    expect(err?.hostMessage).toBe(host);
  });

  it("covers every label", () => {
    expect(Object.keys(APP_CALL_LABELS).sort()).toEqual(cases.map(([l]) => l).sort());
  });

  it("leaves other host messages alone", () => {
    expect(appCallErrorFromHostMessage("AppsCapabilityMissing: nope")).toBeNull();
    expect(appCallErrorFromHostMessage("no label here")).toBeNull();
    expect(appCallErrorFromHostMessage("constructor: x")).toBeNull();
    expect(appCallErrorFromHostMessage("__proto__: x")).toBeNull();
  });
});

describe("isAppCallError", () => {
  it("recognises the class and a lookalike, not other errors", () => {
    expect(isAppCallError(new AppCallError("busy", "x"))).toBe(true);
    expect(
      isAppCallError(Object.assign(new Error("x"), { name: "AppCallError", kind: "busy" }))
    ).toBe(true);
    expect(isAppCallError(new Error("x"))).toBe(false);
    expect(isAppCallError(null)).toBe(false);
  });
});
